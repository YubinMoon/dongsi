//! 서버 연결 관리: 서버마다 시그널링 연결 하나를 열어 두고 방송 목록을 받는다.
//!
//! - 키: 저장한 서버는 서버 ID, 로컬 감지로 찾은 서버는 `local-<지문>`, 내장 서버는 `host`.
//! - 서버가 보내는 변화(방송 목록, 방송 종료, 시청자 수, 연결 끊김)는 [`HubHost::emit`]으로 알린다.
//!   화면은 이벤트를 받으면 필요한 명령을 다시 불러 최신 상태를 가져온다.
//! - [`Hub`]는 Tauri를 직접 쓰지 않는다. 설치 ID·저장한 서버·이벤트 전달은 [`HubHost`]가 맡아서,
//!   시험에서는 Tauri 없이 실제 서버에 접속해 볼 수 있다.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use dongsi_core::proto::{BroadcastInfo, ClientMsg, ErrorCode, PROTOCOL_VERSION, ServerMsg};
use dongsi_core::{Fingerprint, InstallId, ServerAddr, ServerId};
use dongsi_net::NetError;
use dongsi_net::signal::{self, SignalSender};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

use crate::commands::{AppState, FingerprintView};
use crate::error::{CmdError, CmdResult};

/// 내장 서버(내 방송)의 허브 키.
pub const HOST_KEY: &str = "host";

const WELCOME_TIMEOUT: Duration = Duration::from_secs(8);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 연결이 서버 쪽에서 끊겼을 때의 기본 이유.
pub const REASON_LOST: &str = "서버와 연결이 끊어졌습니다";
/// 사용자가 연결을 끊었을 때.
pub const REASON_USER: &str = "연결을 끊었습니다";
/// 서버를 목록에서 삭제했을 때.
pub const REASON_REMOVED: &str = "서버를 삭제했습니다";
/// 방송을 끝내면서 내장 서버 연결을 닫을 때.
pub const REASON_BROADCAST_ENDED: &str = "방송을 끝냈습니다";

/// 접속 대상.
#[derive(Debug, Clone)]
pub struct Target {
    pub addrs: Vec<ServerAddr>,
    /// `None`이면 처음 받은 지문을 믿는다(TOFU). 저장한 서버면 그 지문을 기억한다.
    pub fingerprint: Option<Fingerprint>,
    pub saved: Option<ServerId>,
    /// 내장 서버에서 방송하려면 필요한 토큰.
    pub host_token: Option<String>,
}

/// 허브가 화면(또는 시험)에 알리는 변화.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubEvent {
    /// 방송 목록이 바뀌었다.
    ServerChanged {
        key: String,
    },
    BroadcastEnded {
        key: String,
        broadcaster_id: String,
    },
    ViewerCount {
        key: String,
        count: u32,
    },
    /// 연결이 닫혔다. 같은 연결에 대해 한 번만 온다.
    ServerClosed {
        key: String,
        reason: String,
    },
}

/// 허브가 바깥(앱)에서 얻는 것들.
pub trait HubHost: Send + Sync + 'static {
    /// Hello에 넣을 설치 ID와 표시 이름.
    fn identity(&self) -> (InstallId, String);
    /// 저장한 서버의 접속 대상. 없으면 `None`.
    fn saved_target(&self, id: ServerId) -> Option<Target>;
    /// 첫 접속(TOFU)에서 받은 지문을 저장한다.
    fn confirm_fingerprint(&self, id: ServerId, fingerprint: Fingerprint);
    fn emit(&self, event: HubEvent);
}

