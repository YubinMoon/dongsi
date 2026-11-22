//! 손실 주입 도구(D-087): 시험용 str0m 클라이언트의 소켓 보내기·받기 사이에 끼워 패킷을 결정적으로 버리거나 붙잡는다.
//!
//! 난수를 쓰지 않는다. 규칙은 "지금부터 N ms 동안", "다음 N개", "M개 중 앞의 K개(몰린 손실)"처럼 시간과 순서로만
//! 정한다. 같은 규칙이면 같은 패킷이 버려진다(단, "N ms 동안"은 시계에 걸리므로 시험이 도는 PC 부하에 따라 그 사이에
//! 지나가는 패킷 수는 달라진다).
//!
//! # 패킷 종류
//!
//! SRTP도 RTP 머리는 평문이라 PT·SSRC·seq를 읽을 수 있다(서버 `sfu.rs`의 `rtp_ssrc`와 같은 판별). 어떤 PT가 비디오
//! 원본·재전송(RTX)·오디오인지는 SDP answer의 `a=rtpmap:<PT> <코덱>/<클럭>`으로 정한다: `rtx`면 [`Class::Rtx`],
//! `opus`면 [`Class::Audio`], 그 밖의 코덱(H264·AV1 등)이면 [`Class::Video`]. 방송자·시청자 시험 클라이언트가
//! `start`에서 answer를 넘기면 알아서 채운다(`Impair::set_sdp`). RTCP·STUN·DTLS는 [`Class::Other`]이고 규칙이 건드리지
//! 않는다(손실 주입은 미디어 패킷만).
//!
//! # 규칙
//!
//! - [`Impair::drop_for`]: 지금부터 `dur` 동안 고른 종류의 패킷을 모두 버린다. RTX까지 같이 고르면 복구 불가능한 손실이다.
//! - [`Impair::drop_next`]: 다음 `n`개를 버린다.
//! - [`Impair::drop_pattern_for`]: 고른 종류의 패킷을 센다. `m`개씩 묶어 앞의 `k`개를 버린다(몰린 손실). 손실률은 `k/m`.
//! - [`Impair::hold_for`]: `dur` 동안 고른 종류의 패킷을 버리지 않고 붙잡았다가, 끝나는 때 순서대로 한꺼번에 내보낸다
//!   (Wi-Fi 정체 흉내). 붙잡히지 않은 패킷은 그대로 지나가므로 다른 종류와는 순서가 바뀐다.
//! - 규칙이 여러 개면 먼저 건 것부터 보고, 그 패킷의 종류를 고른 첫 규칙이 정한다.
//!
//! # 쓰는 방법
//!
//! 소켓 쪽 스레드가 패킷마다 [`Impair::release`]로 붙잡았던 것을 내보내고 [`Impair::judge`]로 판정한다
//! (`Fate::Pass`면 그대로, 아니면 버리거나 붙잡았으므로 쓰지 않는다). `Impair`는 `Clone`(내부 `Arc<Mutex<..>>`)이라 시험
//! 쪽이 같은 손잡이로 규칙을 건다. 규칙이 없으면 `judge`는 종류만 세고 `Pass`를 돌려준다.
//!
//! 카운터([`Impair::counters`])는 종류마다 본·버린·붙잡은 패킷 수와 바이트를 센다. [`Impair::record`]를 켜면 패킷마다
//! (시각, 종류, 길이)도 남겨서(`Impair::log`) 시험이 최근 1초 재전송 비율 같은 것을 다시 계산할 수 있다.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::ops::BitOr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 패킷 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// 비디오 원본 RTP
    Video,
    /// 비디오 재전송(RTX) RTP
    Rtx,
    /// 오디오 RTP
    Audio,
    /// RTCP·STUN·DTLS와 PT를 모르는 패킷. 규칙이 건드리지 않는다.
    Other,
}

impl Class {
    fn bit(self) -> u8 {
        match self {
            Class::Video => 1,
            Class::Rtx => 2,
            Class::Audio => 4,
            Class::Other => 0,
        }
    }
}

/// 규칙이 고르는 종류의 집합.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Classes(u8);

impl Classes {
    pub const VIDEO: Classes = Classes(1);
    pub const RTX: Classes = Classes(2);
    /// 비디오 원본과 재전송(복구 불가능한 손실).
    pub const VIDEO_AND_RTX: Classes = Classes(1 | 2);

    fn has(self, c: Class) -> bool {
        self.0 & c.bit() != 0
    }
}

impl BitOr for Classes {
    type Output = Classes;
    fn bitor(self, rhs: Classes) -> Classes {
        Classes(self.0 | rhs.0)
    }
}

