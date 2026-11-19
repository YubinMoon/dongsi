//! 로컬 감지 응답(D-030, D-039): 감지 질의를 읽는 스레드에서 바로 응답한다.
//!
//! 감지 질의는 SFU 스레드를 거치지 않는다. SFU 스레드는 미디어 패킷을 모든 세션에 `drive`한 뒤에야
//! 다음 패킷을 보므로, 질의가 그 뒤에 줄을 서면 미디어 부하만큼 응답이 늦어진다. 읽기 스레드는 소켓에서
//! 읽은 패킷의 첫 4바이트로 감지 패킷만 골라 이 모듈의 [`Responder`]에 넘기고, 나머지(WebRTC)는 채널로
//! SFU 스레드에 보낸다. 같은 UDP 소켓으로 보내므로 응답의 출발지 주소·포트는 질의를 받은 서버 주소다.
//!
//! 응답 횟수 제한은 **질의 단위**로 센다. 질의 하나에 응답 패킷(조각)이 여러 개여도 한 번이다.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use dongsi_core::discovery::{DiscoveryResponse, decode_query, encode_response, is_local_source};

/// 감지 응답 내용(시그널링 쪽이 갱신한다). `nonce`는 응답할 때 채운다.
pub type DiscoverySnapshot = Arc<RwLock<DiscoveryResponse>>;

/// 한 출발지 IP가 1초에 보낼 수 있는 질의 수. 훑기 한 번이 질의를 처음 보내기와 다시 보내기 포함 3번
/// 보내므로, 같은 IP(같은 PC의 앱 여러 개, Docker 브리지 뒤의 클라이언트들)에서 몇 번 훑어도 막히지 않을
/// 만큼 잡는다.
pub(crate) const MAX_QUERIES_PER_SOURCE: u32 = 10;

/// 모든 출발지를 합쳐 1초에 응답할 질의 수.
pub(crate) const MAX_QUERIES_TOTAL: u32 = 200;

/// 감지 질의에 응답한다.
pub(crate) struct Responder {
    snapshot: DiscoverySnapshot,
    limit: RateLimit,
}

impl Responder {
    pub(crate) fn new(snapshot: DiscoverySnapshot) -> Self {
        Self {
            snapshot,
            limit: RateLimit::default(),
        }
    }

    /// 받은 UDP 패킷이 감지 질의면 응답 패킷을 `send`로 하나씩 보낸다. 질의가 아니거나, 출발지가
    /// 로컬(사설·루프백·링크 로컬·Tailscale)이 아니거나, 횟수 제한에 걸리면 아무것도 보내지 않는다.
    pub(crate) fn handle(
        &mut self,
        data: &[u8],
        source: SocketAddr,
        now: Instant,
        mut send: impl FnMut(&[u8]),
    ) {
        let Some(nonce) = decode_query(data) else {
            return;
        };
        if !is_local_source(source.ip()) || !self.limit.allow(source, now) {
            return;
        }
        let mut resp = self
            .snapshot
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        resp.nonce = nonce;
        for packet in encode_response(&resp) {
            send(&packet);
        }
    }
}

/// 감지 응답 횟수 제한: 1초 구간마다 출발지별·전체 질의 수를 센다.
#[derive(Default)]
struct RateLimit {
    window_start: Option<Instant>,
    total: u32,
    per_source: HashMap<IpAddr, u32>,
}

impl RateLimit {
    fn allow(&mut self, source: SocketAddr, now: Instant) -> bool {
        if self
            .window_start
            .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(1))
        {
            self.window_start = Some(now);
            self.total = 0;
            self.per_source.clear();
        }
        // 전체 한도를 넘긴 뒤에는 출발지를 기록하지 않는다(위조 출발지로 표가 커지는 것을 막는다).
        if self.total >= MAX_QUERIES_TOTAL {
            return false;
        }
        let n = self.per_source.entry(source.ip()).or_insert(0);
        if *n >= MAX_QUERIES_PER_SOURCE {
            return false;
        }
        *n += 1;
        self.total += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use dongsi_core::Fingerprint;
    use dongsi_core::InstallId;
    use dongsi_core::VideoCodec;
    use dongsi_core::discovery::{DiscoveredBroadcast, PartAssembler, decode_part, encode_query};