/// 열린 서버 연결.
pub struct ServerConn {
    pub key: String,
    sender: SignalSender,
    host: Arc<dyn HubHost>,
    pub connected_ip: IpAddr,
    pub fingerprint: Fingerprint,
    pub server_name: String,
    pub can_publish: bool,
    pub host_only: bool,
    broadcasts: Mutex<Vec<BroadcastInfo>>,
    pending: Mutex<HashMap<u32, oneshot::Sender<ServerMsg>>>,
    next_req: AtomicU32,
    /// 닫힘을 이미 알렸는지도 겸한다(사용자 끊기와 서버 쪽 끊김 중 먼저 온 것 하나만 알린다).
    closed: Arc<AtomicBool>,
    /// 닫힌 이유(`ServerClosed`로 알린 것과 같은 값). `closed`를 올릴 때 같은 잠금 안에서 넣는다.
    close_reason: Arc<Mutex<Option<String>>>,
    pub viewer_count: AtomicU32,
}

/// 닫힘을 한 번만 표시하고 이유를 남긴다. 이번 호출이 처음이면 true.
fn mark_closed(closed: &AtomicBool, slot: &Mutex<Option<String>>, reason: &str) -> bool {
    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    if closed.swap(true, Ordering::AcqRel) {
        return false;
    }
    *guard = Some(reason.to_owned());
    true
}

impl ServerConn {
    pub fn broadcasts(&self) -> Vec<BroadcastInfo> {
        self.broadcasts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed) || self.sender.is_closed()
    }

    /// 닫힌 이유(서버가 알려 준 이유나 `REASON_*`). 닫힘을 허브가 아직 못 알렸으면 `None`.
    pub fn close_reason(&self) -> Option<String> {
        self.close_reason
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn send(&self, msg: ClientMsg) -> CmdResult<()> {
        self.sender
            .send(msg)
            .map_err(|_| CmdError::new(REASON_LOST))
    }

    /// 요청을 보내고 같은 `req`의 응답(또는 오류)을 기다린다.
    pub async fn request(&self, build: impl FnOnce(u32) -> ClientMsg) -> CmdResult<ServerMsg> {
        let req = self.next_req.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(req, tx);
        self.send(build(req))?;
        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(msg)) => Ok(msg),
            Ok(Err(_)) => Err(CmdError::new(REASON_LOST)),
            Err(_) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&req);
                Err(CmdError::new("서버가 응답하지 않습니다"))
            }
        }
    }

    /// 이쪽에서 연결을 닫는다. 이미 닫힌(닫힘을 알린) 연결이면 아무것도 하지 않는다.
    fn close(&self, reason: &str) {
        if !mark_closed(&self.closed, &self.close_reason, reason) {
            return;
        }
        // 기다리던 요청은 모두 실패한다(보내는 쪽을 버리면 받는 쪽이 오류를 받는다).
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.sender.close();
        self.host.emit(HubEvent::ServerClosed {
            key: self.key.clone(),
            reason: reason.to_owned(),
        });
    }
}

type Slot = Arc<tokio::sync::Mutex<Option<Arc<ServerConn>>>>;

pub struct Hub {
    host: Arc<dyn HubHost>,
    slots: Mutex<HashMap<String, Slot>>,
    /// 저장하지 않은 대상(로컬 감지, 내장 서버).
    targets: Mutex<HashMap<String, Target>>,
    /// 저장한 서버를 로컬 감지로 찾았을 때 그 감지된 주소(키: 서버 ID). 메모리에만 두고, 접속할 때
    /// 저장한 주소 앞에 붙인다. 저장한 서버 목록은 바꾸지 않는다.
    preferred: Mutex<HashMap<String, ServerAddr>>,
    /// 접속한 순서대로 둔 열린 연결(접속하는 중인 것은 들어 있지 않다).
    open: Mutex<Vec<Weak<ServerConn>>>,
}

impl Hub {
    pub fn new(host: Arc<dyn HubHost>) -> Self {
        Self {
            host,
            slots: Mutex::default(),
            targets: Mutex::default(),
            preferred: Mutex::default(),
            open: Mutex::default(),
        }
    }

