//! dongsi 전용 서버.
//!
//! ```text
//! dongsi-server [--data <폴더>] [--port <번호>] [--public-port <번호>] [--name <이름>]
//!               [--address <주소>]... [--no-auto-addresses]
//! ```
//!
//! - 설정 우선순위는 명령줄 > 환경 변수(`DONGSI_*`) > 설정 파일 > 기본값이다. `--help`에 전부 있다.
//! - 데이터 폴더(기본: 현재 폴더의 `dongsi-data`)에 인증서(`cert.pem`, `key.pem`)와 설정
//!   (`dongsi-server.toml`)을 둔다. 처음 실행하면 만든다. 인증서를 지우면 지문이 바뀌어 클라이언트가
//!   다시 추가해야 한다.
//! - 시작하면 서버 초대 코드를 출력한다. 클라이언트의 "서버 추가"에 붙여 넣는다.
//! - SIGINT·SIGTERM(`docker stop`)을 받으면 접속자에게 닫힘을 알리고 종료 코드 0으로 끝난다.
//! - 로그 수준은 `RUST_LOG`로 바꾼다(기본 `info`).

mod addrs;
mod config;

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use dongsi_core::Invite;
use dongsi_net::ifaces::ipv4_interfaces;
use dongsi_net::tls::Identity;
use dongsi_server::ServerConfig;
use tracing::{error, info, warn};

use config::{Cli, EnvConfig, Settings};

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = match config::parse_cli(std::env::args().skip(1)) {
        Ok(cli) => cli,
        Err(msg) => return usage_error(&msg),
    };
    if cli.help {
        println!("{}", config::usage());
        return ExitCode::SUCCESS;
    }
    let env = match config::parse_env(|k| std::env::var(k).ok()) {
        Ok(env) => env,
        Err(msg) => return usage_error(&msg),
    };

    if cli.healthcheck {
        return healthcheck(&cli, &env);
    }

    match run(cli, env).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(RunError::Config(msg)) => usage_error(&msg),
        Err(RunError::Failed(msg)) => {
            error!("{msg}");
            ExitCode::FAILURE
        }
    }
}

/// 설정 오류: 메시지와 사용법을 보이고 종료 코드 2.
fn usage_error(msg: &str) -> ExitCode {
    eprintln!("설정 오류: {msg}\n");
    eprintln!("{}", config::usage());
    ExitCode::from(2)
}

enum RunError {
    /// 설정이 올바르지 않다(종료 코드 2).
    Config(String),
    /// 시작하거나 도는 중 실패했다(종료 코드 1).
    Failed(String),
}

/// `--healthcheck`: 서버가 이 기계의 TCP 포트에서 접속을 받는지만 본다. 설정 파일은 읽기만 한다.
fn healthcheck(cli: &Cli, env: &EnvConfig) -> ExitCode {
    let dir = config::data_dir(cli, env);
    let file = match config::load_file(&dir, false) {
        Ok(f) => f,
        Err(msg) => return usage_error(&msg),
    };
    let port = cli.port.or(env.port).unwrap_or(file.port);
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    match TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{addr}에 접속하지 못했습니다: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli, env: EnvConfig) -> Result<(), RunError> {
    let dir = config::data_dir(&cli, &env);
    std::fs::create_dir_all(&dir)
        .map_err(|e| RunError::Failed(format!("{}을 만들지 못했습니다: {e}", dir.display())))?;
    let file = config::load_file(&dir, true).map_err(RunError::Config)?;
    let settings = Settings::resolve(&cli, &env, file).map_err(RunError::Config)?;

    let identity = Identity::load_or_generate(&dir).map_err(|e| RunError::Failed(e.to_string()))?;
    restrict_key_permissions(&dir);
    let server = dongsi_server::start(
        ServerConfig {
            name: settings.name.clone(),
            port: settings.port,
            external_port: settings.external_port(),
            max_broadcasts: settings.max_broadcasts,
            max_viewers_per_broadcast: settings.max_viewers_per_broadcast,
            discovery: settings.discovery,
            ..Default::default()
        },
        &identity,
    )
    .await
    .map_err(|e| RunError::Failed(e.to_string()))?;

    let port = server.port();
    let public_port = settings.public_port.unwrap_or(port);
    let invite_addrs = addrs::invite_addrs(
        &settings.addresses,
        settings.auto_addresses,
        public_port,
        port,
        ipv4_interfaces(),
    );
    let invite = Invite::for_server(
        invite_addrs.addrs.clone(),
        server.fingerprint(),
        Some(settings.name.clone()),
    )
    .map_err(|e| RunError::Failed(format!("초대 코드를 만들지 못했습니다: {e}")))?;

    println!();
    match settings.external_port() {
        Some(public) => println!(
            "  dongsi 서버 \"{}\"  포트 {port} (TCP·UDP), 공개 포트 {public}",
            settings.name
        ),
        None => println!("  dongsi 서버 \"{}\"  포트 {port} (TCP·UDP)", settings.name),
    }
    println!(
        "  주소: {}",
        invite_addrs
            .addrs
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("  지문: {}", server.fingerprint().to_base64url());
    println!();
    println!("  초대 코드(클라이언트의 \"서버 추가\"에 붙여 넣기):");
    println!("  {}", invite.encode());
    println!();
    if settings.addresses.is_empty() {
        println!(
            "  인터넷으로 받으려면 --address <도메인 또는 공인 IP>(또는 DONGSI_ADDRESSES)를 주고, 공유기에서 TCP·UDP {public_port} 포트를 열어 주세요."
        );
        println!();
    }
    if settings.addresses.is_empty() && is_container() {
        warn!(
            "컨테이너 안에서 실행 중입니다. 브리지 네트워크라면 자동으로 찾은 주소(컨테이너 내부 주소)는 밖에서 닿지 않으니 DONGSI_ADDRESSES에 호스트 주소를 넣어 주세요. (host 네트워크면 무시해도 됩니다)"
        );
    }
    if invite_addrs.loopback_fallback {
        warn!(
            "초대 코드에 넣을 주소가 없어 127.0.0.1을 넣었습니다. 같은 PC에서만 닿습니다. --address 또는 DONGSI_ADDRESSES로 주소를 주세요."
        );
    }

    let signal = wait_for_shutdown()
        .await
        .map_err(|e| RunError::Failed(format!("종료 신호를 기다리지 못했습니다: {e}")))?;
    info!("{signal}를 받아 종료합니다");
    server.shutdown().await;
    Ok(())
}

fn is_container() -> bool {
    config::in_container(|k| std::env::var(k).ok(), Path::new("/.dockerenv").exists())
}

/// SIGINT·SIGTERM(unix), Ctrl+C(Windows) 중 먼저 오는 것을 기다린다.
#[cfg(unix)]
async fn wait_for_shutdown() -> Result<&'static str, std::io::Error> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    Ok(tokio::select! {
        _ = term.recv() => "SIGTERM",
        _ = int.recv() => "SIGINT",
    })
}

#[cfg(not(unix))]
async fn wait_for_shutdown() -> Result<&'static str, std::io::Error> {
    tokio::signal::ctrl_c().await?;
    Ok("Ctrl+C")
}

/// 개인 키는 소유자만 읽게 한다(unix). 볼륨을 다른 권한으로 만든 경우도 바로잡는다.
#[cfg(unix)]
fn restrict_key_permissions(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let key = dir.join("key.pem");
    if let Err(e) = std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)) {
        warn!("{}의 권한을 0600으로 바꾸지 못했습니다: {e}", key.display());
    }
}

#[cfg(not(unix))]
fn restrict_key_permissions(_dir: &Path) {}