/// SDP answer에서 읽은 PT 분류.
#[derive(Debug, Default, Clone)]
struct PtMap {
    video: Vec<u8>,
    rtx: Vec<u8>,
    audio: Vec<u8>,
}

impl PtMap {
    fn from_sdp(sdp: &str) -> Self {
        let mut m = Self::default();
        for line in sdp.lines() {
            let Some(rest) = line.trim().strip_prefix("a=rtpmap:") else {
                continue;
            };
            let mut parts = rest.split_whitespace();
            let (Some(pt), Some(codec)) = (parts.next(), parts.next()) else {
                continue;
            };
            let Ok(pt) = pt.parse::<u8>() else {
                continue;
            };
            let name = codec.split('/').next().unwrap_or("").to_ascii_lowercase();
            match name.as_str() {
                "rtx" => m.rtx.push(pt),
                "opus" => m.audio.push(pt),
                "h264" | "h265" | "av1" | "vp8" | "vp9" => m.video.push(pt),
                _ => {}
            }
        }
        m
    }

    fn classify(&self, packet: &[u8]) -> Class {
        // 서버 sfu.rs rtp_ssrc와 같은 판별: RTP 버전 2, RTCP(PT 200~206 = 둘째 바이트 하위 7비트 72~78)는 제외.
        if packet.len() < 12 || packet[0] >> 6 != 2 {
            return Class::Other;
        }
        let pt = packet[1] & 0x7f;
        if (64..96).contains(&pt) {
            return Class::Other;
        }
        if self.rtx.contains(&pt) {
            Class::Rtx
        } else if self.video.contains(&pt) {
            Class::Video
        } else if self.audio.contains(&pt) {
            Class::Audio
        } else {
            Class::Other
        }
    }
}

/// 한 종류의 카운터.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClassCount {
    /// 본 패킷(버린 것·붙잡은 것 포함)
    pub seen: u64,
    pub bytes: u64,
    pub dropped: u64,
    pub dropped_bytes: u64,
    /// 붙잡은 패킷(나중에 내보낸 것도 센다)
    pub held: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    pub video: ClassCount,
    pub rtx: ClassCount,
    pub audio: ClassCount,
}

/// 패킷 하나의 기록(`Impair::record`를 켰을 때).
#[derive(Debug, Clone, Copy)]
pub struct LogEntry {
    pub at: Instant,
    pub class: Class,
    pub len: usize,
}

/// `Impair::judge`의 판정.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fate {
    /// 그대로 지나간다.
    Pass,
    /// 버렸다.
    Drop,
    /// 붙잡았다(규칙이 끝나면 `release`가 내준다).
    Held,
}

#[derive(Debug)]
enum Action {
    Drop,
    /// 고른 종류의 패킷을 `m`개씩 묶어 앞의 `k`개를 버린다.
    Pattern {
        k: u64,
        m: u64,
    },
    Hold,
}

#[derive(Debug)]
struct Rule {
    classes: Classes,
    action: Action,
    /// 이 시각까지만 쓴다(없으면 `left`로 끝난다).
    until: Option<Instant>,
    /// 남은 패킷 수(`drop_next`).
    left: Option<u64>,
    /// 이 규칙이 센 패킷 수(`Pattern`).
    seen: u64,
}

#[derive(Debug)]
struct Held {
    addr: SocketAddr,
    data: Vec<u8>,
    /// 이 시각이 되면 내보낸다(규칙의 끝).
    release_at: Instant,
}

#[derive(Debug, Default)]
struct State {
    pt: PtMap,
    rules: Vec<Rule>,
    held: VecDeque<Held>,
    counters: Counters,
    recording: bool,
    log: Vec<LogEntry>,
}

impl State {
    fn count_mut(&mut self, c: Class) -> Option<&mut ClassCount> {
        match c {
            Class::Video => Some(&mut self.counters.video),
            Class::Rtx => Some(&mut self.counters.rtx),
            Class::Audio => Some(&mut self.counters.audio),
            Class::Other => None,
        }
    }

    /// 끝난 규칙을 치운다.
    fn expire(&mut self, now: Instant) {
        self.rules
            .retain(|r| r.until.is_none_or(|t| now < t) && r.left != Some(0));
    }
}

/// 손실 주입 손잡이. `Clone`하면 같은 규칙·카운터를 본다.
#[derive(Debug, Clone, Default)]
pub struct Impair {
    inner: Arc<Mutex<State>>,
}

impl Impair {
    pub fn new() -> Self {
        Self::default()
    }

