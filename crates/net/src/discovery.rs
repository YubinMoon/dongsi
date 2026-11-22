//! 로컬 방송 감지 클라이언트(D-030).
//!
//! - LAN: 인터페이스마다 서브넷 브로드캐스트 주소로 질의한다(`default_targets`).
//! - 질의는 `wait` 동안 몇 번 다시 보내고, 받기 오류(ICMP port unreachable)는 건너뛴다. 방송이 많은 서버의
//!   응답은 여러 조각으로 와서 모아 합친다(D-039).
//! - 대상은 `scan_targets`에 직접 넘길 수도 있다(시험은 루프백만, LAN 유니캐스트만 등으로 고른다).
//! - Tailscale: `tailscale status --json`으로 온라인 피어를 얻어 각 피어에 유니캐스트로 질의한다.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use dongsi_core::discovery::{DiscoveryResponse, PartAssembler, decode_part, encode_query};
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
    /// 응답 조각(방송이 많은 서버)을 모두 받았는지. `false`면 `response.broadcasts`는 받은 만큼이다.
    pub complete: bool,
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

/// 감지 질의를 보낼 기본 대상: 인터페이스(Tailscale 제외)마다 서브넷 브로드캐스트, 같은 PC(루프백),
/// `tailscale`이면 온라인 Tailscale 피어(유니캐스트).
///
/// 시험은 `tailscale: false`로 불러 tailnet의 다른 PC로 패킷을 보내지 않게 한다.
pub async fn default_targets(port: u16, tailscale: bool) -> Vec<(SocketAddr, Via)> {
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
    if tailscale {
        for ip in tailscale_peers().await {
            targets.push((SocketAddr::new(ip, port), Via::Tailscale));
        }
    }
    targets
}

/// 기본 대상(Tailscale 포함)으로 한 번 훑는다. [`scan_targets`]와 같다.
pub async fn scan(port: u16, wait: Duration) -> Vec<Discovered> {
    let targets = default_targets(port, true).await;
    scan_targets(&targets, wait).await
}

/// 질의를 처음 보낸 뒤 `wait` 동안 다시 보내는 횟수. 질의·응답 UDP 하나가 사라져도(부하, 무선 LAN) 놓치지
/// 않게 `wait`를 `RESENDS + 1`등분한 때마다 같은 nonce로 다시 보낸다. 응답을 다 받은 유니캐스트 대상에는
/// 다시 보내지 않는다. 서버의 질의 횟수 제한(출발지 IP마다 초당 10회)보다 작아야 한다.
const RESENDS: u32 = 2;

/// 받기 오류(대부분 ICMP port unreachable)가 이만큼 쌓이면 소켓이 망가진 것으로 보고 끝낸다.
const MAX_RECV_ERRORS: u32 = 1000;

/// 한 번 훑을 때 모으는 서버(지문·출발지 IP) 수의 상한. 엉뚱한 응답이 쏟아져도 메모리가 늘지 않게 한다.
const MAX_ASSEMBLIES: usize = 256;

/// 같은 서버(지문)·같은 출발지의 응답 조각을 모으는 중인 것.
struct Assembly {
    fingerprint: Fingerprint,
    ip: IpAddr,
    parts: PartAssembler,
}

