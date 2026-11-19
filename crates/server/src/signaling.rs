//! 시그널링: 접속 관리, 방송 목록, 방송 비밀번호 검사, SFU 세션 요청.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dongsi_core::InstallId;
use dongsi_core::discovery::DiscoveredBroadcast;
use dongsi_core::proto::{
    BroadcastInfo, ClientMsg, ErrorCode, MAX_PASSWORD_CHARS, MAX_SDP_BYTES, PROTOCOL_VERSION,
    PublishInfo, ServerMsg, sanitize_display_name,
};
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tracing::{debug, info, warn};

use crate::ServerConfig;
use crate::discovery::DiscoverySnapshot;
use crate::sfu::{Role, SfuCmd, SfuEvent, SfuHandle};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_CONNECTIONS: usize = 256;
/// 비밀번호 실패: IP마다 1분에 이 횟수까지.
const PASSWORD_FAILURES_PER_MINUTE: usize = 8;

/// 서버 상태 요약.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStats {
    pub connections: usize,
    pub broadcasts: usize,
    pub viewers: usize,
}

struct Conn {
    tx: mpsc::UnboundedSender<ServerMsg>,
}

struct Broadcast {
    key: u64,
    conn: u64,
    publisher_session: u64,
    name: String,
    info: PublishInfo,
    password: Option<String>,
    started_at: u64,
    viewers: HashSet<u64>,
}

enum SessionKind {
    Publisher(InstallId),
    Viewer(InstallId),
}

struct SessionInfo {
    conn: u64,
    kind: SessionKind,
}

struct State {
    conns: HashMap<u64, Conn>,
    broadcasts: BTreeMap<InstallId, Broadcast>,
    sessions: HashMap<u64, SessionInfo>,
    password_failures: HashMap<IpAddr, Vec<Instant>>,
    next_id: u64,
}

impl State {
    fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn broadcast_list(&self) -> Vec<BroadcastInfo> {
        self.broadcasts
            .iter()
            .map(|(id, b)| BroadcastInfo {
                broadcaster: *id,
                name: b.name.clone(),
                codec: b.info.codec,
                width: b.info.width,
                height: b.info.height,
                fps: b.info.fps,
                has_audio: b.info.has_audio,
                locked: b.password.is_some(),
                viewers: b.viewers.len() as u32,
                started_at: b.started_at,
            })
            .collect()
    }
}

struct Shared {
    config: ServerConfig,
    port: u16,
    state: Mutex<State>,
    sfu: crossbeam_channel::Sender<SfuCmd>,
    discovery: Option<DiscoverySnapshot>,
    stats: Arc<RwLock<ServerStats>>,
    external_port: Arc<AtomicU16>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 방송 목록이 바뀌었을 때: 모든 접속자에게 알리고, 감지 응답과 통계를 갱신한다.
    fn publish_list(&self, state: &State) {
        let list = state.broadcast_list();
        for conn in state.conns.values() {
            let _ = conn.tx.send(ServerMsg::Broadcasts {
                broadcasts: list.clone(),
            });
        }
        for b in state.broadcasts.values() {
            if let Some(conn) = state.conns.get(&b.conn) {
                let _ = conn.tx.send(ServerMsg::ViewerCount {
                    count: b.viewers.len() as u32,
                });
            }
        }
        if let Some(d) = &self.discovery {
            let mut d = d.write().unwrap_or_else(|e| e.into_inner());
            d.broadcasts = list
                .iter()
                .map(|b| DiscoveredBroadcast {
                    broadcaster: b.broadcaster,
                    name: b.name.clone(),
                    codec: b.codec,
                    width: b.width,
                    height: b.height,
                    fps: b.fps,
                    locked: b.locked,
                })
                .collect();
        }
        self.update_stats(state);
    }

    fn update_stats(&self, state: &State) {
        let mut s = self.stats.write().unwrap_or_else(|e| e.into_inner());
        s.connections = state.conns.len();
        s.broadcasts = state.broadcasts.len();
        s.viewers = state.broadcasts.values().map(|b| b.viewers.len()).sum();
    }