    /// SDP answer에서 PT를 읽어 패킷 종류를 정한다.
    pub fn set_sdp(&self, sdp: &str) {
        self.inner.lock().unwrap().pt = PtMap::from_sdp(sdp);
    }

    fn add(&self, rule: Rule) {
        self.inner.lock().unwrap().rules.push(rule);
    }

    /// 지금부터 `dur` 동안 `classes`의 패킷을 모두 버린다.
    pub fn drop_for(&self, classes: Classes, dur: Duration) {
        self.add(Rule {
            classes,
            action: Action::Drop,
            until: Some(Instant::now() + dur),
            left: None,
            seen: 0,
        });
    }

    /// `classes`의 다음 `n`개를 버린다.
    pub fn drop_next(&self, classes: Classes, n: u64) {
        self.add(Rule {
            classes,
            action: Action::Drop,
            until: None,
            left: Some(n),
            seen: 0,
        });
    }

    /// 지금부터 `dur` 동안 `classes`의 패킷을 `m`개씩 묶어 앞의 `k`개를 버린다(몰린 손실, 손실률 `k/m`).
    pub fn drop_pattern_for(&self, classes: Classes, k: u64, m: u64, dur: Duration) {
        assert!(k <= m && m > 0, "k={k} m={m}");
        self.add(Rule {
            classes,
            action: Action::Pattern { k, m },
            until: Some(Instant::now() + dur),
            left: None,
            seen: 0,
        });
    }

    /// `dur` 동안 `classes`의 패킷을 붙잡았다가 끝나는 때 순서대로 한꺼번에 내보낸다.
    pub fn hold_for(&self, classes: Classes, dur: Duration) {
        self.add(Rule {
            classes,
            action: Action::Hold,
            until: Some(Instant::now() + dur),
            left: None,
            seen: 0,
        });
    }

    /// 걸린 규칙이 하나도 없고 붙잡은 패킷도 없는지.
    pub fn is_idle(&self) -> bool {
        let mut s = self.inner.lock().unwrap();
        s.expire(Instant::now());
        s.rules.is_empty() && s.held.is_empty()
    }

    pub fn counters(&self) -> Counters {
        self.inner.lock().unwrap().counters
    }

    /// 패킷마다 기록을 남기기 시작한다(`log`).
    pub fn record(&self) {
        self.inner.lock().unwrap().recording = true;
    }

    pub fn log(&self) -> Vec<LogEntry> {
        self.inner.lock().unwrap().log.clone()
    }

    /// 붙잡았다가 내보낼 때가 된 패킷(들어온 순서대로).
    pub fn release(&self) -> Vec<(SocketAddr, Vec<u8>)> {
        let mut s = self.inner.lock().unwrap();
        if s.held.is_empty() {
            return Vec::new();
        }
        let now = Instant::now();
        let mut out = Vec::new();
        let mut keep = VecDeque::new();
        while let Some(h) = s.held.pop_front() {
            if h.release_at <= now {
                out.push((h.addr, h.data));
            } else {
                keep.push_back(h);
            }
        }
        s.held = keep;
        out
    }