    pub fn set_target(&self, key: &str, target: Target) {
        self.targets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_owned(), target);
    }

    /// 저장한 서버(`key`는 서버 ID)에 접속할 때 먼저 시도할 주소. 로컬 감지로 찾은 현재 주소를 넣는다.
    pub fn set_preferred_addr(&self, key: &str, addr: ServerAddr) {
        self.preferred
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_owned(), addr);
    }

    /// 등록한 대상(`set_target`)과 먼저 시도할 주소를 지운다. 서버를 삭제하거나 내장 서버를 닫을 때 쓴다.
    pub fn remove_target(&self, key: &str) {
        self.targets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
        self.preferred
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
    }

    /// `set_target`으로 등록한 대상(로컬 감지·내장 서버). 저장한 서버는 없다.
    pub fn registered_target(&self, key: &str) -> Option<Target> {
        self.targets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned()
    }

    fn target(&self, key: &str) -> CmdResult<Target> {
        if let Some(t) = self
            .targets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
        {
            return Ok(t.clone());
        }
        let id: ServerId = key
            .parse()
            .map_err(|_| CmdError::new("서버를 찾을 수 없습니다"))?;
        let mut target = self
            .host
            .saved_target(id)
            .ok_or_else(|| CmdError::new("서버를 찾을 수 없습니다"))?;
        if let Some(addr) = self
            .preferred
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
        {
            target.addrs.retain(|a| a != addr);
            target.addrs.insert(0, addr.clone());
        }
        Ok(target)
    }

    fn slot(&self, key: &str) -> Slot {
        self.slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(key.to_owned())
            .or_default()
            .clone()
    }

    /// 열린 연결을 돌려주고, 없으면 접속한다.
    pub async fn connect(&self, key: &str) -> CmdResult<Arc<ServerConn>> {
        let slot = self.slot(key);
        let mut guard = slot.lock().await;
        if let Some(conn) = guard.as_ref()
            && !conn.is_closed()
        {
            return Ok(conn.clone());
        }
        let target = self.target(key)?;
        let conn = self.open(key, target).await?;
        {
            let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
            open.retain(|w| w.upgrade().is_some_and(|c| !c.is_closed() && c.key != key));
            open.push(Arc::downgrade(&conn));
        }
        *guard = Some(conn.clone());
        Ok(conn)
    }

    /// 이미 열린 연결만 돌려준다.
    pub async fn get(&self, key: &str) -> Option<Arc<ServerConn>> {
        let slot = self.slot(key);
        let guard = slot.lock().await;
        guard.as_ref().filter(|c| !c.is_closed()).cloned()
    }

    /// 지금 열려 있는 연결의 키를 접속한 순서대로 돌려준다. 내장 서버(`host`)와 접속하는 중인 것은 뺀다.
    pub fn connected_keys(&self) -> Vec<String> {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        open.retain(|w| w.upgrade().is_some_and(|c| !c.is_closed()));
        open.iter()
            .filter_map(Weak::upgrade)
            .map(|c| c.key.clone())
            .filter(|k| k != HOST_KEY)
            .collect()
    }

    /// 그 키의 열린 연결을 닫고 `ServerClosed`를 한 번 알린다. 열린 연결이 없으면 아무 일도 없다.
    ///
    /// `reason`은 화면에 보이는 이유(예: [`REASON_USER`]). 로컬 감지 대상(`local-…`)은 지우지 않는다.
    pub fn disconnect(&self, key: &str, reason: &str) {
        let conns: Vec<Arc<ServerConn>> = {
            let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
            let mut found = Vec::new();
            open.retain(|w| match w.upgrade() {
                Some(c) if c.key == key => {
                    found.push(c);
                    false
                }
                Some(_) => true,
                None => false,
            });
            found
        };
        for conn in conns {
            conn.close(reason);
        }
        // 슬롯에 남은 닫힌 연결은 치운다(접속하는 중이라 잠겨 있으면 다음 접속이 덮어쓴다).
        let slot = self
            .slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned();
        if let Some(slot) = slot
            && let Ok(mut guard) = slot.try_lock()
            && guard.as_ref().is_some_and(|c| c.is_closed())
        {
            guard.take();
        }
    }

    async fn open(&self, key: &str, target: Target) -> CmdResult<Arc<ServerConn>> {
        let mut conn = signal::connect_any(&target.addrs, target.fingerprint)
            .await
            .map_err(net_error)?;
        let (install_id, display_name) = self.host.identity();
        conn.sender
            .send(ClientMsg::Hello {
                protocol: PROTOCOL_VERSION,
                app_version: env!("CARGO_PKG_VERSION").into(),
                install_id,
                display_name,
                connected_ip: Some(signal::connected_ip(&conn)),
                host_token: target.host_token.clone(),
            })
            .map_err(net_error)?;
        let first = tokio::time::timeout(WELCOME_TIMEOUT, conn.receiver.recv())
            .await
            .map_err(|_| CmdError::new("서버가 응답하지 않습니다"))?
            .ok_or_else(|| CmdError::new("서버가 연결을 끊었습니다"))?;
        let (server_name, can_publish, host_only, broadcasts) = match first {
            ServerMsg::Welcome {
                server_name,
                can_publish,
                host_only,
                broadcasts,
                ..
            } => (server_name, can_publish, host_only, broadcasts),
            ServerMsg::Error { code, message, .. } => {
                conn.sender.close();
                return Err(match code {
                    ErrorCode::ProtocolMismatch => CmdError::new(
                        "서버와 앱의 버전이 맞지 않습니다. 앱이나 서버를 업데이트해 주세요.",
                    ),
                    _ => CmdError::new(message),
                });
            }
            _ => {
                conn.sender.close();
                return Err(CmdError::new("서버 응답이 올바르지 않습니다"));
            }
        };

        // 첫 접속: 받은 지문을 기억한다(TOFU, D-022).
        if let Some(id) = target.saved
            && target.fingerprint.is_none()
        {
            self.host.confirm_fingerprint(id, conn.fingerprint);
        }

        info!(key, server = %server_name, peer = %conn.peer, "서버 연결");
        let closed = Arc::new(AtomicBool::new(false));
        let close_reason = Arc::new(Mutex::new(None));
        let sc = Arc::new(ServerConn {
            key: key.to_owned(),
            sender: conn.sender.clone(),
            host: self.host.clone(),
            connected_ip: signal::connected_ip(&conn),
            fingerprint: conn.fingerprint,
            server_name,
            can_publish,
            host_only,
            broadcasts: Mutex::new(broadcasts),
            pending: Mutex::new(HashMap::new()),
            next_req: AtomicU32::new(0),
            closed: closed.clone(),
            close_reason: close_reason.clone(),
            viewer_count: AtomicU32::new(0),
        });

        let weak = Arc::downgrade(&sc);
        let host = self.host.clone();
        let key = key.to_owned();
        let mut receiver = conn.receiver;
        tokio::spawn(async move {
            let mut reason = REASON_LOST.to_string();
            while let Some(msg) = receiver.recv().await {
                let Some(sc) = weak.upgrade() else { break };
                handle_server_msg(&*host, &sc, msg, &mut reason);
            }
            if let Some(sc) = weak.upgrade() {
                sc.pending.lock().unwrap_or_else(|e| e.into_inner()).clear();
            }
            // 이쪽에서 먼저 닫아 이미 알렸다면 다시 알리지 않는다.
            if !mark_closed(&closed, &close_reason, &reason) {
                debug!(key, "서버 연결 종료(이미 알림)");
                return;
            }
            debug!(key, "서버 연결 종료: {reason}");
            host.emit(HubEvent::ServerClosed { key, reason });
        });
        Ok(sc)
    }
}