    /// 방송을 끝낸다(방송자 종료, 접속 끊김, 송출 세션 끊김).
    fn end_broadcast(&self, state: &mut State, broadcaster: InstallId) {
        let Some(b) = state.broadcasts.remove(&broadcaster) else {
            return;
        };
        info!(%broadcaster, name = %b.name, "방송 종료");
        let _ = self.sfu.send(SfuCmd::Remove {
            session: b.publisher_session,
        });
        state.sessions.remove(&b.publisher_session);
        for v in b.viewers {
            let _ = self.sfu.send(SfuCmd::Remove { session: v });
            if let Some(info) = state.sessions.remove(&v)
                && let Some(conn) = state.conns.get(&info.conn)
            {
                let _ = conn.tx.send(ServerMsg::BroadcastEnded { broadcaster });
            }
        }
        self.publish_list(state);
    }

    fn remove_session(&self, state: &mut State, session: u64) {
        let Some(info) = state.sessions.remove(&session) else {
            return;
        };
        match info.kind {
            SessionKind::Publisher(id) => {
                // 송출 연결이 끊기면 방송도 끝난다.
                if state.broadcasts.get(&id).map(|b| b.publisher_session) == Some(session) {
                    state.sessions.insert(session, info);
                    self.end_broadcast(state, id);
                }
            }
            SessionKind::Viewer(id) => {
                let _ = self.sfu.send(SfuCmd::Remove { session });
                if let Some(b) = state.broadcasts.get_mut(&id) {
                    b.viewers.remove(&session);
                }
                self.publish_list(state);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    config: ServerConfig,
    port: u16,
    listener: TcpListener,
    tls: Arc<tokio_rustls::rustls::ServerConfig>,
    mut sfu: SfuHandle,
    mut sfu_events: mpsc::UnboundedReceiver<SfuEvent>,
    discovery: Option<DiscoverySnapshot>,
    stats: Arc<RwLock<ServerStats>>,
    external_port: Arc<AtomicU16>,
    mut shutdown: watch::Receiver<bool>,
) {
    let shared = Arc::new(Shared {
        config,
        port,
        state: Mutex::new(State {
            conns: HashMap::new(),
            broadcasts: BTreeMap::new(),
            sessions: HashMap::new(),
            password_failures: HashMap::new(),
            next_id: 0,
        }),
        sfu: sfu.cmd.clone(),
        discovery,
        stats,
        external_port,
    });
    let acceptor = TlsAcceptor::from(tls);
    let mut conn_tasks = tokio::task::JoinSet::new();

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            event = sfu_events.recv() => {
                let Some(SfuEvent::SessionClosed { session }) = event else { continue };
                let mut state = shared.lock();
                shared.remove_session(&mut state, session);
            }
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(x) => x,
                    Err(e) => {
                        debug!("accept 실패: {e}");
                        continue;
                    }
                };
                if shared.lock().conns.len() >= MAX_CONNECTIONS {
                    continue;
                }
                let shared = shared.clone();
                let acceptor = acceptor.clone();
                let shutdown = shutdown.clone();
                conn_tasks.spawn(async move {
                    if let Err(e) = handle_conn(shared, acceptor, stream, peer, shutdown).await {
                        debug!(%peer, "접속 종료: {e}");
                    }
                });
            }
            Some(_) = conn_tasks.join_next(), if !conn_tasks.is_empty() => {}
        }
    }

    // 종료: 접속자에게 알리고 잠시 기다린다.
    {
        let state = shared.lock();
        for conn in state.conns.values() {
            let _ = conn.tx.send(ServerMsg::Closing {
                reason: "서버가 닫혔습니다".into(),
            });
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while conn_tasks.join_next().await.is_some() {}
    })
    .await;
    conn_tasks.abort_all();
    tokio::task::spawn_blocking(move || sfu.shutdown())
        .await
        .ok();
    info!("서버 종료");
}

