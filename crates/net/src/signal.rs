//! 시그널링 클라이언트: TLS(지문 확인) 위 WebSocket으로 서버와 JSON 메시지를 주고받는다.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use dongsi_core::proto::{ClientMsg, ServerMsg, TLS_SERVER_NAME, WS_PATH};
use dongsi_core::{Fingerprint, Host, ServerAddr};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tracing::{debug, warn};

use crate::NetError;
use crate::tls::{self, client_config};

/// 접속 시도 하나의 제한 시간.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);

/// 이 시간 동안 서버에서 아무것도 오지 않으면 끊긴 것으로 본다.
const IDLE_TIMEOUT: Duration = Duration::from_secs(45);
const PING_INTERVAL: Duration = Duration::from_secs(15);
/// 닫을 때 Close 프레임을 보내거나 서버의 답을 기다리는 최대 시간.
const CLOSE_GRACE: Duration = Duration::from_secs(1);

/// 연결된 시그널링 세션.
pub struct Connection {
    /// 실제로 확인한 서버 인증서 지문.
    pub fingerprint: Fingerprint,
    /// 접속에 성공한 주소.
    pub addr: ServerAddr,
    /// 실제 TCP 상대 주소(도메인이면 해석한 IP).
    pub peer: SocketAddr,
    pub sender: SignalSender,
    pub receiver: mpsc::UnboundedReceiver<ServerMsg>,
}

/// 메시지를 보내는 쪽. 복제해서 여러 곳에서 쓸 수 있다.
#[derive(Clone)]
pub struct SignalSender {
    tx: mpsc::UnboundedSender<ClientMsg>,
    /// `close`를 부르면 true가 된다. 보내기·받기 태스크가 지켜본다.
    close: Arc<watch::Sender<bool>>,
}

impl SignalSender {
    pub fn send(&self, msg: ClientMsg) -> Result<(), NetError> {
        if *self.close.borrow() {
            return Err(NetError::Closed);
        }
        self.tx.send(msg).map_err(|_| NetError::Closed)
    }

    pub fn is_closed(&self) -> bool {
        *self.close.borrow() || self.tx.is_closed()
    }

    /// 연결을 닫는다. 보내기 태스크가 WebSocket Close를 보내고 끝나며, 받기 태스크도 곧 끝나
    /// TCP가 닫힌다(서버가 답하지 않아도 `CLOSE_GRACE` 안에). 이미 보낸 메시지는 Close 전에 나간다.
    /// 닫은 뒤 `send`는 `NetError::Closed`다. 여러 번 불러도 된다.
    pub fn close(&self) {
        self.close.send_replace(true);
    }
}

/// 주소 후보에 동시에 접속을 시도해서 먼저 성공한 연결을 돌려준다.
///
/// `expected`가 있으면 지문이 같은 서버에만 접속한다. 없으면 처음 받은 지문을 그대로 받는다(TOFU).
pub async fn connect_any(
    addrs: &[ServerAddr],
    expected: Option<Fingerprint>,
) -> Result<Connection, NetError> {
    if addrs.is_empty() {
        return Err(NetError::Connect("주소가 없습니다".into()));
    }
    let mut set = JoinSet::new();
    for (i, addr) in addrs.iter().cloned().enumerate() {
        set.spawn(async move {
            // 목록 앞쪽 주소를 조금 먼저 시도한다.
            tokio::time::sleep(Duration::from_millis(150 * i as u64)).await;
            connect(&addr, expected).await
        });
    }
    let mut last_err = None;
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(conn)) => {
                set.abort_all();
                return Ok(conn);
            }
            Ok(Err(e)) => {
                // 지문이 다른 경우는 다른 주소에서 성공할 수 있으므로 계속 기다리되,
                // 사용자에게 보여 줄 오류로는 지문 불일치를 우선한다.
                if matches!(e, NetError::FingerprintMismatch { .. })
                    || !matches!(last_err, Some(NetError::FingerprintMismatch { .. }))
                {
                    last_err = Some(e);
                }
            }
            Err(e) => last_err = Some(NetError::Connect(e.to_string())),
        }
    }
    Err(last_err.unwrap_or_else(|| NetError::Connect("접속하지 못했습니다".into())))
}

/// 주소 하나로 접속한다.
pub async fn connect(
    addr: &ServerAddr,
    expected: Option<Fingerprint>,
) -> Result<Connection, NetError> {
    tokio::time::timeout(CONNECT_TIMEOUT, connect_inner(addr, expected))
        .await
        .map_err(|_| NetError::Timeout(format!("{addr}에 접속하지 못했습니다(시간 초과)")))?
}

