//! 로컬 감지(D-030, D-039) 시험: 방송이 많을 때 목록이 다 오는지, 미디어 부하 중에도 응답이 빠지지 않는지.
//!
//! 외부 서버 모드 규칙(main.rs 머리 문서)을 따른다. 내부 서버 설정이 필요한 시험은 `internal_only`다.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dongsi_core::VideoCodec;
use dongsi_core::discovery::{MAX_RESPONSE_BYTES, decode_part, encode_query};
use dongsi_net::discovery::{Via, scan_targets};
use dongsi_server::ServerConfig;

use crate::client::{Client, Watching};
use crate::media::{Pump, info};
use crate::target::{Target, default_config};

/// 글자 수가 `MAX_DISPLAY_NAME_CHARS`(32)인 한글 이름.
fn long_name(i: usize) -> String {
    let mut name = format!("방송{i:02}");
    while name.chars().count() < dongsi_core::proto::MAX_DISPLAY_NAME_CHARS {
        name.push('가');
    }
    name
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let idx = ((sorted.len() as f64 * p).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[idx]
}

/// 쓰는 사람이 없는 UDP 포트(닫힌 포트). 이 포트로 보내면 ICMP port unreachable이 돌아온다.
fn closed_udp_port() -> u16 {
    let s = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    s.local_addr().unwrap().port()
}

// ---------------------------------------------------------------------------
// 고장 5: 방송이 많아도 감지 목록이 다 온다

/// 방송 32개(이름은 최대 길이 한글)를 올리고 `scan_targets`가 32개를 모두 돌려받는지 본다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_many_broadcasts() {
    const N: usize = 32;
    let Some(t) = Target::internal_only(
        "discovery_many_broadcasts",
        ServerConfig {
            max_broadcasts: N,
            ..default_config()
        },
    )
    .await
    else {
        return;
    };
    let mut live = Vec::new();
    for i in 0..N {
        let mut c = Client::connect(&t, &long_name(i)).await;
        let p = c.publish(info(VideoCodec::H264), None).await.unwrap();
        live.push((c, p));
    }
    let targets = [(t.udp_addr(), Via::Local)];
    let found = scan_targets(&targets, Duration::from_millis(800)).await;
    let me = found
        .iter()
        .find(|d| d.fingerprint() == t.fp)
        .expect("감지 응답이 없습니다");
    let ids: std::collections::HashSet<_> = live.iter().map(|(c, _)| c.id).collect();
    let got: std::collections::HashSet<_> = me
        .response
        .broadcasts
        .iter()
        .map(|b| b.broadcaster)
        .collect();
    eprintln!(
        "[e2e] 감지: 방송 {N}개 중 {}개를 받음(이름 {}자)",
        got.len(),
        long_name(0).chars().count()
    );
    assert_eq!(got, ids, "감지 응답에 방송 {N}개가 모두 있어야 합니다");
    assert!(
        me.response
            .broadcasts
            .iter()
            .all(|b| b.name.chars().count() == dongsi_core::proto::MAX_DISPLAY_NAME_CHARS),
        "방송 이름이 잘렸습니다"
    );
}

// ---------------------------------------------------------------------------
// 고장 4: 부하·오류 중에도 감지 응답이 빠지지 않는다