    /// 패킷 하나를 판정한다. `Held`면 이 안에 복사해 두었다가 `release`가 내준다.
    pub fn judge(&self, addr: SocketAddr, data: &[u8]) -> Fate {
        let mut s = self.inner.lock().unwrap();
        let now = Instant::now();
        let class = s.pt.classify(data);
        s.expire(now);
        let mut fate = Fate::Pass;
        let mut release_at = now;
        if class != Class::Other {
            for r in &mut s.rules {
                if !r.classes.has(class) {
                    continue;
                }
                match r.action {
                    Action::Drop => fate = Fate::Drop,
                    Action::Pattern { k, m } => {
                        if r.seen % m < k {
                            fate = Fate::Drop;
                        }
                        r.seen += 1;
                        if fate == Fate::Pass {
                            // 이 규칙이 통과시킨 패킷은 다른 규칙도 볼 수 있게 한다.
                            continue;
                        }
                    }
                    Action::Hold => {
                        fate = Fate::Held;
                        release_at = r.until.unwrap_or(now);
                    }
                }
                if let Some(left) = &mut r.left {
                    *left -= 1;
                }
                break;
            }
        }
        let len = data.len() as u64;
        if let Some(c) = s.count_mut(class) {
            c.seen += 1;
            c.bytes += len;
            match fate {
                Fate::Drop => {
                    c.dropped += 1;
                    c.dropped_bytes += len;
                }
                Fate::Held => c.held += 1,
                Fate::Pass => {}
            }
        }
        if s.recording && class != Class::Other {
            s.log.push(LogEntry {
                at: now,
                class,
                len: data.len(),
            });
        }
        if fate == Fate::Held {
            s.held.push_back(Held {
                addr,
                data: data.to_vec(),
                release_at,
            });
        }
        fate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SDP: &str = "m=video 9 UDP/TLS/RTP/SAVPF 96 97\r\na=rtpmap:96 H264/90000\r\na=rtpmap:97 rtx/90000\r\na=fmtp:97 apt=96\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=rtpmap:111 opus/48000/2\r\n";

    fn pkt(pt: u8) -> Vec<u8> {
        let mut p = vec![0x80, pt, 0, 1, 0, 0, 0, 0, 1, 2, 3, 4];
        p.extend_from_slice(&[0xAA; 8]);
        p
    }

    fn addr() -> SocketAddr {
        "127.0.0.1:9".parse().unwrap()
    }

    fn impair() -> Impair {
        let i = Impair::new();
        i.set_sdp(SDP);
        i
    }

    #[test]
    fn classifies_by_pt_and_ignores_rtcp() {
        let m = PtMap::from_sdp(SDP);
        assert_eq!(m.classify(&pkt(96)), Class::Video);
        assert_eq!(m.classify(&pkt(97)), Class::Rtx);
        assert_eq!(m.classify(&pkt(111)), Class::Audio);
        assert_eq!(m.classify(&pkt(100)), Class::Other);
        // SR(PT 200) 둘째 바이트 하위 7비트 = 72
        assert_eq!(m.classify(&pkt(200)), Class::Other);
        assert_eq!(
            m.classify(&[0x00, 0x01, 0, 0, 0x21, 0x12, 0xA4, 0x42, 0, 0, 0, 0]),
            Class::Other
        );
        assert_eq!(m.classify(&[0x80, 96]), Class::Other);
    }

    #[test]
    fn drop_next_and_counters() {
        let i = impair();
        i.drop_next(Classes::VIDEO, 2);
        let fates: Vec<Fate> = (0..4).map(|_| i.judge(addr(), &pkt(96))).collect();
        assert_eq!(fates, [Fate::Drop, Fate::Drop, Fate::Pass, Fate::Pass]);
        // 고르지 않은 종류는 그대로.
        i.drop_next(Classes::VIDEO, 1);
        assert_eq!(i.judge(addr(), &pkt(97)), Fate::Pass);
        assert_eq!(i.judge(addr(), &pkt(96)), Fate::Drop);
        let c = i.counters();
        assert_eq!((c.video.seen, c.video.dropped), (5, 3));
        assert_eq!((c.rtx.seen, c.rtx.dropped), (1, 0));
        assert!(i.is_idle());
    }

    #[test]
    fn pattern_drops_k_of_m_leading() {
        let i = impair();
        i.drop_pattern_for(Classes::VIDEO, 3, 10, Duration::from_secs(5));
        let fates: Vec<bool> = (0..20)
            .map(|_| i.judge(addr(), &pkt(96)) == Fate::Drop)
            .collect();
        let dropped: Vec<usize> = (0..20).filter(|n| fates[*n]).collect();
        assert_eq!(dropped, [0, 1, 2, 10, 11, 12]);
        // RTX는 세지도 않는다.
        assert_eq!(i.judge(addr(), &pkt(97)), Fate::Pass);
    }

    #[test]
    fn drop_for_expires_and_holds_release_in_order() {
        let i = impair();
        i.drop_for(Classes::VIDEO_AND_RTX, Duration::from_millis(60));
        assert_eq!(i.judge(addr(), &pkt(96)), Fate::Drop);
        assert_eq!(i.judge(addr(), &pkt(97)), Fate::Drop);
        assert_eq!(i.judge(addr(), &pkt(111)), Fate::Pass);
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(i.judge(addr(), &pkt(96)), Fate::Pass);

        i.hold_for(Classes::VIDEO, Duration::from_millis(60));
        let mut a = pkt(96);
        a[3] = 1;
        let mut b = pkt(96);
        b[3] = 2;
        assert_eq!(i.judge(addr(), &a), Fate::Held);
        assert_eq!(i.judge(addr(), &pkt(111)), Fate::Pass);
        assert_eq!(i.judge(addr(), &b), Fate::Held);
        assert!(i.release().is_empty());
        std::thread::sleep(Duration::from_millis(80));
        let out = i.release();
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].1[3], out[1].1[3]), (1, 2));
        assert!(i.is_idle());
        assert_eq!(i.counters().video.held, 2);
    }
}
