//! dongsi 전용 서버.
//!
//! ```text
//! dongsi-server [--data <폴더>] [--port <번호>] [--name <이름>] [--address <주소>]...
//! ```
//!
//! - 데이터 폴더(기본: 현재 폴더의 `dongsi-data`)에 인증서(`cert.pem`, `key.pem`)와 설정
//!   (`dongsi-server.toml`)을 둔다. 처음 실행하면 만든다. 인증서를 지우면 지문이 바뀌어 클라이언트가
//!   다시 추가해야 한다.
//! - 시작하면 서버 초대 코드를 출력한다. 클라이언트의 "서버 추가"에 붙여 넣는다.
//! - 로그 수준은 `RUST_LOG`로 바꾼다(기본 `info`).

use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use dongsi_core::{DEFAULT_PORT, Host, Invite, ServerAddr};
use dongsi_net::ifaces::{IfaceKind, ipv4_interfaces};
use dongsi_net::tls::Identity;
use dongsi_server::ServerConfig;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

const CONFIG_FILE: &str = "dongsi-server.toml";
const MAX_INVITE_ADDRS: usize = 6;

const DEFAULT_CONFIG: &str = r#"# dongsi 전용 서버 설정

# 목록과 로컬 감지에 보이는 서버 이름
name = "dongsi 서버"

# TCP(시그널링)·UDP(미디어, 로컬 감지) 포트. 둘 다 열어야 한다.
port = 7884

# 초대 코드에 넣을 주소(도메인이나 공인 IP). 포트를 붙이지 않으면 위 port를 쓴다.
# 비우면 이 PC의 LAN·Tailscale 주소만 넣는다.
addresses = []

# 동시에 할 수 있는 방송 수, 방송 하나당 시청자 수
max_broadcasts = 16
max_viewers_per_broadcast = 16

# 같은 LAN·tailnet의 감지 질의에 응답할지
discovery = true
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    name: String,
    port: u16,
    addresses: Vec<String>,
    max_broadcasts: usize,
    max_viewers_per_broadcast: usize,
    discovery: bool,
}

impl Default for FileConfig {
    fn default() -> Self {
        let d = ServerConfig::default();
        Self {
            name: d.name,
            port: DEFAULT_PORT,
            addresses: Vec::new(),
            max_broadcasts: d.max_broadcasts,
            max_viewers_per_broadcast: d.max_viewers_per_broadcast,
            discovery: d.discovery,
        }
    }
}

struct Args {
    data: PathBuf,
    port: Option<u16>,
    name: Option<String>,
    addresses: Vec<String>,
}

fn usage() -> &'static str {
    "사용법: dongsi-server [--data <폴더>] [--port <번호>] [--name <이름>] [--address <주소>]...\n\
     \n\
     --data     인증서와 설정을 둘 폴더(기본: ./dongsi-data)\n\
     --port     포트(설정 파일보다 우선, 기본 7884)\n\
     --name     서버 이름(설정 파일보다 우선)\n\
     --address  초대 코드에 넣을 주소(도메인이나 공인 IP, 여러 번 쓸 수 있음)"
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        data: PathBuf::from("dongsi-data"),
        port: None,
        name: None,
        addresses: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .ok_or_else(|| format!("{flag} 다음에 값이 필요합니다"))
        };
        match arg.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--data" => args.data = PathBuf::from(value("--data")?),
            "--port" => {
                let v = value("--port")?;
                args.port = Some(
                    v.parse()
                        .map_err(|_| format!("포트가 올바르지 않습니다: {v}"))?,
                );
            }
            "--name" => args.name = Some(value("--name")?),
            "--address" => args.addresses.push(value("--address")?),
            other => return Err(format!("알 수 없는 인자: {other}")),
        }
    }
    Ok(args)
}

fn load_config(dir: &Path) -> Result<FileConfig, String> {
    let path = dir.join(CONFIG_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::write(&path, DEFAULT_CONFIG)
                .map_err(|e| format!("{}을 만들지 못했습니다: {e}", path.display()))?;
            info!(path = %path.display(), "기본 설정 파일을 만들었습니다");
            Ok(toml::from_str(DEFAULT_CONFIG).expect("기본 설정"))
        }
        Err(e) => Err(format!("{}을 읽지 못했습니다: {e}", path.display())),
    }
}