/// 대상 목록 앞에 닫힌 포트가 있어도(ICMP port unreachable → Windows에서 다음 recv가 WSAECONNRESET)
/// 서버 응답을 받는다. 서버가 없는 Tailscale 피어·닫힌 포트가 흔한 실사용과 같다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_scan_survives_closed_ports() {
    let t = Target::shared().await;
    let _gate = t.discovery_gate().await;
    let closed = closed_udp_port();
    let targets = [
        (SocketAddr::from((Ipv4Addr::LOCALHOST, closed)), Via::Local),
        (
            SocketAddr::from((Ipv4Addr::LOCALHOST, closed)),
            Via::Tailscale,
        ),
        (t.udp_addr(), Via::Local),
    ];
    let scans = scan_count(&t);
    let mut missed = 0;
    for _ in 0..scans {
        let found = scan_targets(&targets, Duration::from_millis(300)).await;
        if !found.iter().any(|d| d.fingerprint() == t.fp) {
            missed += 1;
        }
        // 응답 횟수 제한(출발지마다 초당 10회)을 넘지 않게 한다. 훑기 하나는 질의를 3번 보내므로
        // (300ms 동안 처음 + 다시 보내기 둘) 550ms마다 3번, 초당 약 5.5번이다.
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    eprintln!("[e2e] 닫힌 포트가 앞에 있는 훑기 {scans}번 중 {missed}번 서버를 못 찾음");
    assert_eq!(
        missed, 0,
        "닫힌 포트의 오류에 훑기가 끝나 서버를 놓쳤습니다"
    );
}

/// 미디어를 흘리는 부하(방송 `broadcasts`개 × 시청자 `viewers`명). 버리면 멈춘다.
struct MediaLoad {
    stop: Arc<AtomicBool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    _clients: Vec<Client>,
}

impl MediaLoad {
    async fn start(t: &Target, broadcasts: u32, viewers: usize) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let mut tasks = Vec::new();
        let mut clients = Vec::new();
        for i in 0..broadcasts {
            let mut b = Client::connect(t, &format!("부하 방송자 {i}")).await;
            let id = b.id;
            let publisher = b.publish(info(VideoCodec::Vp8), None).await.unwrap();
            let mut pump = Pump::new(100 + i);
            pump.vp8(&publisher, 10).await;
            let mut watching: Vec<Watching> = Vec::new();
            for v in 0..viewers {
                let mut c = Client::connect(t, &format!("부하 시청자 {i}-{v}")).await;
                watching.push(c.watch_connected(id, None).await);
                clients.push(c);
            }
            let flag = stop.clone();
            tasks.push(tokio::spawn(async move {
                let mut pump = pump;
                let mut last_drain = Instant::now();
                while !flag.load(Ordering::Relaxed) {
                    pump.vp8(&publisher, 25).await;
                    if last_drain.elapsed() > Duration::from_millis(500) {
                        for w in &watching {
                            w.viewer.drain();
                        }
                        last_drain = Instant::now();
                    }
                }
            }));
            clients.push(b);
        }
        Self {
            stop,
            tasks,
            _clients: clients,
        }
    }

    async fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.tasks {
            let _ = t.await;
        }
    }
}

/// 질의 하나를 보내 응답이 올 때까지의 시간. `wait` 안에 오지 않으면 `None`.
async fn query_once(
    sock: &tokio::net::UdpSocket,
    target: SocketAddr,
    wait: Duration,
) -> Option<Duration> {
    let nonce: u64 = rand::random();
    let started = Instant::now();
    sock.send_to(&encode_query(nonce), target).await.ok()?;
    let mut buf = vec![0u8; 4096];
    loop {
        let left = wait.checked_sub(started.elapsed())?;
        let Ok(Ok((n, _))) = tokio::time::timeout(left, sock.recv_from(&mut buf)).await else {
            return None;
        };
        if decode_part(&buf[..n]).is_some_and(|p| p.response.nonce == nonce) {
            return Some(started.elapsed());
        }
    }
}

struct Stats {
    sent: usize,
    missed: usize,
    p50: Duration,
    p95: Duration,
    max: Duration,
}

impl Stats {
    fn of(results: Vec<Option<Duration>>) -> Self {
        let sent = results.len();
        let mut ok: Vec<Duration> = results.into_iter().flatten().collect();
        ok.sort();
        Self {
            sent,
            missed: sent - ok.len(),
            p50: percentile(&ok, 0.5),
            p95: percentile(&ok, 0.95),
            max: ok.last().copied().unwrap_or_default(),
        }
    }