    use super::*;

    fn snapshot(broadcasts: usize) -> DiscoverySnapshot {
        Arc::new(RwLock::new(DiscoveryResponse {
            nonce: 0,
            fingerprint: Fingerprint::from_bytes([9; 32]),
            name: "시험".into(),
            port: 7884,
            host_only: false,
            broadcasts: (0..broadcasts)
                .map(|i| DiscoveredBroadcast {
                    broadcaster: InstallId::from_bytes([i as u8 + 1; 16]),
                    name: format!("{:가<32}", format!("방송{i}")),
                    codec: VideoCodec::H264,
                    width: 3840,
                    height: 2160,
                    fps: 60,
                    locked: false,
                })
                .collect(),
        }))
    }

    fn ask(r: &mut Responder, from: &str, nonce: u64, now: Instant) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        r.handle(&encode_query(nonce), from.parse().unwrap(), now, |p| {
            out.push(p.to_vec())
        });
        out
    }

    #[test]
    fn answers_with_all_parts_and_the_query_nonce() {
        let mut r = Responder::new(snapshot(32));
        let packets = ask(&mut r, "192.168.0.5:4000", 0xABCD, Instant::now());
        assert!(packets.len() > 1);
        let mut a = PartAssembler::new();
        for p in &packets {
            let part = decode_part(p).unwrap();
            assert_eq!(part.response.nonce, 0xABCD);
            a.push(part);
        }
        assert!(a.is_complete());
        assert_eq!(a.merge().unwrap().broadcasts.len(), 32);
    }

    #[test]
    fn ignores_non_queries_and_non_local_sources() {
        let mut r = Responder::new(snapshot(1));
        let now = Instant::now();
        assert!(ask(&mut r, "8.8.8.8:4000", 1, now).is_empty());
        assert!(ask(&mut r, "[2001:db8::1]:4000", 1, now).is_empty());
        let mut sent = 0;
        r.handle(b"dsq1short", "127.0.0.1:1".parse().unwrap(), now, |_| {
            sent += 1
        });
        r.handle(
            &[0x80, 0x60, 0, 0],
            "127.0.0.1:1".parse().unwrap(),
            now,
            |_| sent += 1,
        );
        assert_eq!(sent, 0);
        assert_eq!(ask(&mut r, "127.0.0.1:1", 1, now).len(), 1);
    }

    /// 횟수는 응답 패킷이 아니라 질의로 센다: 조각이 여러 개인 응답도 질의 한도까지 모두 응답한다.
    #[test]
    fn rate_limit_counts_queries_not_packets() {
        let mut r = Responder::new(snapshot(32));
        let now = Instant::now();
        let mut answered = 0;
        let mut packets = 0;
        for i in 0..(MAX_QUERIES_PER_SOURCE + 5) {
            let p = ask(&mut r, "192.168.0.5:4000", u64::from(i), now);
            if !p.is_empty() {
                answered += 1;
                packets += p.len();
            }
        }
        assert_eq!(answered, MAX_QUERIES_PER_SOURCE);
        assert!(
            packets > answered as usize,
            "질의마다 조각이 여러 개여야 합니다"
        );
    }

    #[test]
    fn rate_limit_per_source_and_total() {
        let mut rl = RateLimit::default();
        let now = Instant::now();
        let a: SocketAddr = "192.168.0.2:5000".parse().unwrap();
        let b: SocketAddr = "192.168.0.3:5000".parse().unwrap();
        for _ in 0..MAX_QUERIES_PER_SOURCE {
            assert!(rl.allow(a, now));
        }
        assert!(!rl.allow(a, now));
        assert!(rl.allow(b, now));
        assert!(rl.allow(a, now + Duration::from_secs(1)));

        // 전체 한도
        let mut rl = RateLimit::default();
        let mut allowed = 0;
        for i in 0..(MAX_QUERIES_TOTAL + 50) {
            let src = SocketAddr::from(([10, 0, (i / 250) as u8, (i % 250) as u8], 1));
            if rl.allow(src, now) {
                allowed += 1;
            }
        }
        assert_eq!(allowed, MAX_QUERIES_TOTAL);
        assert!(rl.per_source.len() <= MAX_QUERIES_TOTAL as usize);
    }
}