async fn handle_conn(
    shared: Arc<Shared>,
    acceptor: TlsAcceptor,
    stream: TcpStream,
    peer: SocketAddr,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let _ = stream.set_nodelay(true);
    let local = stream.local_addr().map_err(|e| e.to_string())?;
    let tls = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream))
        .await
        .map_err(|_| "TLS 시간 초과".to_string())?
        .map_err(|e| format!("TLS 실패: {e}"))?;
    let mut ws_config = WebSocketConfig::default();
    ws_config.max_message_size = Some(256 * 1024);
    ws_config.max_frame_size = Some(256 * 1024);
    let ws = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        tokio_tungstenite::accept_async_with_config(tls, Some(ws_config)),
    )
    .await
    .map_err(|_| "WebSocket 시간 초과".to_string())?
    .map_err(|e| format!("WebSocket 실패: {e}"))?;
    let (mut sink, mut stream) = ws.split();

    // Hello
    let hello = tokio::time::timeout(HANDSHAKE_TIMEOUT, next_msg(&mut stream))
        .await
        .map_err(|_| "Hello 시간 초과".to_string())??;
    let ClientMsg::Hello {
        protocol,
        install_id,
        display_name,
        connected_ip,
        host_token,
        ..
    } = hello
    else {
        return Err("첫 메시지가 Hello가 아닙니다".into());
    };
    if protocol != PROTOCOL_VERSION {
        let msg = ServerMsg::Error {
            req: None,
            code: ErrorCode::ProtocolMismatch,
            message: ErrorCode::ProtocolMismatch.default_message().into(),
        };
        let _ = sink
            .send(Message::text(
                serde_json::to_string(&msg).unwrap_or_default(),
            ))
            .await;
        return Err(format!("프로토콜 버전이 다릅니다: {protocol}"));
    }
    let name = sanitize_display_name(&display_name).unwrap_or_else(|| "이름 없음".into());
    let can_publish = match &shared.config.host_token {
        None => true,
        Some(token) => host_token
            .as_deref()
            .is_some_and(|t| constant_time_eq(t.as_bytes(), token.as_bytes())),
    };
    let candidate_ip = candidate_ip(connected_ip, local, peer);
    let candidate = SocketAddr::new(candidate_ip, media_port(&shared, candidate_ip, peer.ip()));

    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMsg>();
    let conn_id = {
        let mut state = shared.lock();
        let id = state.next_id();
        state.conns.insert(id, Conn { tx: tx.clone() });
        let _ = tx.send(ServerMsg::Welcome {
            protocol: PROTOCOL_VERSION,
            server_name: shared.config.name.clone(),
            server_version: env!("CARGO_PKG_VERSION").into(),
            can_publish,
            host_only: shared.config.host_token.is_some(),
            broadcasts: state.broadcast_list(),
        });
        shared.update_stats(&state);
        id
    };
    info!(%peer, conn = conn_id, %name, can_publish, %candidate, "접속");

    let ctx = ConnCtx {
        shared: shared.clone(),
        conn_id,
        install_id,
        name,
        can_publish,
        peer_ip: peer.ip(),
        candidate,
        tx: tx.clone(),
    };

    let result = async {
        let mut shutting_down = false;
        loop {
            tokio::select! {
                out = rx.recv() => {
                    let Some(msg) = out else { break };
                    let closing = matches!(msg, ServerMsg::Closing { .. });
                    let text = serde_json::to_string(&msg).map_err(|e| e.to_string())?;
                    sink.send(Message::text(text)).await.map_err(|e| e.to_string())?;
                    if closing {
                        break;
                    }
                }
                incoming = tokio::time::timeout(IDLE_TIMEOUT, next_msg(&mut stream)) => {
                    let msg = incoming.map_err(|_| "응답 없음".to_string())??;
                    ctx.handle(msg).await;
                }
                changed = shutdown.changed(), if !shutting_down => {
                    // 서버가 닫히면 Closing 메시지가 rx로 온다. 그 메시지를 보낸 뒤 끝낸다.
                    if changed.is_err() || *shutdown.borrow() {
                        shutting_down = true;
                    }
                }
            }
        }
        Ok::<(), String>(())
    }
    .await;

    let _ = sink.close().await;
    // 정리: 이 접속의 방송과 시청 세션을 끝낸다.
    let mut state = shared.lock();
    state.conns.remove(&conn_id);
    if state
        .broadcasts
        .get(&install_id)
        .is_some_and(|b| b.conn == conn_id)
    {
        shared.end_broadcast(&mut state, install_id);
    }
    let mine: Vec<u64> = state
        .sessions
        .iter()
        .filter(|(_, s)| s.conn == conn_id)
        .map(|(id, _)| *id)
        .collect();
    for s in mine {
        shared.remove_session(&mut state, s);
    }
    shared.update_stats(&state);
    info!(conn = conn_id, "접속 끊김");
    result
}