fn net_error(e: NetError) -> CmdError {
    match e {
        NetError::FingerprintMismatch { expected, actual } => CmdError::with_detail(
            format!(
                "서버 인증서 지문이 저장된 것과 다릅니다(저장 {}…, 받은 값 {}…). \
                 서버를 다시 설치했다면 목록에서 삭제하고 새 초대 코드로 추가해 주세요.",
                expected.short(),
                actual.short()
            ),
            "fingerprintMismatch",
            FingerprintMismatchDetail {
                stored: expected.into(),
                actual: actual.into(),
            },
        ),
        other => CmdError::new(other.to_string()),
    }
}

/// `fingerprintMismatch` 오류의 `detail`.
#[derive(Serialize)]
struct FingerprintMismatchDetail {
    stored: FingerprintView,
    actual: FingerprintView,
}

fn deliver(sc: &ServerConn, req: u32, msg: ServerMsg) {
    if let Some(tx) = sc
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&req)
    {
        let _ = tx.send(msg);
    }
}

fn handle_server_msg(host: &dyn HubHost, sc: &ServerConn, msg: ServerMsg, reason: &mut String) {
    let key = sc.key.as_str();
    match msg {
        ServerMsg::Published { req, .. } | ServerMsg::Watching { req, .. } => deliver(sc, req, msg),
        ServerMsg::Error { req: Some(req), .. } => deliver(sc, req, msg),
        ServerMsg::Error {
            req: None, message, ..
        } => {
            warn!(key, "서버 오류: {message}");
        }
        ServerMsg::Broadcasts { broadcasts } => {
            *sc.broadcasts.lock().unwrap_or_else(|e| e.into_inner()) = broadcasts;
            host.emit(HubEvent::ServerChanged { key: key.into() });
        }
        ServerMsg::BroadcastEnded { broadcaster } => {
            host.emit(HubEvent::BroadcastEnded {
                key: key.into(),
                broadcaster_id: broadcaster.to_string(),
            });
        }
        ServerMsg::ViewerCount { count } => {
            sc.viewer_count.store(count, Ordering::Relaxed);
            host.emit(HubEvent::ViewerCount {
                key: key.into(),
                count,
            });
        }
        ServerMsg::Closing { reason: r } => *reason = r,
        ServerMsg::Welcome { .. } | ServerMsg::Pong { .. } => {}
    }
}