/// 주어진 대상에 질의를 보내고 `wait` 동안 응답을 모은다.
///
/// - 응답이 조각 여러 개로 오면(방송이 많은 서버) (지문, 출발지 IP)마다 모아 합친다. 같은 서버(지문)가 여러
///   경로로 응답하면 조각을 다 모은 것 가운데 LAN > Tailscale > 같은 PC 순으로 고른다.
/// - 조각을 다 못 모은 서버는, 다 모은 경로가 없으면 모은 만큼만 돌려준다([`Discovered::complete`]가
///   `false`). 서버를 아예 못 찾는 것보다 낫고, 빠진 방송은 다음 훑기에서 채워진다.
/// - 받기 오류(ICMP port unreachable 등)는 건너뛰고 계속 받는다. 질의는 `wait` 동안 몇 번 다시 보낸다.
/// - 대상이 모두 유니캐스트이고 모두 응답을 다 받았으면 `wait`를 채우지 않고 끝낸다.
pub async fn scan_targets(targets: &[(SocketAddr, Via)], wait: Duration) -> Vec<Discovered> {
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

    let tailscale_ips: Vec<IpAddr> = targets
        .iter()
        .filter(|(_, v)| *v == Via::Tailscale)
        .map(|(a, _)| a.ip())
        .collect();
    let broadcast_addrs: Vec<Ipv4Addr> = ipv4_interfaces()
        .iter()
        .filter_map(|i| i.broadcast)
        .collect();
    let broadcast: Vec<bool> = targets
        .iter()
        .map(|(a, _)| is_broadcast(a.ip(), &broadcast_addrs))
        .collect();

    let started = tokio::time::Instant::now();
    let deadline = started + wait;
    let mut resend_at: Vec<tokio::time::Instant> = (1..=RESENDS)
        .map(|k| started + wait * k / (RESENDS + 1))
        .collect();
    resend_at.reverse(); // 가장 이른 것이 끝에 오게 해서 pop한다.

    let mut assemblies: Vec<Assembly> = Vec::new();
    let mut complete_ips: HashSet<IpAddr> = HashSet::new();
    send_queries(&sock, &query, targets, |_| true).await;

    let mut errors = 0u32;
    let mut buf = vec![0u8; 4096];
    loop {
        // 유니캐스트 대상이 모두 응답을 다 줬으면 더 기다리지 않는다.
        if !targets.is_empty()
            && targets
                .iter()
                .zip(&broadcast)
                .all(|((a, _), is_bc)| !is_bc && complete_ips.contains(&a.ip()))
        {
            break;
        }
        let until = resend_at.last().copied().unwrap_or(deadline).min(deadline);
        match tokio::time::timeout_at(until, sock.recv_from(&mut buf)).await {
            Err(_) => {
                if until >= deadline {
                    break;
                }
                resend_at.pop();
                send_queries(&sock, &query, targets, |i| {
                    broadcast[i] || !complete_ips.contains(&targets[i].0.ip())
                })
                .await;
            }
            Ok(Err(e)) => {
                // 질의를 보낸 곳에서 ICMP port unreachable이 오면 Windows는 다음 recv_from을
                // WSAECONNRESET(ConnectionReset)으로 실패시킨다. 서버가 없는 Tailscale 피어나 닫힌 포트에
                // 보내는 일이 흔하므로, 이 오류로 끝내면 뒤에 오는 서버 응답을 모두 잃는다.
                errors += 1;
                if errors > MAX_RECV_ERRORS {
                    debug!("감지 소켓 받기 오류가 계속됩니다: {e}");
                    break;
                }
            }
            Ok(Ok((n, from))) => {
                let Some(part) = decode_part(&buf[..n]) else {
                    continue;
                };
                if part.response.nonce != nonce {
                    continue;
                }
                let fingerprint = part.response.fingerprint;
                let idx = match assemblies
                    .iter()
                    .position(|a| a.fingerprint == fingerprint && a.ip == from.ip())
                {
                    Some(i) => i,
                    None if assemblies.len() < MAX_ASSEMBLIES => {
                        assemblies.push(Assembly {
                            fingerprint,
                            ip: from.ip(),
                            parts: PartAssembler::new(),
                        });
                        assemblies.len() - 1
                    }
                    None => continue,
                };
                if assemblies[idx].parts.push(part) {
                    complete_ips.insert(from.ip());
                }
            }
        }
    }

    // 서버(지문)마다 경로를 고른다: 다 모은 것 > 경로 우선순위 > 방송이 많은 것 > 먼저 온 것.
    let mut best: Vec<(Fingerprint, Discovered)> = Vec::new();
    for a in &assemblies {
        let Some(response) = a.parts.merge() else {
            continue;
        };
        let via = classify_via(a.ip, &tailscale_ips);
        let entry = Discovered {
            ip: a.ip,
            via,
            complete: a.parts.is_complete(),
            response,
        };
        match best.iter_mut().find(|(fp, _)| *fp == a.fingerprint) {
            Some((_, prev)) => {
                if preference(&entry) < preference(prev) {
                    *prev = entry;
                }
            }
            None => best.push((a.fingerprint, entry)),
        }
    }
    let mut out: Vec<Discovered> = best.into_iter().map(|(_, d)| d).collect();
    out.sort_by(|a, b| a.response.name.cmp(&b.response.name));
    out
}