async fn next_msg<S>(stream: &mut S) -> Result<ClientMsg, String>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match stream.next().await {
            None => return Err("연결 닫힘".into()),
            Some(Err(e)) => return Err(e.to_string()),
            Some(Ok(Message::Text(text))) => {
                return serde_json::from_str(&text).map_err(|e| format!("메시지 형식 오류: {e}"));
            }
            Some(Ok(Message::Close(_))) => return Err("연결 닫힘".into()),
            Some(Ok(_)) => {}
        }
    }
}

/// 클라이언트에게 알려 줄 WebRTC 후보 IP.
fn candidate_ip(connected_ip: Option<IpAddr>, local: SocketAddr, peer: SocketAddr) -> IpAddr {
    let mut ip = connected_ip
        .filter(|ip| !ip.is_unspecified() && !ip.is_multicast())
        .unwrap_or_else(|| local.ip());
    if let IpAddr::V6(v6) = ip
        && let Some(v4) = v6.to_ipv4_mapped()
    {
        ip = IpAddr::V4(v4);
    }
    // 같은 PC에서 루프백으로 접속한 경우: WebView2(Chromium)는 루프백 후보를 쓰지 않을 수 있으므로
    // 이 PC의 LAN 주소를 알려 준다.
    if ip.is_loopback()
        && peer.ip().is_loopback()
        && let Some(lan) = dongsi_net::ifaces::primary_lan_ip()
    {
        return IpAddr::V4(lan);
    }
    ip
}

/// 공인 주소(이 기계의 주소가 아님)로 접속했으면 외부 포트(UPnP 또는 공개 포트 설정)를 알려 준다.
fn media_port(shared: &Shared, candidate_ip: IpAddr, peer_ip: IpAddr) -> u16 {
    pick_media_port(
        shared.external_port.load(Ordering::Relaxed),
        shared.port,
        candidate_ip,
        peer_ip,
        dongsi_net::ifaces::is_own_ip,
    )
}