/// 초대 코드에 넣을 주소: 설정한 공개 주소, Tailscale, LAN 순서.
fn invite_addrs(configured: &[String], port: u16) -> Vec<ServerAddr> {
    let mut out: Vec<ServerAddr> = Vec::new();
    for a in configured {
        match ServerAddr::parse_with_default_port(a, port) {
            Ok(addr) => out.push(addr),
            Err(e) => warn!("주소를 무시합니다({a}): {e}"),
        }
    }
    let mut ifaces = ipv4_interfaces();
    ifaces.sort_by_key(|i| match i.kind {
        IfaceKind::Tailscale => 0,
        IfaceKind::Lan => 1,
        _ => 2,
    });
    for i in ifaces {
        if matches!(i.kind, IfaceKind::Tailscale | IfaceKind::Lan) {
            out.push(ServerAddr::new(Host::Ip(IpAddr::V4(i.ip)), port));
        }
    }
    if out.is_empty() {
        out.push(ServerAddr::new(
            Host::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            port,
        ));
    }
    let mut seen = Vec::new();
    out.retain(|a| {
        let key = a.to_string();
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
    out.truncate(MAX_INVITE_ADDRS);
    out
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("{msg}\n");
            }
            eprintln!("{}", usage());
            return if msg.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(2)
            };
        }
    };

    match run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!("{e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<(), String> {
    std::fs::create_dir_all(&args.data)
        .map_err(|e| format!("{}을 만들지 못했습니다: {e}", args.data.display()))?;
    let mut file = load_config(&args.data)?;
    if let Some(p) = args.port {
        file.port = p;
    }
    if let Some(n) = args.name {
        file.name = n;
    }
    file.addresses.extend(args.addresses);
    let name = dongsi_core::proto::sanitize_display_name(&file.name)
        .ok_or_else(|| "서버 이름이 비어 있습니다".to_string())?;

    let identity = Identity::load_or_generate(&args.data).map_err(|e| e.to_string())?;
    let server = dongsi_server::start(
        ServerConfig {
            name: name.clone(),
            port: file.port,
            max_broadcasts: file.max_broadcasts.max(1),
            max_viewers_per_broadcast: file.max_viewers_per_broadcast.max(1),
            discovery: file.discovery,
            ..Default::default()
        },
        &identity,
    )
    .await
    .map_err(|e| e.to_string())?;

    let addrs = invite_addrs(&file.addresses, server.port());
    let invite = Invite::for_server(addrs.clone(), server.fingerprint(), Some(name.clone()))
        .map_err(|e| format!("초대 코드를 만들지 못했습니다: {e}"))?;
    println!();
    println!("  dongsi 서버 \"{name}\"  포트 {} (TCP·UDP)", server.port());
    println!(
        "  주소: {}",
        addrs
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
    if file.addresses.is_empty() {
        println!(
            "  인터넷으로 받으려면 --address <도메인 또는 공인 IP>를 주고, 공유기에서 TCP·UDP {} 포트를 열어 주세요.",
            server.port()
        );
        println!();
    }

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| format!("종료 신호를 기다리지 못했습니다: {e}"))?;
    info!("종료합니다");
    server.shutdown().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses() {
        let c: FileConfig = toml::from_str(DEFAULT_CONFIG).unwrap();
        assert_eq!(c.port, DEFAULT_PORT);
        assert!(c.addresses.is_empty());
        assert!(c.discovery);
    }

    #[test]
    fn invite_addrs_keep_configured_first() {
        let addrs = invite_addrs(&["example.com".into(), "203.0.113.7:9000".into()], 7884);
        assert_eq!(addrs[0].to_string(), "example.com");
        assert_eq!(addrs[0].port(), 7884);
        assert_eq!(addrs[1].to_string(), "203.0.113.7:9000");
        assert!(addrs.len() <= MAX_INVITE_ADDRS);
    }
}
