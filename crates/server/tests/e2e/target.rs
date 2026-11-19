//! 시험 대상 서버: 내부(시험마다 자기 서버) 또는 외부(환경 변수로 지정한 서버).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use dongsi_core::{Fingerprint, Invite, ServerAddr};
use dongsi_net::signal;
use dongsi_net::tls::Identity;
use dongsi_server::{RunningServer, ServerConfig, ServerStats};
use tokio::sync::{OnceCell, RwLock, RwLockReadGuard, RwLockWriteGuard};

pub const SERVER_NAME: &str = "시험 서버";

/// 외부 서버를 같이 쓰는 시험들의 문. 보통 시험은 읽기로, 파괴적 시험(서버 재시작,
/// 비밀번호 횟수 제한)은 쓰기로 잡아 다른 시험이 끝난 뒤 혼자 돈다.
static GATE: RwLock<()> = RwLock::const_new(());
static EXTERNAL: OnceCell<Option<External>> = OnceCell::const_new();

#[allow(dead_code)]
enum Guard {
    Read(RwLockReadGuard<'static, ()>),
    Write(RwLockWriteGuard<'static, ()>),
}

/// 환경 변수로 지정한 외부 서버.
pub struct External {
    pub addr: ServerAddr,
    pub ip: IpAddr,
    pub fp: Fingerprint,
    /// 지문을 환경 변수나 초대 코드로 받았는지(아니면 첫 접속에서 기억한 것: TOFU).
    pub pinned: bool,
    pub destructive: bool,
    pub restart_cmd: Option<String>,
    pub discovery: bool,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

async fn init_external() -> Option<External> {
    let server = env("DONGSI_E2E_SERVER");
    let invite = env("DONGSI_E2E_INVITE");
    if server.is_none() && invite.is_none() {
        return None;
    }
    let invite = invite.map(|code| {
        Invite::decode(&code).unwrap_or_else(|e| panic!("DONGSI_E2E_INVITE를 읽지 못했습니다: {e}"))
    });
    let env_fp = env("DONGSI_E2E_FINGERPRINT").map(|text| {
        Fingerprint::from_base64url(&text)
            .unwrap_or_else(|e| panic!("DONGSI_E2E_FINGERPRINT를 읽지 못했습니다: {e}"))
    });
    let pinned_fp = env_fp.or(invite.as_ref().map(|i| *i.fingerprint()));
    let candidates: Vec<ServerAddr> = match (&server, &invite) {
        (Some(s), _) => vec![
            s.parse()
                .unwrap_or_else(|e| panic!("DONGSI_E2E_SERVER `{s}`를 읽지 못했습니다: {e}")),
        ],
        (None, Some(invite)) => invite.addrs().to_vec(),
        (None, None) => unreachable!(),
    };

    // 주소를 하나씩 시도해서 처음 접속되는 것을 쓴다(초대 코드에는 LAN 주소 등이 여러 개 들어 있을 수 있다).
    let mut errors = Vec::new();
    for addr in &candidates {
        match signal::connect(addr, pinned_fp).await {
            Ok(conn) => {
                return Some(External {
                    addr: addr.clone(),
                    ip: signal::connected_ip(&conn),
                    fp: pinned_fp.unwrap_or(conn.fingerprint),
                    pinned: pinned_fp.is_some(),
                    destructive: env("DONGSI_E2E_DESTRUCTIVE").as_deref() == Some("1"),
                    restart_cmd: env("DONGSI_E2E_RESTART_CMD"),
                    discovery: env("DONGSI_E2E_DISCOVERY").as_deref() != Some("off"),
                });
            }
            Err(e) => errors.push(format!("{addr}: {e}")),
        }
    }
    panic!(
        "외부 서버에 접속하지 못했습니다:\n  {}",
        errors.join("\n  ")
    );
}

async fn external() -> Option<&'static External> {
    EXTERNAL.get_or_init(init_external).await.as_ref()
}

fn local_addr(port: u16) -> ServerAddr {
    format!("127.0.0.1:{port}").parse().unwrap()
}

pub fn default_config() -> ServerConfig {
    ServerConfig {
        name: SERVER_NAME.into(),
        port: 0,
        ..Default::default()
    }
}

struct Internal {
    config: ServerConfig,
    identity: Identity,
    server: Option<RunningServer>,
    /// 인증서를 저장한 임시 폴더(재시작 시험용)
    dir: Option<PathBuf>,
}

/// 시험 하나가 쓰는 서버.
pub struct Target {
    pub addr: ServerAddr,
    pub ip: IpAddr,
    pub fp: Fingerprint,
    internal: Option<Internal>,
    ext: Option<&'static External>,
    _guard: Guard,
}

impl Drop for Target {
    fn drop(&mut self) {
        if let Some(dir) = self.internal.as_ref().and_then(|i| i.dir.as_ref()) {
            // 이 시험이 만든 임시 폴더(인증서 두 개)만 지운다.
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

impl Target {
    /// 보통 시험의 대상: 외부 서버가 지정돼 있으면 그것, 아니면 새 내부 서버.
    pub async fn shared() -> Self {
        if let Some(ext) = external().await {
            return Self::from_external(ext, Guard::Read(GATE.read().await));
        }
        Self::start_internal(default_config(), false).await
    }

    /// 파괴적 시험의 대상. 외부 서버에서는 `DONGSI_E2E_DESTRUCTIVE=1`이 있어야 하고(없으면 `None`),
    /// 다른 시험이 모두 끝난 뒤에 혼자 돈다. `restartable`이면 내부 서버가 인증서를 임시 폴더에 둔다.
    pub async fn exclusive(test: &str, restartable: bool) -> Option<Self> {
        if let Some(ext) = external().await {
            if !ext.destructive {
                eprintln!(
                    "[e2e] 건너뜀: {test}은(는) 파괴적 시험입니다(외부 서버에서는 DONGSI_E2E_DESTRUCTIVE=1이 필요합니다)"
                );
                return None;
            }
            return Some(Self::from_external(ext, Guard::Write(GATE.write().await)));
        }
        Some(Self::start_internal(default_config(), restartable).await)
    }

    /// 내부 서버 전용 시험의 대상(설정을 직접 정한다). 외부 모드에서는 `None`.
    pub async fn internal_only(test: &str, config: ServerConfig) -> Option<Self> {
        if external().await.is_some() {
            eprintln!("[e2e] 건너뜀: {test}은(는) 내부 서버 전용입니다");
            return None;
        }
        Some(Self::start_internal(config, false).await)
    }

    fn from_external(ext: &'static External, guard: Guard) -> Self {
        eprintln!(
            "[e2e] 대상: 외부 {}, 지문 {}… ({})",
            ext.addr,
            ext.fp.short(),
            if ext.pinned { "고정" } else { "TOFU" }
        );
        Self {
            addr: ext.addr.clone(),
            ip: ext.ip,
            fp: ext.fp,
            internal: None,
            ext: Some(ext),
            _guard: guard,
        }
    }

    async fn start_internal(config: ServerConfig, restartable: bool) -> Self {
        let (identity, dir) = if restartable {
            let dir = std::env::temp_dir().join(format!(
                "dongsi-e2e-{}-{:016x}",
                std::process::id(),
                rand::random::<u64>()
            ));
            (Identity::load_or_generate(&dir).unwrap(), Some(dir))
        } else {
            (Identity::generate().unwrap(), None)
        };
        let server = dongsi_server::start(config.clone(), &identity)
            .await
            .unwrap();
        let addr = local_addr(server.port());
        eprintln!("[e2e] 대상: 내부 서버 {addr}");
        let fp = server.fingerprint();
        Self {
            addr,
            ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            fp,
            internal: Some(Internal {
                config,
                identity,
                server: Some(server),
                dir,
            }),
            ext: None,
            _guard: Guard::Read(GATE.read().await),
        }
    }

    pub fn is_external(&self) -> bool {
        self.ext.is_some()
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// 미디어·감지 UDP 주소.
    pub fn udp_addr(&self) -> SocketAddr {
        SocketAddr::new(self.ip, self.port())
    }

    /// 내부 서버의 이름(내부 모드에서만 정해져 있다).
    pub fn internal_name(&self) -> Option<&str> {
        self.internal.as_ref().map(|i| i.config.name.as_str())
    }

    pub fn stats(&self) -> Option<ServerStats> {
        self.internal
            .as_ref()
            .and_then(|i| i.server.as_ref())
            .map(|s| s.stats())
    }

    pub fn discovery_enabled(&self) -> bool {
        self.ext.is_none_or(|e| e.discovery)
    }

    pub fn can_restart(&self) -> bool {
        self.ext.is_none_or(|e| e.restart_cmd.is_some())
    }

    /// 서버를 정상 종료했다가 같은 신원으로 다시 띄운다.
    /// 내부: `shutdown()` 뒤 새 서버(포트는 바뀔 수 있다). 외부: `DONGSI_E2E_RESTART_CMD` 실행.
    pub async fn restart(&mut self) {
        if let Some(internal) = &mut self.internal {
            internal
                .server
                .take()
                .expect("서버가 없습니다")
                .shutdown()
                .await;
            let server = dongsi_server::start(internal.config.clone(), &internal.identity)
                .await
                .unwrap();
            self.addr = local_addr(server.port());
            assert_eq!(
                server.fingerprint(),
                self.fp,
                "재시작 뒤 지문이 바뀌었습니다"
            );
            internal.server = Some(server);
            eprintln!("[e2e] 서버 재시작: {}", self.addr);
        } else if let Some(cmd) = self.ext.and_then(|e| e.restart_cmd.clone()) {
            run_shell(&cmd).await;
        } else {
            panic!("재시작 명령이 없습니다");
        }
    }
}

/// 셸 명령을 실행하고 끝나기를 기다린다(Windows는 `cmd /C`, 그 밖은 `sh -c`).
async fn run_shell(cmd: &str) {
    #[cfg(windows)]
    let mut command = {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").raw_arg(cmd);
        c
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(cmd);
        c
    };
    eprintln!("[e2e] 재시작 명령 실행: {cmd}");
    let started = std::time::Instant::now();
    let out = tokio::time::timeout(
        Duration::from_secs(120),
        command.kill_on_drop(true).output(),
    )
    .await
    .expect("재시작 명령이 120초 안에 끝나지 않았습니다")
    .expect("재시작 명령을 실행하지 못했습니다");
    eprintln!(
        "[e2e] 재시작 명령 끝남({:.1}초, {}): {}{}",
        started.elapsed().as_secs_f64(),
        out.status,
        String::from_utf8_lossy(&out.stdout).trim(),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    assert!(
        out.status.success(),
        "재시작 명령이 실패했습니다: {}",
        out.status
    );
}