/// 대상마다 질의를 보낸다(`include(i)`가 참인 대상만).
async fn send_queries(
    sock: &UdpSocket,
    query: &[u8],
    targets: &[(SocketAddr, Via)],
    include: impl Fn(usize) -> bool,
) {
    for (i, (target, _)) in targets.iter().enumerate() {
        if !include(i) {
            continue;
        }
        if let Err(e) = sock.send_to(query, target).await {
            debug!("감지 질의를 보내지 못했습니다({target}): {e}");
        }
    }
}

/// 서브넷 브로드캐스트 주소인지(응답이 여러 서버에서 온다). `interface_broadcasts`는 이 PC 인터페이스들의
/// 브로드캐스트 주소다.
fn is_broadcast(ip: IpAddr, interface_broadcasts: &[Ipv4Addr]) -> bool {
    match ip {
        IpAddr::V4(v4) => v4 == Ipv4Addr::BROADCAST || interface_broadcasts.contains(&v4),
        IpAddr::V6(_) => false,
    }
}

/// 응답이 온 주소로 경로를 정한다(루프백, Tailscale 대역, 그 밖은 LAN).
fn classify_via(ip: IpAddr, tailscale_ips: &[IpAddr]) -> Via {
    if ip.is_loopback() {
        Via::Local
    } else if tailscale_ips.contains(&ip)
        || matches!(ip, IpAddr::V4(v4) if crate::ifaces::classify(v4) == IfaceKind::Tailscale)
    {
        Via::Tailscale
    } else {
        Via::Lan
    }
}