async fn connect_inner(
    addr: &ServerAddr,
    expected: Option<Fingerprint>,
) -> Result<Connection, NetError> {
    let tcp = tcp_connect(addr).await?;
    let peer = tcp.peer_addr()?;
    let _ = tcp.set_nodelay(true);

    let (config, observed) = client_config(expected)?;
    let connector = TlsConnector::from(config);
    let tls = match connector.connect(tls::server_name(), tcp).await {
        Ok(tls) => tls,
        Err(e) => {
            if let (Some(expected), Some(actual)) = (expected, observed.get())
                && expected != actual
            {
                return Err(NetError::FingerprintMismatch { expected, actual });
            }
            return Err(NetError::Tls(format!("TLS 연결에 실패했습니다: {e}")));
        }
    };
    let fingerprint = observed
        .get()
        .ok_or_else(|| NetError::Tls("서버 인증서를 받지 못했습니다".into()))?;

    let url = format!("wss://{TLS_SERVER_NAME}{WS_PATH}");
    let mut ws_config = WebSocketConfig::default();
    ws_config.max_message_size = Some(1 << 20);
    ws_config.max_frame_size = Some(1 << 20);
    let (ws, _resp) =
        tokio_tungstenite::client_async_with_config(url.as_str(), tls, Some(ws_config))
            .await
            .map_err(|e| NetError::Connect(format!("WebSocket 연결 실패: {e}")))?;

    let (mut sink, mut stream) = ws.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<ClientMsg>();
    let (in_tx, in_rx) = mpsc::unbounded_channel::<ServerMsg>();
    let close_tx = Arc::new(watch::channel(false).0);
    let mut send_close = close_tx.subscribe();
    let mut recv_close = close_tx.subscribe();

    // 보내기: 앱 → 서버. 주기적으로 Ping을 보내 연결을 유지한다.
    tokio::spawn(async move {
        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.tick().await;
        let mut nonce = 0u64;
        loop {
            let msg = tokio::select! {
                biased;
                _ = closed(&mut send_close) => {
                    // 닫기 전에 이미 큐에 들어간 메시지를 먼저 내보낸다.
                    while let Ok(m) = out_rx.try_recv() {
                        if let Ok(text) = serde_json::to_string(&m)
                            && sink.send(Message::text(text)).await.is_err()
                        {
                            break;
                        }
                    }
                    break;
                }
                msg = out_rx.recv() => match msg {
                    Some(m) => m,
                    None => break,
                },
                _ = ping.tick() => {
                    nonce += 1;
                    ClientMsg::Ping { nonce }
                }
            };
            let text = match serde_json::to_string(&msg) {
                Ok(t) => t,
                Err(e) => {
                    warn!("메시지 직렬화 실패: {e}");
                    continue;
                }
            };
            if sink.send(Message::text(text)).await.is_err() {
                break;
            }
        }
        let _ = tokio::time::timeout(CLOSE_GRACE, sink.close()).await;
    });

    // 받기: 서버 → 앱.
    tokio::spawn(async move {
        loop {
            let next = tokio::select! {
                next = tokio::time::timeout(IDLE_TIMEOUT, stream.next()) => next,
                _ = closed(&mut recv_close) => {
                    // 서버가 Close로 답하면 바로, 아니면 잠깐 기다렸다가 끝낸다.
                    let _ = tokio::time::timeout(CLOSE_GRACE, async {
                        while stream.next().await.is_some() {}
                    })
                    .await;
                    break;
                }
            };
            let msg = match next {
                Err(_) => {
                    debug!("시그널링: 응답이 없어 연결을 닫습니다");
                    break;
                }
                Ok(None) => break,
                Ok(Some(Err(e))) => {
                    debug!("시그널링 수신 오류: {e}");
                    break;
                }
                Ok(Some(Ok(m))) => m,
            };
            match msg {
                Message::Text(text) => match serde_json::from_str::<ServerMsg>(&text) {
                    Ok(ServerMsg::Pong { .. }) => {}
                    Ok(m) => {
                        if in_tx.send(m).is_err() {
                            break;
                        }
                    }
                    Err(e) => warn!("알 수 없는 서버 메시지: {e}"),
                },
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    Ok(Connection {
        fingerprint,
        addr: addr.clone(),
        peer,
        sender: SignalSender {
            tx: out_tx,
            close: close_tx,
        },
        receiver: in_rx,
    })
}

/// `close`가 불릴 때까지 기다린다. 보내는 쪽이 모두 사라져도(닫기를 부르지 않고) 끝나지 않는다.
async fn closed(rx: &mut watch::Receiver<bool>) {
    if rx.wait_for(|c| *c).await.is_err() {
        std::future::pending::<()>().await;
    }
}

async fn tcp_connect(addr: &ServerAddr) -> Result<TcpStream, NetError> {
    let candidates: Vec<SocketAddr> = match addr.host() {
        Host::Ip(ip) => vec![SocketAddr::new(*ip, addr.port())],
        Host::Domain(name) => tokio::net::lookup_host((name.as_str(), addr.port()))
            .await
            .map_err(|e| NetError::Connect(format!("{name}의 주소를 찾지 못했습니다: {e}")))?
            .collect(),
    };
    let mut last = None;
    for sa in candidates {
        match TcpStream::connect(sa).await {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    Err(NetError::Connect(match last {
        Some(e) => format!("{addr}에 접속하지 못했습니다: {e}"),
        None => format!("{addr}의 주소가 없습니다"),
    }))
}

/// 클라이언트가 서버에 알려 줄 "접속에 쓴 서버 IP".
pub fn connected_ip(conn: &Connection) -> IpAddr {
    match conn.peer.ip() {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        ip => ip,
    }
}