/// `media_port`의 판단. `is_own`은 "이 기계의 주소인지"(시험에서 바꿔 끼운다).
///
/// 후보 IP가 루프백인데 TCP 상대(`peer_ip`)가 루프백이 아니면 프록시(예: docker-proxy)나 NAT를 거쳐
/// 들어온 접속이다. 서버는 루프백을 "이 기계"로 보지만 클라이언트는 프록시의 공개 포트로 접속했으므로
/// 외부 포트를 알려 준다.
fn pick_media_port(
    external: u16,
    internal: u16,
    candidate_ip: IpAddr,
    peer_ip: IpAddr,
    is_own: impl Fn(IpAddr) -> bool,
) -> u16 {
    if external == 0 {
        return internal;
    }
    let proxied_loopback = candidate_ip.is_loopback() && !peer_ip.to_canonical().is_loopback();
    if proxied_loopback || !is_own(candidate_ip) {
        external
    } else {
        internal
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

struct ConnCtx {
    shared: Arc<Shared>,
    conn_id: u64,
    install_id: InstallId,
    name: String,
    can_publish: bool,
    peer_ip: IpAddr,
    candidate: SocketAddr,
    tx: mpsc::UnboundedSender<ServerMsg>,
}

impl ConnCtx {
    fn error(&self, req: Option<u32>, code: ErrorCode) {
        let _ = self.tx.send(ServerMsg::Error {
            req,
            code,
            message: code.default_message().into(),
        });
    }

    async fn handle(&self, msg: ClientMsg) {
        match msg {
            ClientMsg::Hello { .. } => {}
            ClientMsg::Ping { nonce } => {
                let _ = self.tx.send(ServerMsg::Pong { nonce });
            }
            ClientMsg::Publish {
                req,
                offer,
                info,
                password,
            } => self.publish(req, offer, info, password).await,
            ClientMsg::UpdatePublish { info } => {
                let mut state = self.shared.lock();
                if let Some(b) = state.broadcasts.get_mut(&self.install_id)
                    && b.conn == self.conn_id
                {
                    b.info = info;
                    self.shared.publish_list(&state);
                }
            }
            ClientMsg::Unpublish => {
                let mut state = self.shared.lock();
                if state
                    .broadcasts
                    .get(&self.install_id)
                    .is_some_and(|b| b.conn == self.conn_id)
                {
                    self.shared.end_broadcast(&mut state, self.install_id);
                }
            }
            ClientMsg::Watch {
                req,
                broadcaster,
                password,
                offer,
            } => self.watch(req, broadcaster, password, offer).await,
            ClientMsg::StopWatch { session } => {
                let mut state = self.shared.lock();
                if state
                    .sessions
                    .get(&session)
                    .is_some_and(|s| s.conn == self.conn_id)
                {
                    self.shared.remove_session(&mut state, session);
                }
            }
            ClientMsg::RequestKeyframe { session } => {
                let state = self.shared.lock();
                if let Some(SessionInfo {
                    conn,
                    kind: SessionKind::Viewer(id),
                }) = state.sessions.get(&session)
                    && *conn == self.conn_id
                    && let Some(b) = state.broadcasts.get(id)
                {
                    let _ = self
                        .shared
                        .sfu
                        .send(SfuCmd::RequestKeyframe { broadcast: b.key });
                }
            }
        }
    }

    async fn publish(&self, req: u32, offer: String, info: PublishInfo, password: Option<String>) {
        if !self.can_publish {
            return self.error(Some(req), ErrorCode::NotAllowed);
        }
        if offer.len() > MAX_SDP_BYTES {
            return self.error(Some(req), ErrorCode::BadRequest);
        }
        let password = password.filter(|p| !p.is_empty());
        if password
            .as_ref()
            .is_some_and(|p| p.chars().count() > MAX_PASSWORD_CHARS)
        {
            return self.error(Some(req), ErrorCode::BadRequest);
        }
        let (key, session) = {
            let mut state = self.shared.lock();
            if state.broadcasts.contains_key(&self.install_id) {
                return self.error(Some(req), ErrorCode::AlreadyPublishing);
            }
            if state.broadcasts.len() >= self.shared.config.max_broadcasts {
                return self.error(Some(req), ErrorCode::ServerFull);
            }
            (state.next_id(), state.next_id())
        };

        let answer = match self.sfu_session(session, key, Role::Publisher, offer).await {
            Ok(a) => a,
            Err(e) => {
                warn!("송출 세션 실패: {e}");
                return self.error(Some(req), ErrorCode::BadRequest);
            }
        };

        let mut state = self.shared.lock();
        if state.broadcasts.contains_key(&self.install_id)
            || !state.conns.contains_key(&self.conn_id)
        {
            let _ = self.shared.sfu.send(SfuCmd::Remove { session });
            return self.error(Some(req), ErrorCode::AlreadyPublishing);
        }
        state.sessions.insert(
            session,
            SessionInfo {
                conn: self.conn_id,
                kind: SessionKind::Publisher(self.install_id),
            },
        );
        state.broadcasts.insert(
            self.install_id,
            Broadcast {
                key,
                conn: self.conn_id,
                publisher_session: session,
                name: self.name.clone(),
                info,
                password,
                started_at: now_unix(),
                viewers: HashSet::new(),
            },
        );
        info!(broadcaster = %self.install_id, name = %self.name, "방송 시작");
        let _ = self.tx.send(ServerMsg::Published { req, answer });
        self.shared.publish_list(&state);
    }

    async fn watch(
        &self,
        req: u32,
        broadcaster: InstallId,
        password: Option<String>,
        offer: String,
    ) {
        if offer.len() > MAX_SDP_BYTES {
            return self.error(Some(req), ErrorCode::BadRequest);
        }
        let (key, session) = {
            let mut state = self.shared.lock();
            let Some(b) = state.broadcasts.get(&broadcaster) else {
                return self.error(Some(req), ErrorCode::NotFound);
            };
            let key = b.key;
            let required = b.password.clone();
            let viewers = b.viewers.len();
            if let Some(required) = required {
                let failures = state.password_failures.entry(self.peer_ip).or_default();
                failures.retain(|t| t.elapsed() < Duration::from_secs(60));
                if failures.len() >= PASSWORD_FAILURES_PER_MINUTE {
                    return self.error(Some(req), ErrorCode::RateLimited);
                }
                match password.as_deref() {
                    None | Some("") => {
                        return self.error(Some(req), ErrorCode::PasswordRequired);
                    }
                    Some(p) if !constant_time_eq(p.as_bytes(), required.as_bytes()) => {
                        failures.push(Instant::now());
                        return self.error(Some(req), ErrorCode::WrongPassword);
                    }
                    Some(_) => {}
                }
            }
            if viewers >= self.shared.config.max_viewers_per_broadcast {
                return self.error(Some(req), ErrorCode::ServerFull);
            }
            (key, state.next_id())
        };

        let answer = match self.sfu_session(session, key, Role::Viewer, offer).await {
            Ok(a) => a,
            Err(e) => {
                warn!("시청 세션 실패: {e}");
                return self.error(Some(req), ErrorCode::BadRequest);
            }
        };

        let mut state = self.shared.lock();
        let Some(b) = state
            .broadcasts
            .get_mut(&broadcaster)
            .filter(|b| b.key == key)
        else {
            let _ = self.shared.sfu.send(SfuCmd::Remove { session });
            return self.error(Some(req), ErrorCode::NotFound);
        };
        b.viewers.insert(session);
        state.sessions.insert(
            session,
            SessionInfo {
                conn: self.conn_id,
                kind: SessionKind::Viewer(broadcaster),
            },
        );
        debug!(%broadcaster, session, viewer = %self.name, "시청 시작");
        let _ = self.tx.send(ServerMsg::Watching {
            req,
            session,
            answer,
        });
        self.shared.publish_list(&state);
    }

    async fn sfu_session(
        &self,
        session: u64,
        broadcast: u64,
        role: Role,
        offer: String,
    ) -> Result<String, String> {
        let (reply, rx) = oneshot::channel();
        self.shared
            .sfu
            .send(SfuCmd::AddSession {
                session,
                broadcast,
                role,
                offer,
                candidate: self.candidate,
                reply,
            })
            .map_err(|_| "SFU가 멈췄습니다".to_string())?;
        tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .map_err(|_| "SFU 응답 시간 초과".to_string())?
            .map_err(|_| "SFU 응답 없음".to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn media_port_choice() {
        let own = |ip: IpAddr| ip.is_loopback() || ip == "192.168.0.5".parse::<IpAddr>().unwrap();
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        // 외부 포트가 없으면 항상 내부 포트
        assert_eq!(
            pick_media_port(0, 7884, ip("203.0.113.7"), ip("203.0.113.9"), own),
            7884
        );
        // 공인 주소로 접속 -> 외부 포트
        assert_eq!(
            pick_media_port(9000, 7884, ip("203.0.113.7"), ip("203.0.113.9"), own),
            9000
        );
        // 이 기계의 LAN 주소로 접속 -> 내부 포트(UPnP 내장 서버)
        assert_eq!(
            pick_media_port(9000, 7884, ip("192.168.0.5"), ip("192.168.0.9"), own),
            7884
        );
        // 같은 PC의 루프백 접속 -> 내부 포트
        assert_eq!(
            pick_media_port(9000, 7884, ip("127.0.0.1"), ip("127.0.0.1"), own),
            7884
        );
        assert_eq!(
            pick_media_port(9000, 7884, ip("::1"), ip("::ffff:127.0.0.1"), own),
            7884
        );
        // docker-proxy: 후보는 127.0.0.1, TCP 상대는 브리지 게이트웨이 -> 외부 포트
        assert_eq!(
            pick_media_port(9000, 7884, ip("127.0.0.1"), ip("172.17.0.1"), own),
            9000
        );
        assert_eq!(
            pick_media_port(0, 7884, ip("127.0.0.1"), ip("172.17.0.1"), own),
            7884
        );
    }

    #[test]
    fn candidate_prefers_connected_ip() {
        let local: SocketAddr = "192.168.0.5:7884".parse().unwrap();
        let peer: SocketAddr = "192.168.0.9:5000".parse().unwrap();
        assert_eq!(
            candidate_ip(Some("203.0.113.7".parse().unwrap()), local, peer),
            "203.0.113.7".parse::<IpAddr>().unwrap()
        );
        assert_eq!(candidate_ip(None, local, peer), local.ip());
        let mapped: IpAddr = "::ffff:10.0.0.1".parse().unwrap();
        assert_eq!(
            candidate_ip(Some(mapped), local, peer),
            "10.0.0.1".parse::<IpAddr>().unwrap()
        );
    }
}