/// 같은 서버의 여러 응답 가운데 고르는 기준(작을수록 우선).
fn preference(d: &Discovered) -> (bool, u8, std::cmp::Reverse<usize>) {
    (
        !d.complete,
        rank(d.via),
        std::cmp::Reverse(d.response.broadcasts.len()),
    )
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

    #[tokio::test]
    async fn default_targets_without_tailscale_are_lan_broadcasts_and_loopback() {
        let targets = default_targets(8250, false).await;
        assert!(
            targets
                .iter()
                .all(|(a, v)| a.port() == 8250 && matches!(v, Via::Lan | Via::Local))
        );
        // 루프백은 항상 마지막에 하나 있고, 나머지는 서브넷 브로드캐스트(Lan)다.
        let (last, via) = targets.last().unwrap();
        assert_eq!(
            (last.ip(), *via),
            (IpAddr::V4(Ipv4Addr::LOCALHOST), Via::Local)
        );
        assert_eq!(targets.iter().filter(|(_, v)| *v == Via::Local).count(), 1);
    }

    #[tokio::test]
    async fn scan_targets_with_no_targets_finds_nothing() {
        assert!(
            scan_targets(&[], Duration::from_millis(50))
                .await
                .is_empty()
        );
    }

    // -----------------------------------------------------------------------
    // 가짜 서버로 훑기의 동작을 확인한다.

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use dongsi_core::discovery::{DiscoveredBroadcast, decode_query, encode_response};
    use dongsi_core::{InstallId, VideoCodec};

    fn response(broadcasts: usize, name: &str) -> DiscoveryResponse {
        DiscoveryResponse {
            nonce: 0,
            fingerprint: Fingerprint::from_bytes([7; 32]),
            name: name.into(),
            port: 9999,
            host_only: false,
            broadcasts: (0..broadcasts)
                .map(|i| DiscoveredBroadcast {
                    broadcaster: InstallId::from_bytes([i as u8 + 1; 16]),
                    name: format!("{:가<32}", format!("방송{i}")),
                    codec: VideoCodec::H264,
                    width: 1920,
                    height: 1080,
                    fps: 60,
                    locked: false,
                })
                .collect(),
        }
    }

    /// 질의에 응답하는 가짜 서버. `send(질의 순번, 조각 번호)`가 참인 조각만 보낸다.
    struct FakeServer {
        addr: SocketAddr,
        queries: Arc<AtomicUsize>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for FakeServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn fake_server(
        ip: Ipv4Addr,
        resp: DiscoveryResponse,
        send: impl Fn(usize, usize) -> bool + Send + 'static,
    ) -> FakeServer {
        let sock = UdpSocket::bind((ip, 0)).await.unwrap();
        let addr = sock.local_addr().unwrap();
        let queries = Arc::new(AtomicUsize::new(0));
        let counter = queries.clone();
        let task = tokio::spawn(async move {
            let mut buf = [0u8; 64];
            loop {
                let Ok((n, from)) = sock.recv_from(&mut buf).await else {
                    continue;
                };
                let Some(nonce) = decode_query(&buf[..n]) else {
                    continue;
                };
                let q = counter.fetch_add(1, Ordering::SeqCst);
                let mut r = resp.clone();
                r.nonce = nonce;
                for (i, p) in encode_response(&r).iter().enumerate() {
                    if send(q, i) {
                        let _ = sock.send_to(p, from).await;
                    }
                }
            }
        });
        FakeServer {
            addr,
            queries,
            task,
        }
    }

    fn unused_port() -> SocketAddr {
        let s = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        s.local_addr().unwrap()
    }

    fn found_fp(found: &[Discovered]) -> Option<&Discovered> {
        found
            .iter()
            .find(|d| d.fingerprint() == Fingerprint::from_bytes([7; 32]))
    }

    /// 고장 4(H1): 닫힌 포트에서 ICMP port unreachable이 와도(Windows는 다음 recv_from이 ConnectionReset)
    /// 그 뒤에 오는 서버 응답을 받는다.
    #[tokio::test]
    async fn scan_survives_icmp_errors() {
        let server = fake_server(Ipv4Addr::LOCALHOST, response(1, "서버"), |_, _| true).await;
        let targets = [
            (unused_port(), Via::Local),
            (unused_port(), Via::Tailscale),
            (unused_port(), Via::Lan),
            (server.addr, Via::Local),
        ];
        for round in 0..20 {
            let found = scan_targets(&targets, Duration::from_millis(400)).await;
            assert!(
                found_fp(&found).is_some(),
                "{round}번째 훑기가 서버를 놓쳤습니다"
            );
        }
    }

    /// 고장 5: 방송이 많은 서버의 조각을 모아 합친다.
    #[tokio::test]
    async fn scan_assembles_parts() {
        let server = fake_server(Ipv4Addr::LOCALHOST, response(32, "서버"), |_, _| true).await;
        let found = scan_targets(&[(server.addr, Via::Local)], Duration::from_millis(500)).await;
        let d = found_fp(&found).expect("서버를 못 찾았습니다");
        assert!(d.complete);
        assert_eq!(d.response.broadcasts, response(32, "서버").broadcasts);
    }

    /// 조각이 거꾸로(번호 역순)와도 합친다.
    #[tokio::test]
    async fn scan_assembles_parts_in_any_order() {
        let resp = response(32, "서버");
        let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = sock.local_addr().unwrap();
        let reply = resp.clone();
        let task = tokio::spawn(async move {
            let mut buf = [0u8; 64];
            loop {
                let Ok((n, from)) = sock.recv_from(&mut buf).await else {
                    continue;
                };
                let Some(nonce) = decode_query(&buf[..n]) else {
                    continue;
                };
                let mut r = reply.clone();
                r.nonce = nonce;
                for p in encode_response(&r).iter().rev() {
                    let _ = sock.send_to(p, from).await;
                }
            }
        });
        let found = scan_targets(&[(addr, Via::Local)], Duration::from_millis(500)).await;
        task.abort();
        let d = found_fp(&found).expect("서버를 못 찾았습니다");
        assert!(d.complete);
        assert_eq!(d.response.broadcasts, resp.broadcasts);
    }

    /// 고장 4(H3): 질의나 응답 하나가 사라져도 같은 nonce로 다시 보내서 찾는다.
    #[tokio::test]
    async fn scan_resends_lost_queries() {
        // 첫 질의에는 응답하지 않는다(사라진 것과 같다).
        let server = fake_server(Ipv4Addr::LOCALHOST, response(1, "서버"), |q, _| q >= 1).await;
        let found = scan_targets(&[(server.addr, Via::Local)], Duration::from_millis(600)).await;
        assert!(
            found_fp(&found).is_some(),
            "다시 보낸 질의에도 서버를 못 찾았습니다"
        );
        assert!(server.queries.load(Ordering::SeqCst) >= 2);
    }

    /// 응답을 다 받은 유니캐스트 대상에는 다시 보내지 않고, 모든 대상이 끝나면 `wait`를 채우지 않는다.
    #[tokio::test]
    async fn scan_stops_when_unicast_targets_are_complete() {
        let server = fake_server(Ipv4Addr::LOCALHOST, response(32, "서버"), |_, _| true).await;
        let started = Instant::now();
        let found = scan_targets(&[(server.addr, Via::Local)], Duration::from_secs(5)).await;
        assert!(found_fp(&found).is_some());
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        // 다시 보낼 때가 지나도 보내지 않았으므로 질의는 한 번이다.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(server.queries.load(Ordering::SeqCst), 1);
    }

    /// 조각 하나가 계속 안 오면 모은 만큼 돌려주고 `complete`는 거짓이다.
    #[tokio::test]
    async fn scan_returns_partial_when_a_part_never_arrives() {
        let server = fake_server(Ipv4Addr::LOCALHOST, response(32, "서버"), |_, part| {
            part != 1
        })
        .await;
        let found = scan_targets(&[(server.addr, Via::Local)], Duration::from_millis(500)).await;
        let d = found_fp(&found).expect("서버를 못 찾았습니다");
        assert!(!d.complete);
        assert!(!d.response.broadcasts.is_empty() && d.response.broadcasts.len() < 32);
    }

    /// 같은 서버가 두 경로로 응답하면 조각을 다 모은 경로를 고른다.
    #[tokio::test]
    async fn scan_prefers_the_complete_path() {
        let partial = fake_server(
            Ipv4Addr::new(127, 0, 0, 2),
            response(32, "서버"),
            |_, p| p != 1,
        )
        .await;
        let complete = fake_server(
            Ipv4Addr::new(127, 0, 0, 3),
            response(32, "서버"),
            |_, _| true,
        )
        .await;
        let found = scan_targets(
            &[(partial.addr, Via::Local), (complete.addr, Via::Local)],
            Duration::from_millis(500),
        )
        .await;
        assert_eq!(found.len(), 1);
        assert!(found[0].complete);
        assert_eq!(found[0].ip, complete.addr.ip());
        assert_eq!(found[0].response.broadcasts.len(), 32);
    }

    #[test]
    fn preference_order() {
        let mk = |complete: bool, via: Via, n: usize| Discovered {
            ip: Ipv4Addr::LOCALHOST.into(),
            via,
            complete,
            response: response(n, "서버"),
        };
        // 다 모은 것이 먼저, 그다음 경로(LAN > Tailscale > 같은 PC), 그다음 방송이 많은 것.
        assert!(preference(&mk(true, Via::Local, 1)) < preference(&mk(false, Via::Lan, 30)));
        assert!(preference(&mk(true, Via::Lan, 1)) < preference(&mk(true, Via::Tailscale, 1)));
        assert!(preference(&mk(true, Via::Tailscale, 1)) < preference(&mk(true, Via::Local, 1)));
        assert!(preference(&mk(false, Via::Lan, 5)) < preference(&mk(false, Via::Lan, 2)));
    }

    #[test]
    fn broadcast_addresses() {
        let all: Vec<Ipv4Addr> = ipv4_interfaces()
            .iter()
            .filter_map(|i| i.broadcast)
            .collect();
        assert!(is_broadcast(IpAddr::V4(Ipv4Addr::BROADCAST), &[]));
        assert!(!is_broadcast(IpAddr::V4(Ipv4Addr::LOCALHOST), &all));
        for b in &all {
            assert!(is_broadcast(IpAddr::V4(*b), &all));
        }
    }

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