    fn print(&self, what: &str) {
        eprintln!(
            "[e2e] {what}: 질의 {}번 중 {}번 빠짐({:.1}%), 지연 p50 {:.1}ms / p95 {:.1}ms / 최대 {:.1}ms",
            self.sent,
            self.missed,
            100.0 * self.missed as f64 / self.sent.max(1) as f64,
            self.p50.as_secs_f64() * 1000.0,
            self.p95.as_secs_f64() * 1000.0,
            self.max.as_secs_f64() * 1000.0,
        );
    }
}

/// 훑기 시험을 몇 번 되풀이할지. 외부 서버에서는 시험이 한 번에 하나씩 돌아서(`discovery_gate`) 전체
/// 시간이 늘지 않게 줄인다.
fn scan_count(t: &Target) -> usize {
    if t.is_external() { 20 } else { 40 }
}

/// 유니캐스트 질의를 여러 번 보낸다.
///
/// - 내부 서버: 출발지 IP를 127.0.0.x 20개로 나눠(출발지마다 초당 10회 제한) 짧은 시간에 수백 번 보낸다.
/// - 외부 서버: 출발지 하나(0.0.0.0)로 초당 4번씩 20번. 외부 서버는 Docker 브리지(docker-proxy)나 WSL
///   localhost 전달 뒤에 있을 수 있고, 거기서는 모든 클라이언트가 한 출발지로 합쳐지며 127.0.0.2 같은 다른
///   루프백 출발지로는 응답이 돌아오지 않는다(`Target::discovery_gate`).
async fn query_many(t: &Target) -> Stats {
    let (sources, per_source, gap) = if t.is_external() {
        (1usize, 20usize, Duration::from_millis(250))
    } else {
        (20, 15, Duration::from_millis(300))
    };
    let target = t.udp_addr();
    let mut tasks = Vec::new();
    for s in 0..sources {
        let bind: SocketAddr = if t.is_external() {
            "0.0.0.0:0".parse().unwrap()
        } else {
            SocketAddr::from((Ipv4Addr::new(127, 0, 0, 2 + s as u8), 0))
        };
        let sock = tokio::net::UdpSocket::bind(bind).await.unwrap();
        tasks.push(tokio::spawn(async move {
            let mut out = Vec::new();
            for _ in 0..per_source {
                out.push(query_once(&sock, target, Duration::from_secs(1)).await);
                tokio::time::sleep(gap).await;
            }
            out
        }));
    }
    let mut all = Vec::new();
    for t in tasks {
        all.extend(t.await.unwrap());
    }
    Stats::of(all)
}

/// 미디어를 흘리는 중에도 유니캐스트 감지 질의가 빠지지 않고 빨리 답한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_unicast_under_media_load() {
    let t = Target::shared_with(3).await;
    let _gate = t.discovery_gate().await;
    let idle = query_many(&t).await;
    idle.print("유휴 상태 유니캐스트");

    let load = MediaLoad::start(&t, 3, 5).await;
    let loaded = query_many(&t).await;
    loaded.print("미디어 부하(방송 3 × 시청자 5) 중 유니캐스트");
    load.stop().await;

    assert_eq!(idle.missed, 0, "유휴 상태에서 응답이 빠졌습니다");
    assert_eq!(loaded.missed, 0, "미디어 부하 중 응답이 빠졌습니다");
    assert!(
        loaded.p95 <= Duration::from_millis(50),
        "미디어 부하 중 응답 지연 p95가 큽니다: {:?}",
        loaded.p95
    );
    assert!(
        loaded.max <= Duration::from_millis(200),
        "미디어 부하 중 응답 지연 최대가 큽니다: {:?}",
        loaded.max
    );
}

