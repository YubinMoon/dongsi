//! dongsi 서버 코어.
//!
//! 전용 서버(`apps/server`)와 앱 내장 서버(로컬 랜·PC 직접 호스팅)가 같은 코드를 쓴다.
//!
//! - TCP(기본 7884): TLS 위 WebSocket 시그널링([`dongsi_core::proto`])
//! - UDP(같은 포트): WebRTC 미디어(SFU)와 로컬 감지 응답

mod discovery;
mod metrics;
mod sfu;
mod signaling;
mod simulcast;

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, RwLock};

use dongsi_core::DEFAULT_PORT;
use dongsi_core::Fingerprint;
use dongsi_core::discovery::DiscoveryResponse;
use dongsi_net::tls::Identity;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::info;

pub use metrics::Metrics;
pub use signaling::ServerStats;

/// 서버 설정.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// 목록·감지에 보이는 서버 이름.
    pub name: String,
    pub bind_ip: IpAddr,
    /// TCP·UDP 포트. 0이면 빈 포트를 고른다(시험용).
    pub port: u16,
    /// 사용 중이면 빈 포트로 대신한다(내장 서버).
    pub fallback_port: bool,
    /// UPnP로 받은 외부 포트가 내부 포트와 다를 때. 공인 주소로 접속한 클라이언트에게 이 포트를 알린다.
    pub external_port: Option<u16>,
    /// 내장 서버: 이 토큰을 보낸 접속자(호스트)만 방송할 수 있다. `None`이면 누구나 방송한다(전용 서버).
    pub host_token: Option<String>,
    pub max_broadcasts: usize,
    pub max_viewers_per_broadcast: usize,
    /// 로컬 감지 질의에 응답할지.
    pub discovery: bool,
    /// 시청자 회선에 맞춤(D-081): 시청자 세션에 BWE를 켜고 가장 느린 시청자의 추정값을 방송자에게 REMB로 보낸다.
    /// 내장 서버만 켠다(시청자들이 방송자 업로드를 나눠 쓴다). 전용 서버는 끈다.
    pub adapt_to_viewers: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            name: "dongsi 서버".into(),
            bind_ip: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: DEFAULT_PORT,
            fallback_port: false,
            external_port: None,
            host_token: None,
            max_broadcasts: 16,
            max_viewers_per_broadcast: 16,
            discovery: true,
            adapt_to_viewers: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("포트 {port}을 열지 못했습니다: {source}")]
    Bind {
        port: u16,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Net(#[from] dongsi_net::NetError),
}

/// 실행 중인 서버. [`RunningServer::shutdown`]을 부르거나 버리면 닫힌다.
pub struct RunningServer {
    port: u16,
    fingerprint: Fingerprint,
    shutdown_tx: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
    stats: Arc<RwLock<ServerStats>>,
    metrics: Arc<Metrics>,
    external_port: Arc<AtomicU16>,
}

impl RunningServer {
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }

    pub fn stats(&self) -> ServerStats {
        self.stats.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Prometheus 지표(D-083). 전용 서버가 `GET /metrics`로 내보낸다.
    pub fn metrics(&self) -> Arc<Metrics> {
        self.metrics.clone()
    }

    /// UPnP로 받은 외부 포트를 알린다(서버를 연 뒤에 매핑하므로 나중에 정한다). 이후 공인 주소로
    /// 접속한 클라이언트에게 이 포트를 미디어 후보로 준다.
    pub fn set_external_port(&self, port: Option<u16>) {
        self.external_port
            .store(port.unwrap_or(0), Ordering::Relaxed);
    }

    /// 접속자에게 닫힘을 알리고 끝날 때까지 기다린다.
    pub async fn shutdown(mut self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// 서버를 시작한다. 현재 tokio 런타임에서 돈다.
pub async fn start(
    config: ServerConfig,
    identity: &Identity,
) -> Result<RunningServer, ServerError> {
    let (listener, udp) = bind_pair(config.bind_ip, config.port, config.fallback_port).await?;
    let port = listener.local_addr()?.port();
    let tls = identity.server_config()?;
    let fingerprint = identity.fingerprint();

    let discovery = config.discovery.then(|| {
        Arc::new(RwLock::new(DiscoveryResponse {
            nonce: 0,
            fingerprint,
            name: config.name.clone(),
            port,
            host_only: config.host_token.is_some(),
            broadcasts: Vec::new(),
        }))
    });

    let stats = Arc::new(RwLock::new(ServerStats::default()));
    let metrics = Arc::new(Metrics::new(config.name.clone(), stats.clone()));
    let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();
    let sfu = sfu::spawn(
        udp,
        discovery.clone(),
        events_tx,
        metrics.clone(),
        config.adapt_to_viewers,
    )?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let external_port = Arc::new(AtomicU16::new(config.external_port.unwrap_or(0)));

    info!(port, fingerprint = %fingerprint.to_base64url(), name = %config.name, "서버 시작");
    let task = tokio::spawn(signaling::run(
        config,
        port,
        listener,
        tls,
        sfu,
        events_rx,
        discovery,
        stats.clone(),
        metrics.clone(),
        external_port.clone(),
        shutdown_rx,
    ));

    Ok(RunningServer {
        port,
        fingerprint,
        shutdown_tx,
        task: Some(task),
        stats,
        metrics,
        external_port,
    })
}

/// 같은 번호의 TCP·UDP 포트를 연다.
async fn bind_pair(
    ip: IpAddr,
    port: u16,
    fallback: bool,
) -> Result<(TcpListener, UdpSocket), ServerError> {
    match try_bind(ip, port).await {
        Ok(pair) => return Ok(pair),
        Err(e) if !fallback || port == 0 => {
            if port != 0 {
                return Err(ServerError::Bind { port, source: e });
            }
        }
        Err(_) => {}
    }
    // 빈 포트: UDP를 먼저 아무 번호로 열고 같은 번호로 TCP를 연다.
    let mut last = None;
    for _ in 0..20 {
        let udp = UdpSocket::bind(SocketAddr::new(ip, 0))?;
        let p = udp.local_addr()?.port();
        match TcpListener::bind(SocketAddr::new(ip, p)).await {
            Ok(tcp) => return Ok((tcp, udp)),
            Err(e) => last = Some(e),
        }
    }
    Err(ServerError::Bind {
        port,
        source: last.unwrap_or_else(|| std::io::Error::other("빈 포트를 찾지 못했습니다")),
    })
}

async fn try_bind(ip: IpAddr, port: u16) -> std::io::Result<(TcpListener, UdpSocket)> {
    let tcp = TcpListener::bind(SocketAddr::new(ip, port)).await?;
    let actual = tcp.local_addr()?.port();
    let udp = UdpSocket::bind(SocketAddr::new(ip, actual))?;
    Ok((tcp, udp))
}
