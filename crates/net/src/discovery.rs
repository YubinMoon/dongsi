//! 로컬 방송 감지 클라이언트(D-030).
//!
//! - LAN: 인터페이스마다 서브넷 브로드캐스트 주소로 질의한다.
//! - Tailscale: `tailscale status --json`으로 온라인 피어를 얻어 각 피어에 유니캐스트로 질의한다.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use dongsi_core::discovery::{DiscoveryResponse, decode_response, encode_query};
use dongsi_core::{DEFAULT_PORT, Fingerprint};
use serde::Deserialize;
use tokio::net::UdpSocket;
use tracing::debug;

use crate::ifaces::{IfaceKind, ipv4_interfaces};

/// 감지 경로.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Via {
    Lan,
    Tailscale,
    /// 같은 PC(루프백).
    Local,
}

#[derive(Debug, Clone)]
pub struct Discovered {
    /// 응답이 온 주소(시그널링은 이 IP와 응답의 `port`로 접속).
    pub ip: IpAddr,
    pub via: Via,
    pub response: DiscoveryResponse,
}

impl Discovered {
    pub fn fingerprint(&self) -> Fingerprint {
        self.response.fingerprint
    }

    pub fn signal_addr(&self) -> SocketAddr {
        SocketAddr::new(self.ip, self.response.port)
    }
}

/// 한 번 훑는다. `wait` 동안 응답을 모은다. 같은 서버(지문)가 여러 경로로 응답하면 LAN을 우선한다.
pub async fn scan(port: u16, wait: Duration) -> Vec<Discovered> {
    let sock = match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await {
        Ok(s) => s,
        Err(e) => {
            debug!("감지 소켓을 열지 못했습니다: {e}");
            return Vec::new();
        }
    };
    let _ = sock.set_broadcast(true);
    let nonce: u64 = rand::random();
    let query = encode_query(nonce);

    let mut targets: Vec<(SocketAddr, Via)> = Vec::new();
    for iface in ipv4_interfaces() {
        if iface.kind == IfaceKind::Tailscale {
            continue;
        }
        if let Some(b) = iface.broadcast {
            targets.push((SocketAddr::new(IpAddr::V4(b), port), Via::Lan));
        }
    }
    // 같은 PC의 서버(개발·시험용). 실제로는 앱이 자기 방송을 걸러 낸다.
    targets.push((
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        Via::Local,
    ));
    for ip in tailscale_peers().await {
        targets.push((SocketAddr::new(ip, port), Via::Tailscale));
    }

    for (target, _) in &targets {
        if let Err(e) = sock.send_to(&query, target).await {
            debug!("감지 질의를 보내지 못했습니다({target}): {e}");
        }
    }

    let tailscale_ips: Vec<IpAddr> = targets
        .iter()
        .filter(|(_, v)| *v == Via::Tailscale)
        .map(|(a, _)| a.ip())
        .collect();

    let mut found: HashMap<Fingerprint, Discovered> = HashMap::new();
    let deadline = tokio::time::Instant::now() + wait;
    let mut buf = vec![0u8; 4096];
    loop {
        let recv = tokio::time::timeout_at(deadline, sock.recv_from(&mut buf)).await;
        let Ok(Ok((n, from))) = recv else {
            break;
        };
        let Some(resp) = decode_response(&buf[..n]) else {
            continue;
        };
        if resp.nonce != nonce {
            continue;
        }
        let via = if from.ip().is_loopback() {
            Via::Local
        } else if tailscale_ips.contains(&from.ip())
            || matches!(from.ip(), IpAddr::V4(v4) if crate::ifaces::classify(v4) == IfaceKind::Tailscale)
        {
            Via::Tailscale
        } else {
            Via::Lan
        };
        let entry = Discovered {
            ip: from.ip(),
            via,
            response: resp,
        };
        match found.get(&entry.fingerprint()) {
            Some(prev) if rank(prev.via) <= rank(entry.via) => {}
            _ => {
                found.insert(entry.fingerprint(), entry);
            }
        }
    }
    let mut out: Vec<Discovered> = found.into_values().collect();
    out.sort_by(|a, b| a.response.name.cmp(&b.response.name));
    out
}