/// 미디어를 흘리는 중에 `scan_targets`(닫힌 포트 포함)를 여러 번 해도 서버를 놓치지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_scan_under_media_load() {
    let t = Target::shared_with(3).await;
    let _gate = t.discovery_gate().await;
    let load = MediaLoad::start(&t, 3, 5).await;
    let closed = closed_udp_port();
    let targets = [
        (SocketAddr::from((Ipv4Addr::LOCALHOST, closed)), Via::Local),
        (t.udp_addr(), Via::Local),
    ];
    let scans = scan_count(&t);
    let mut results = Vec::new();
    for _ in 0..scans {
        let started = Instant::now();
        let found = scan_targets(&targets, Duration::from_millis(400)).await;
        let hit = found.iter().any(|d| d.fingerprint() == t.fp);
        results.push(hit.then(|| started.elapsed()));
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    load.stop().await;
    let stats = Stats::of(results);
    stats.print("미디어 부하 중 scan_targets(닫힌 포트 포함)");
    assert_eq!(stats.missed, 0, "부하 중 훑기가 서버를 놓쳤습니다");
}

// ---------------------------------------------------------------------------
// 응답 횟수 제한(고장 4의 H4): 질의 단위로 센다

/// 한 출발지에서 질의를 몇 번 보냈을 때 몇 번 응답을 받는지. 응답은 nonce별로 센다(조각 여러 개도 한 번).
async fn answered(
    sock: &tokio::net::UdpSocket,
    target: SocketAddr,
    queries: usize,
    gap: Duration,
) -> usize {
    let mut nonces = Vec::new();
    for _ in 0..queries {
        let nonce: u64 = rand::random();
        nonces.push(nonce);
        let _ = sock.send_to(&encode_query(nonce), target).await;
        if !gap.is_zero() {
            tokio::time::sleep(gap).await;
        }
    }
    let mut got = std::collections::HashSet::new();
    let mut buf = vec![0u8; 4096];
    let end = Instant::now() + Duration::from_millis(600);
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        let Ok(Ok((n, _))) = tokio::time::timeout(left, sock.recv_from(&mut buf)).await else {
            break;
        };
        assert!(n <= MAX_RESPONSE_BYTES);
        if let Some(p) = decode_part(&buf[..n])
            && nonces.contains(&p.response.nonce)
        {
            got.insert(p.response.nonce);
        }
    }
    got.len()
}

/// 같은 PC·같은 IP에서 앱 여러 개나 시험이 훑어도(훑기 하나가 질의를 처음+다시 보내기 3번) 정상 질의가
/// 막히지 않고, 그래도 한 출발지가 쏟아내는 질의는 막는다(증폭 공격 방어).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_rate_limit() {
    let Some(t) = Target::exclusive("discovery_rate_limit", false).await else {
        return;
    };
    let _gate = t.discovery_gate().await;
    // 내부 서버: 다른 시험과 출발지 IP가 겹치지 않게 전용 루프백 주소를 쓴다. 외부 서버: 출발지 하나
    // (WSL localhost 전달·docker-proxy 뒤에서는 127.0.0.77 같은 출발지로 응답이 돌아오지 않는다).
    let bind: SocketAddr = if t.is_external() {
        "0.0.0.0:0".parse().unwrap()
    } else {
        (Ipv4Addr::new(127, 0, 0, 77), 0).into()
    };
    // 이 실행의 앞선 질의 기록(1초 구간)이 지워지게 한다.
    tokio::time::sleep(Duration::from_millis(1100)).await;

    // 훑기 세 번(질의 9번)을 1초 안에 보낸다: 모두 응답받아야 한다.
    let sock = tokio::net::UdpSocket::bind(bind).await.unwrap();
    let normal = answered(&sock, t.udp_addr(), 9, Duration::from_millis(90)).await;
    eprintln!("[e2e] 한 출발지가 0.8초에 질의 9번 → 응답 {normal}번");
    assert_eq!(normal, 9, "정상 훑기가 응답 횟수 제한에 막혔습니다");

    // 쏟아내는 질의: 막힌다. 1초 구간이 걸쳐도 한도의 두 배를 넘지 않는다.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let flood = answered(&sock, t.udp_addr(), 60, Duration::ZERO).await;
    eprintln!("[e2e] 한 출발지가 한꺼번에 질의 60번 → 응답 {flood}번");
    assert!(
        (5..=20).contains(&flood),
        "한꺼번에 보낸 질의 60번에 응답 {flood}번(10~20번이어야 합니다)"
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
}