/// 방송자 ID 문자열을 읽는다.
pub fn parse_broadcaster(id: &str) -> CmdResult<InstallId> {
    id.parse()
        .map_err(|_| CmdError::new("방송자를 찾을 수 없습니다"))
}

// ---------- Tauri 연결 ----------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct KeyEvent<'a> {
    key: &'a str,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BroadcastEndedEvent<'a> {
    key: &'a str,
    broadcaster_id: &'a str,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClosedEvent<'a> {
    key: &'a str,
    reason: &'a str,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ViewerCountEvent<'a> {
    key: &'a str,
    count: u32,
}

/// 앱의 상태 저장소를 읽고 Tauri 이벤트로 알리는 [`HubHost`].
pub struct TauriHubHost {
    app: AppHandle,
}

impl TauriHubHost {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl HubHost for TauriHubHost {
    fn identity(&self) -> (InstallId, String) {
        self.app.state::<AppState>().identity()
    }

    fn saved_target(&self, id: ServerId) -> Option<Target> {
        self.app.state::<AppState>().saved_target(id)
    }

    fn confirm_fingerprint(&self, id: ServerId, fingerprint: Fingerprint) {
        self.app
            .state::<AppState>()
            .confirm_fingerprint(id, fingerprint);
    }

    fn emit(&self, event: HubEvent) {
        let app = &self.app;
        let _ = match &event {
            HubEvent::ServerChanged { key } => app.emit("server-changed", KeyEvent { key }),
            HubEvent::BroadcastEnded {
                key,
                broadcaster_id,
            } => app.emit(
                "broadcast-ended",
                BroadcastEndedEvent {
                    key,
                    broadcaster_id,
                },
            ),
            HubEvent::ViewerCount { key, count } => {
                app.emit("viewer-count", ViewerCountEvent { key, count: *count })
            }
            HubEvent::ServerClosed { key, reason } => {
                app.emit("server-closed", ClosedEvent { key, reason })
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use dongsi_net::tls::Identity;
    use dongsi_server::{RunningServer, ServerConfig};
    use tokio::sync::mpsc;

    use super::*;

    /// 시험용 호스트: 이벤트를 채널로 모으고 지문 저장 호출을 기록한다.
    struct TestHost {
        install_id: InstallId,
        saved: Mutex<HashMap<ServerId, Target>>,
        confirmed: Mutex<Vec<(ServerId, Fingerprint)>>,
        tx: mpsc::UnboundedSender<HubEvent>,
    }

    impl HubHost for TestHost {
        fn identity(&self) -> (InstallId, String) {
            (self.install_id, "시험".into())
        }

        fn saved_target(&self, id: ServerId) -> Option<Target> {
            self.saved.lock().unwrap().get(&id).cloned()
        }

        fn confirm_fingerprint(&self, id: ServerId, fingerprint: Fingerprint) {
            self.confirmed.lock().unwrap().push((id, fingerprint));
        }

        fn emit(&self, event: HubEvent) {
            let _ = self.tx.send(event);
        }
    }

    struct Fixture {
        hub: Hub,
        host: Arc<TestHost>,
        events: mpsc::UnboundedReceiver<HubEvent>,
        server: RunningServer,
    }

    impl Fixture {
        async fn new() -> Self {
            let (tx, events) = mpsc::unbounded_channel();
            let host = Arc::new(TestHost {
                install_id: InstallId::generate(),
                saved: Mutex::default(),
                confirmed: Mutex::default(),
                tx,
            });
            let server = dongsi_server::start(
                ServerConfig {
                    name: "시험 서버".into(),
                    bind_ip: IpAddr::from([127, 0, 0, 1]),
                    port: 0,
                    discovery: false,
                    ..Default::default()
                },
                &Identity::generate().unwrap(),
            )
            .await
            .unwrap();
            let hub = Hub::new(host.clone());
            Self {
                hub,
                host,
                events,
                server,
            }
        }

        fn target(&self, fingerprint: Option<Fingerprint>) -> Target {
            Target {
                addrs: vec![format!("127.0.0.1:{}", self.server.port()).parse().unwrap()],
                fingerprint,
                saved: None,
                host_token: None,
            }
        }

        /// 서버 지문을 아는 대상을 키로 등록한다.
        fn add_target(&self, key: &str) {
            self.hub
                .set_target(key, self.target(Some(self.server.fingerprint())));
        }

        /// 제한 시간 안에 `ServerClosed`가 올 때까지 기다린다(다른 이벤트는 건너뛴다).
        async fn next_closed(&mut self) -> (String, String) {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                let event = tokio::time::timeout(left, self.events.recv())
                    .await
                    .expect("server-closed 이벤트가 오지 않았습니다")
                    .expect("이벤트 채널이 닫혔습니다");
                if let HubEvent::ServerClosed { key, reason } = event {
                    return (key, reason);
                }
            }
        }

        /// `dur` 동안 들어오는 `ServerClosed`를 모두 모은다.
        async fn closed_within(&mut self, dur: Duration) -> Vec<(String, String)> {
            let end = Instant::now() + dur;
            let mut out = Vec::new();
            while let Ok(Some(event)) = tokio::time::timeout(
                end.saturating_duration_since(Instant::now()),
                self.events.recv(),
            )
            .await
            {
                if let HubEvent::ServerClosed { key, reason } = event {
                    out.push((key, reason));
                }
            }
            out
        }

        /// 서버가 본 접속 수가 `n`이 될 때까지 기다린다.
        async fn wait_connections(&self, n: usize) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.server.stats().connections != n {
                assert!(
                    Instant::now() < deadline,
                    "서버의 접속 수가 {n}이 되지 않았습니다(지금 {})",
                    self.server.stats().connections
                );
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    }

    fn json(e: &CmdError) -> serde_json::Value {
        serde_json::to_value(e).unwrap()
    }

    #[tokio::test]
    async fn connected_keys_keep_connect_order_and_skip_host() {
        let f = Fixture::new().await;
        let saved = ServerId::generate();
        f.host
            .saved
            .lock()
            .unwrap()
            .insert(saved, f.target(Some(f.server.fingerprint())));
        f.add_target(HOST_KEY);
        f.add_target("local-abc");

        assert!(f.hub.connected_keys().is_empty());
        f.hub.connect("local-abc").await.unwrap();
        f.hub.connect(HOST_KEY).await.unwrap();
        f.hub.connect(&saved.to_string()).await.unwrap();
        assert_eq!(
            f.hub.connected_keys(),
            vec!["local-abc".to_string(), saved.to_string()]
        );
        // 같은 키로 다시 부르면 같은 연결이고 순서도 그대로다.
        f.hub.connect("local-abc").await.unwrap();
        assert_eq!(f.hub.connected_keys().len(), 2);
        f.wait_connections(3).await;
    }

    #[tokio::test]
    async fn connect_to_unknown_server_fails_and_is_not_listed() {
        let f = Fixture::new().await;
        let err = f.hub.connect("local-nobody").await.err().unwrap();
        assert_eq!(
            json(&err),
            serde_json::json!({ "message": "서버를 찾을 수 없습니다" })
        );
        assert!(f.hub.connected_keys().is_empty());
    }

    #[tokio::test]
    async fn user_disconnect_notifies_once_and_closes_the_socket() {
        let mut f = Fixture::new().await;
        f.add_target("local-abc");
        let conn = f.hub.connect("local-abc").await.unwrap();
        f.wait_connections(1).await;

        f.hub.disconnect("local-abc", REASON_USER);
        assert!(conn.is_closed());
        assert!(f.hub.connected_keys().is_empty());
        assert!(conn.send(ClientMsg::Unpublish).is_err());
        // 이벤트는 바로 한 번 오고, 받는 루프가 끝난 뒤에도 두 번째는 오지 않는다.
        let closed = f.closed_within(Duration::from_millis(1200)).await;
        assert_eq!(
            closed,
            vec![("local-abc".to_string(), "연결을 끊었습니다".to_string())]
        );
        // 서버 쪽에서도 연결이 실제로 닫혔다.
        f.wait_connections(0).await;

        // 이미 닫았으면 아무 일도 없다(멱등).
        f.hub.disconnect("local-abc", REASON_USER);
        f.hub.disconnect("local-other", REASON_USER);
        assert!(f.closed_within(Duration::from_millis(200)).await.is_empty());

        // 대상은 남아 있어서 다시 연결할 수 있고, 목록에 다시 나타난다.
        f.hub.connect("local-abc").await.unwrap();
        assert_eq!(f.hub.connected_keys(), vec!["local-abc".to_string()]);
        f.wait_connections(1).await;
    }

    #[tokio::test]
    async fn disconnect_fails_pending_requests() {
        let f = Fixture::new().await;
        f.add_target("local-abc");
        let conn = f.hub.connect("local-abc").await.unwrap();
        // 서버는 Ping에 req 없는 Pong으로 답하므로 이 요청은 영영 응답을 받지 못한다.
        let waiting = {
            let conn = conn.clone();
            tokio::spawn(async move {
                conn.request(|req| ClientMsg::Ping { nonce: req.into() })
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        f.hub.disconnect("local-abc", REASON_USER);
        let result = tokio::time::timeout(Duration::from_secs(3), waiting)
            .await
            .expect("끊었는데 요청이 계속 기다립니다")
            .unwrap();
        assert_eq!(
            json(&result.err().unwrap()),
            serde_json::json!({ "message": "서버와 연결이 끊어졌습니다" })
        );
    }

    #[tokio::test]
    async fn server_shutdown_notifies_once_with_server_reason() {
        let f = Fixture::new().await;
        f.add_target("local-abc");
        let conn = f.hub.connect("local-abc").await.unwrap();
        assert_eq!(conn.close_reason(), None);
        let Fixture {
            hub,
            host: _host,
            mut events,
            server,
        } = f;
        server.shutdown().await;

        let mut closed = Vec::new();
        let end = Instant::now() + Duration::from_secs(2);
        while let Ok(Some(event)) =
            tokio::time::timeout(end.saturating_duration_since(Instant::now()), events.recv()).await
        {
            if let HubEvent::ServerClosed { key, reason } = event {
                closed.push((key, reason));
            }
        }
        assert_eq!(
            closed,
            vec![("local-abc".to_string(), "서버가 닫혔습니다".to_string())]
        );
        assert!(hub.connected_keys().is_empty());
        // 감시하는 쪽(방송 화면)이 같은 이유를 읽을 수 있다.
        assert!(conn.is_closed());
        assert_eq!(conn.close_reason().as_deref(), Some("서버가 닫혔습니다"));
        // 이미 닫힌 연결을 끊어도 이벤트가 더 나오지 않고 이유도 바뀌지 않는다.
        hub.disconnect("local-abc", REASON_USER);
        assert!(events.try_recv().is_err());
        assert_eq!(conn.close_reason().as_deref(), Some("서버가 닫혔습니다"));
    }

    #[tokio::test]
    async fn fingerprint_mismatch_is_structured() {
        let f = Fixture::new().await;
        let stored = Fingerprint::from_bytes([7; Fingerprint::LEN]);
        let saved = ServerId::generate();
        f.host
            .saved
            .lock()
            .unwrap()
            .insert(saved, f.target(Some(stored)));

        let err = f.hub.connect(&saved.to_string()).await.err().unwrap();
        let v = json(&err);
        let actual = f.server.fingerprint();
        assert_eq!(v["code"], "fingerprintMismatch");
        assert_eq!(
            v["message"],
            format!(
                "서버 인증서 지문이 저장된 것과 다릅니다(저장 {}…, 받은 값 {}…). \
                 서버를 다시 설치했다면 목록에서 삭제하고 새 초대 코드로 추가해 주세요.",
                stored.short(),
                actual.short()
            )
        );
        assert_eq!(v["detail"]["stored"]["full"], stored.to_hex_colon());
        assert_eq!(v["detail"]["stored"]["short"], stored.short());
        assert_eq!(v["detail"]["actual"]["full"], actual.to_hex_colon());
        assert_eq!(v["detail"]["actual"]["short"], actual.short());
        assert!(f.hub.connected_keys().is_empty());
        assert!(f.host.confirmed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn first_connection_confirms_fingerprint() {
        let f = Fixture::new().await;
        let saved = ServerId::generate();
        let mut target = f.target(None);
        target.saved = Some(saved);
        f.host.saved.lock().unwrap().insert(saved, target);

        f.hub.connect(&saved.to_string()).await.unwrap();
        assert_eq!(
            *f.host.confirmed.lock().unwrap(),
            vec![(saved, f.server.fingerprint())]
        );
    }

    #[tokio::test]
    async fn broadcasting_key_cannot_be_disconnected() {
        use crate::broadcast::Broadcaster;

        let mut f = Fixture::new().await;
        f.add_target("local-abc");
        f.add_target(HOST_KEY);
        f.hub.connect("local-abc").await.unwrap();
        f.hub.connect(HOST_KEY).await.unwrap();

        let broadcaster = Broadcaster::default();
        broadcaster.set_busy(Some(HOST_KEY.into()));
        let err = broadcaster
            .disconnect_unless_broadcasting(&f.hub, HOST_KEY)
            .err()
            .unwrap();
        assert_eq!(json(&err)["code"], "broadcasting");
        assert_eq!(
            json(&err)["message"],
            "방송 중인 서버는 연결을 끊을 수 없습니다. 방송을 먼저 끝내 주세요."
        );
        assert!(f.hub.get(HOST_KEY).await.is_some());
        assert!(f.closed_within(Duration::from_millis(200)).await.is_empty());

        // 방송 중이 아닌 다른 서버는 끊을 수 있다.
        broadcaster
            .disconnect_unless_broadcasting(&f.hub, "local-abc")
            .unwrap();
        assert_eq!(f.next_closed().await.1, "연결을 끊었습니다");

        // 방송이 끝나면(키를 비우면) 끊을 수 있다.
        broadcaster.set_busy(None);
        broadcaster
            .disconnect_unless_broadcasting(&f.hub, HOST_KEY)
            .unwrap();
        assert_eq!(f.next_closed().await.0, HOST_KEY);
    }
}