/// 경로 우선순위(작을수록 우선).
fn rank(via: Via) -> u8 {
    match via {
        Via::Lan => 0,
        Via::Tailscale => 1,
        Via::Local => 2,
    }
}

pub fn default_port() -> u16 {
    DEFAULT_PORT
}

// ---------------------------------------------------------------------------
// Tailscale 피어

static TS_CACHE: Mutex<Option<(Instant, Vec<IpAddr>)>> = Mutex::new(None);
const TS_CACHE_TTL: Duration = Duration::from_secs(20);

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TsStatus {
    #[serde(default)]
    peer: HashMap<String, TsPeer>,
    #[serde(default)]
    backend_state: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TsPeer {
    #[serde(default, rename = "TailscaleIPs")]
    tailscale_ips: Vec<IpAddr>,
    #[serde(default)]
    online: bool,
}

/// 온라인 Tailscale 피어의 IPv4 주소. Tailscale이 없거나 꺼져 있으면 빈 목록.
pub async fn tailscale_peers() -> Vec<IpAddr> {
    if let Some((at, ips)) = TS_CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
        && at.elapsed() < TS_CACHE_TTL
    {
        return ips.clone();
    }
    let ips = query_tailscale().await.unwrap_or_default();
    *TS_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), ips.clone()));
    ips
}

async fn query_tailscale() -> Option<Vec<IpAddr>> {
    let exe = tailscale_exe()?;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(["status", "--json"]).kill_on_drop(true);
    #[cfg(windows)]
    {
        // 콘솔 창이 뜨지 않게 한다(CREATE_NO_WINDOW).
        cmd.creation_flags(0x0800_0000);
    }
    let out = tokio::time::timeout(Duration::from_secs(3), cmd.output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_tailscale_status(&out.stdout)
}

fn parse_tailscale_status(json: &[u8]) -> Option<Vec<IpAddr>> {
    let status: TsStatus = serde_json::from_slice(json).ok()?;
    if status.backend_state != "Running" {
        return Some(Vec::new());
    }
    let mut ips: Vec<IpAddr> = status
        .peer
        .values()
        .filter(|p| p.online)
        .filter_map(|p| p.tailscale_ips.iter().find(|ip| ip.is_ipv4()).copied())
        .collect();
    ips.sort();
    ips.dedup();
    Some(ips)
}

fn tailscale_exe() -> Option<std::path::PathBuf> {
    let default = std::path::PathBuf::from(r"C:\Program Files\Tailscale\tailscale.exe");
    if default.exists() {
        return Some(default);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|p| p.join("tailscale.exe"))
        .find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status() {
        let json = br#"{
            "BackendState": "Running",
            "Self": {"HostName": "me", "TailscaleIPs": ["100.90.1.1"]},
            "Peer": {
                "nodekey:a": {"HostName": "a", "Online": true, "TailscaleIPs": ["100.64.0.5", "fd7a::5"]},
                "nodekey:b": {"HostName": "b", "Online": false, "TailscaleIPs": ["100.64.0.6"]},
                "nodekey:c": {"HostName": "c", "Online": true, "TailscaleIPs": ["fd7a::7"]}
            }
        }"#;
        let ips = parse_tailscale_status(json).unwrap();
        assert_eq!(ips, vec!["100.64.0.5".parse::<IpAddr>().unwrap()]);

        let stopped = br#"{"BackendState": "Stopped", "Peer": {}}"#;
        assert_eq!(parse_tailscale_status(stopped), Some(vec![]));
        assert_eq!(parse_tailscale_status(b"not json"), None);
    }
}
