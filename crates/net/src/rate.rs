//! 송출 비트레이트 자동 조절(D-081).
//!
//! str0m BWE(GoogCC, TWCC 피드백)의 추정값과 서버가 보낸 REMB(내장 서버: 가장 느린 시청자의 추정값)로
//! 비디오 목표 비트레이트를 정한다. 방송 화면의 비트레이트 슬라이더 값은 상한이다.
//!
//! - 목표 = min(TWCC 추정, 3초 안의 REMB) × 0.9 − 오디오 몫, 300kbps~상한.
//! - 보통 내림: 원하는 값이 지금 목표의 90% 미만이면 직전 변경에서 3초 뒤부터(D-087, 옛 1초). Wi-Fi에서 추정이
//!   흔들릴 때마다 1초 간격으로 내리면 그때마다 인코더가 IDR을 내(ffmpeg nvenc는 `forceIDR`, AMF·QSV는 다시 연다,
//!   `crates/media/src/video.rs` `apply_cap`) 화면이 더 막히고 손실이 는다.
//! - 급한 내림: 원하는 값이 지금 목표의 70% 미만이면 3초를 기다리지 않는다. 다만 IDR이 몇백 ms 안에 연달아 생기지
//!   않게 직전 변경에서 500ms는 둔다.
//! - 올릴 때: 15% 넘게 높은 상태(또는 상한까지 오를 수 있는 상태)가 5초 이어지면. 2배 넘게 높으면 2초.
//!   화면이 멈춰 미디어가 적을 때(application-limited) 추정이 한 번 떨어지면 GoogCC는 받은 비트레이트의
//!   0.85배까지 내리므로, ALR 탐색으로 추정이 크게 돌아오면 빨리 따라간다.
//!
//! 비트레이트를 바꿀 때마다 키프레임이 생기므로(ffmpeg nvenc는 바꿀 때 IDR을 넣고, 다른 인코더는 다시 연다)
//! 자주 바꾸지 않는다.

use std::time::{Duration, Instant};

use dongsi_core::media::AUDIO_RED_DISTANCES;

/// 비디오 목표의 하한(kbps). 화질 설정의 최솟값과 같다.
pub const MIN_VIDEO_KBPS: u32 = 300;
/// 오디오 패킷 헤더(IP/UDP/RTP/SRTP 약 50바이트 × 10ms 패킷 100개/초, D-080) 몫(kbps).
/// 오디오 RED(D-087)는 패킷 수를 늘리지 않으므로 헤더 몫은 그대로고, 늘어나는 payload는
/// [`RateConfig::audio_share_kbps`]가 `audio × (1 + AUDIO_RED_DISTANCES 길이)`로 센다.
pub const AUDIO_OVERHEAD_KBPS: u32 = 40;
/// 추정값 중 미디어에 쓰는 비율(나머지는 재전송·헤더·흔들림 여유).
const HEADROOM: f64 = 0.9;
/// BWE가 찾아볼 상한(desired): (상한 + 오디오 몫) × 이 값.
const DESIRED_FACTOR: f64 = 1.25;
/// 서버가 보낸 REMB를 믿는 시간. 그동안 새 REMB가 없으면 무시한다(시청자가 모두 나갔거나 전용 서버).
pub const REMB_TTL: Duration = Duration::from_secs(3);
/// 원하는 값이 목표의 이 비율 미만이면 내린다(10% 안의 흔들림은 무시).
const DOWN_RATIO: f64 = 0.9;
/// 보통 내림의 최소 간격(직전 목표 변경에서 이만큼 뒤부터). D-087: 1초 → 3초.
const DOWN_GAP: Duration = Duration::from_secs(3);
/// 원하는 값이 목표의 이 비율 미만이면 급한 내림이다(대역폭이 반 토막 가까이 났다). D-087.
const URGENT_DOWN_RATIO: f64 = 0.7;
/// 급한 내림의 최소 간격. IDR이 몇백 ms 안에 연달아 나오지 않게 한다. D-087.
const URGENT_DOWN_GAP: Duration = Duration::from_millis(500);
const UP_RATIO: f64 = 1.15;
const UP_HOLD: Duration = Duration::from_secs(5);
const FAST_UP_RATIO: f64 = 2.0;
const FAST_UP_HOLD: Duration = Duration::from_secs(2);

/// 목표가 바뀐 까닭([`RateAdapter::tick`]). 진단 로그에 쓴다(D-087).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateReason {
    /// 보통 내림: 원하는 값이 목표의 70~90%.
    Down,
    /// 급한 내림: 원하는 값이 목표의 70% 미만.
    UrgentDown,
    /// 올림: 5초 유지.
    Up,
    /// 빠른 올림: 목표의 2배 넘게 돌아와 2초 유지.
    FastUp,
}

impl RateReason {
    /// 로그에 적는 글.
    pub fn label(self) -> &'static str {
        match self {
            RateReason::Down => "보통 내림",
            RateReason::UrgentDown => "급한 내림",
            RateReason::Up => "올림",
            RateReason::FastUp => "빠른 올림",
        }
    }
}

/// [`RateAdapter::tick`]이 돌려주는 목표 변경.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateChange {
    /// 새 비디오 목표(kbps).
    pub kbps: u32,
    pub reason: RateReason,
}

/// 송출 비트레이트 상한과 오디오.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateConfig {
    /// 비디오 비트레이트 상한(방송 화질 설정의 비트레이트, kbps).
    pub max_video_kbps: u32,
    /// 오디오(Opus) 비트레이트(kbps). `None`이면 오디오 트랙이 없다.
    pub audio_kbps: Option<u32>,
}

impl RateConfig {
    /// 오디오가 쓰는 몫(헤더와 RED 사본 포함, kbps). `audio × (1 + RED 거리 수) + 헤더`다(D-087).
    /// 128kbps Opus는 128 × 3 + 40 = 424kbps(RED 없이는 168). 실측(W4)은 payload 기준 +약 265kbps라 계산값
    /// (+256)과 맞는다.
    pub fn audio_share_kbps(&self) -> u32 {
        self.audio_kbps.map_or(0, |a| {
            a * (1 + AUDIO_RED_DISTANCES.len() as u32) + AUDIO_OVERHEAD_KBPS
        })
    }
}

/// 비디오 목표 비트레이트를 정하는 상태.
#[derive(Debug, Clone)]
pub struct RateAdapter {
    max_video: u32,
    audio: u32,
    twcc: Option<u32>,
    remb: Option<(u32, Instant)>,
    target: u32,
    changed_at: Option<Instant>,
    above_since: Option<Instant>,
}

impl RateAdapter {
    pub fn new(config: RateConfig) -> Self {
        let max_video = config.max_video_kbps.max(MIN_VIDEO_KBPS);
        RateAdapter {
            max_video,
            audio: config.audio_share_kbps(),
            twcc: None,
            remb: None,
            target: max_video,
            changed_at: None,
            above_since: None,
        }
    }

    /// BWE 시작 추정값(kbps). 이 값이면 시작 목표가 상한이 된다.
    pub fn initial_estimate_kbps(&self) -> u32 {
        ((self.max_video + self.audio) as f64 / HEADROOM).ceil() as u32
    }

    /// BWE가 찾아볼 상한(desired, kbps).
    pub fn desired_kbps(&self) -> u32 {
        ((self.max_video + self.audio) as f64 * DESIRED_FACTOR).ceil() as u32
    }

    /// 지금 비디오 목표(kbps).
    pub fn target_kbps(&self) -> u32 {
        self.target
    }

    pub fn max_video_kbps(&self) -> u32 {
        self.max_video
    }

    /// 네트워크 때문에 상한보다 낮추고 있는지.
    pub fn limited(&self) -> bool {
        self.target < self.max_video
    }

    /// TWCC 추정값(전체, kbps).
    pub fn on_twcc(&mut self, kbps: u32) {
        self.twcc = Some(kbps);
    }

    /// 서버가 보낸 REMB(전체, kbps).
    pub fn on_remb(&mut self, kbps: u32, now: Instant) {
        self.remb = Some((kbps, now));
    }

    pub fn twcc_kbps(&self) -> Option<u32> {
        self.twcc
    }

    /// 아직 믿을 수 있는 REMB.
    pub fn remb_kbps(&self, now: Instant) -> Option<u32> {
        self.remb
            .filter(|(_, at)| now.saturating_duration_since(*at) <= REMB_TTL)
            .map(|(k, _)| k)
    }

    /// 쓸 수 있는 전체 대역폭 추정(kbps): TWCC와 REMB 중 작은 값.
    pub fn available_kbps(&self, now: Instant) -> Option<u32> {
        match (self.twcc, self.remb_kbps(now)) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// 전체 대역폭이 `total`일 때 비디오에 줄 몫.
    fn video_for(&self, total: u32) -> u32 {
        ((total as f64 * HEADROOM) as u32)
            .saturating_sub(self.audio)
            .clamp(MIN_VIDEO_KBPS, self.max_video)
    }

    fn wanted(&self, now: Instant) -> Option<u32> {
        self.available_kbps(now).map(|t| self.video_for(t))
    }

    /// 시간이 지나거나 추정값이 바뀌었을 때 부른다. 목표가 바뀌면 새 목표와 까닭을 돌려준다.
    pub fn tick(&mut self, now: Instant) -> Option<RateChange> {
        let Some(want) = self.wanted(now) else {
            self.above_since = None;
            return None;
        };
        let target = self.target as f64;
        if (want as f64) < target * DOWN_RATIO {
            self.above_since = None;
            let urgent = (want as f64) < target * URGENT_DOWN_RATIO;
            let (gap, reason) = if urgent {
                (URGENT_DOWN_GAP, RateReason::UrgentDown)
            } else {
                (DOWN_GAP, RateReason::Down)
            };
            let gap_ok = self
                .changed_at
                .is_none_or(|t| now.saturating_duration_since(t) >= gap);
            if gap_ok {
                return Some(self.change(want, reason, now));
            }
            return None;
        }
        let up =
            (want as f64) > target * UP_RATIO || (want == self.max_video && want > self.target);
        if !up {
            self.above_since = None;
            return None;
        }
        let since = *self.above_since.get_or_insert(now);
        let (hold, reason) = if (want as f64) >= target * FAST_UP_RATIO {
            (FAST_UP_HOLD, RateReason::FastUp)
        } else {
            (UP_HOLD, RateReason::Up)
        };
        if now.saturating_duration_since(since) >= hold {
            return Some(self.change(want, reason, now));
        }
        None
    }

    fn change(&mut self, kbps: u32, reason: RateReason, now: Instant) -> RateChange {
        RateChange {
            kbps: self.set_target(kbps, now),
            reason,
        }
    }

    fn set_target(&mut self, kbps: u32, now: Instant) -> u32 {
        self.target = kbps;
        self.changed_at = Some(now);
        self.above_since = None;
        kbps
    }

    /// 상한(화질 설정의 비트레이트)을 바꾼다. BWE를 새 시작값([`initial_estimate_kbps`](Self::initial_estimate_kbps))에서
    /// 다시 시작해야 하면 `true`.
    ///
    /// - 내리면: 목표가 새 상한보다 높으면 바로 새 상한으로.
    /// - 올리면: 지금 네트워크 때문에 낮추고 있지 않으면 바로 새 상한으로 올리고(REMB가 있으면 그 몫까지만) BWE를
    ///   다시 시작한다. 낮추고 있으면 목표는 그대로 두고 평소의 올림 규칙을 따른다.
    pub fn set_max_video(&mut self, kbps: u32, now: Instant) -> bool {
        let kbps = kbps.max(MIN_VIDEO_KBPS);
        let old = self.max_video;
        let was_limited = self.limited();
        self.max_video = kbps;
        if kbps == old {
            return false;
        }
        if self.target > kbps {
            self.set_target(kbps, now);
            return false;
        }
        if kbps > old && !was_limited {
            let cap = self.remb_kbps(now).map(|r| self.video_for(r));
            let next = cap.map_or(kbps, |c| c.min(kbps)).max(self.target);
            self.set_target(next, now);
            self.twcc = Some(self.initial_estimate_kbps());
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(max: u32, audio: Option<u32>) -> RateConfig {
        RateConfig {
            max_video_kbps: max,
            audio_kbps: audio,
        }
    }

    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// [`RateAdapter::tick`]의 새 목표만 본다.
    fn tick(r: &mut RateAdapter, now: Instant) -> Option<u32> {
        r.tick(now).map(|c| c.kbps)
    }

    #[test]
    fn starts_at_the_cap_and_initial_estimate_keeps_it() {
        let mut r = RateAdapter::new(cfg(7000, Some(128)));
        assert_eq!(r.target_kbps(), 7000);
        assert!(!r.limited());
        // 시작 추정값 그대로면 목표가 상한이다.
        let t0 = Instant::now();
        r.on_twcc(r.initial_estimate_kbps());
        assert_eq!(tick(&mut r, t0), None);
        assert_eq!(r.target_kbps(), 7000);
        assert!(r.desired_kbps() > r.initial_estimate_kbps());
        // 오디오 RED(D-087): 128 × (1 + 2) + 40
        assert_eq!(cfg(7000, Some(128)).audio_share_kbps(), 424);
        assert_eq!(cfg(7000, None).audio_share_kbps(), 0);
    }

    /// 오디오 RED(D-087)로 오디오 몫이 168 → 424kbps가 되어도 비디오 목표는 바닥(300) 아래로 내려가지 않는다.
    /// 추정값이 (300 + 424) / 0.9 ≈ 804kbps 아래면 비디오 300 + 오디오 424가 추정값을 넘는다(정책은 정하지 않았다).
    #[test]
    fn audio_red_share_squeezes_video_on_narrow_links() {
        let t0 = Instant::now();
        let mut r = RateAdapter::new(cfg(1000, Some(128)));
        assert_eq!(r.initial_estimate_kbps(), 1583); // (1000 + 424) / 0.9
        // 추정 1000kbps: 1000 × 0.9 − 424 = 476(RED 없이는 732).
        r.on_twcc(1000);
        assert_eq!(tick(&mut r, t0), Some(476));
        // 추정 700kbps: 630 − 424 = 206이지만 바닥 300(RED 없이는 462).
        r.on_twcc(700);
        assert_eq!(tick(&mut r, t0 + s(3)), Some(MIN_VIDEO_KBPS));
    }

    #[test]
    fn drops_urgently_when_the_estimate_collapses() {
        let t0 = Instant::now();
        let mut r = RateAdapter::new(cfg(8000, Some(128)));
        r.on_twcc(3000);
        // 처음 내림은 기다리지 않는다. 3000 × 0.9 − 424 = 2276
        assert_eq!(tick(&mut r, t0), Some(2276));
        assert!(r.limited());
        // 1500 → 926는 2276의 41%(70% 미만): 급한 내림이라 3초를 기다리지 않지만 500ms는 둔다.
        r.on_twcc(1500);
        assert_eq!(tick(&mut r, t0 + ms(499)), None);
        assert_eq!(
            r.tick(t0 + ms(500)).map(|c| c.reason),
            Some(RateReason::UrgentDown)
        );
        assert_eq!(r.target_kbps(), 926);
        // 바닥은 300kbps. 급한 내림도 직전 변경에서 500ms는 둔다.
        r.on_twcc(100);
        assert_eq!(tick(&mut r, t0 + ms(900)), None);
        assert_eq!(tick(&mut r, t0 + ms(1000)), Some(MIN_VIDEO_KBPS));
    }

    #[test]
    fn normal_drop_waits_three_seconds() {
        let t0 = Instant::now();
        let mut r = RateAdapter::new(cfg(8000, None));
        r.on_twcc(5000);
        // 5000 × 0.9 = 4500
        assert_eq!(tick(&mut r, t0), Some(4500));
        // 3600: 4500의 80%(70~90%)라 보통 내림. 직전 변경에서 3초 뒤부터다.
        r.on_twcc(4000);
        assert_eq!(tick(&mut r, t0 + s(1)), None);
        assert_eq!(tick(&mut r, t0 + ms(2999)), None);
        let c = r.tick(t0 + s(3)).unwrap();
        assert_eq!((c.kbps, c.reason), (3600, RateReason::Down));
        // 기다리는 동안 70% 아래로 떨어지면 3초를 기다리지 않고 급한 내림이 된다.
        r.on_twcc(3500); // 3150: 3600의 87.5%
        assert_eq!(tick(&mut r, t0 + s(4)), None);
        r.on_twcc(2000); // 1800: 3600의 50%
        assert_eq!(tick(&mut r, t0 + ms(3400)), None);
        let c = r.tick(t0 + ms(3500)).unwrap();
        assert_eq!((c.kbps, c.reason), (1800, RateReason::UrgentDown));
    }

    #[test]
    fn exactly_seventy_percent_is_not_urgent() {
        let t0 = Instant::now();
        let mut r = RateAdapter::new(cfg(10_000, None));
        r.on_twcc(2000);
        assert_eq!(tick(&mut r, t0), Some(1800));
        // 1800의 70%는 1260. 1260은 70% 미만이 아니므로 보통 내림이다(3초).
        r.on_twcc(1400); // 1260
        assert_eq!(tick(&mut r, t0 + ms(500)), None);
        assert_eq!(tick(&mut r, t0 + s(3)), Some(1260));
        // 조금(10% 이하) 흔들리는 것은 무시한다.
        let mut r = RateAdapter::new(cfg(8000, None));
        r.on_twcc(8500); // 8500 × 0.9 = 7650 (8000의 95.6%)
        assert_eq!(tick(&mut r, t0), None);
        assert_eq!(r.target_kbps(), 8000);
    }

    #[test]
    fn reports_why_the_target_changed() {
        let t0 = Instant::now();
        let mut r = RateAdapter::new(cfg(8000, None));
        r.on_twcc(2000);
        assert_eq!(
            r.tick(t0),
            Some(RateChange {
                kbps: 1800,
                reason: RateReason::UrgentDown
            })
        );
        // 1800의 1.9배(2배 미만)로 돌아오면 5초 뒤 올림.
        r.on_twcc(3800);
        assert_eq!(
            r.tick(t0 + s(6)).map(|c| (c.kbps, c.reason)),
            None,
            "처음 본 시각부터 5초를 센다"
        );
        assert_eq!(
            r.tick(t0 + s(11)).map(|c| (c.kbps, c.reason)),
            Some((3420, RateReason::Up))
        );
        // 2배 넘게 돌아오면 2초 뒤 빠른 올림.
        r.on_twcc(8000); // 7200: 3420의 2.1배
        assert_eq!(r.tick(t0 + s(12)), None);
        assert_eq!(
            r.tick(t0 + s(14)).map(|c| (c.kbps, c.reason)),
            Some((7200, RateReason::FastUp))
        );
        assert_eq!(RateReason::Down.label(), "보통 내림");
        assert_eq!(RateReason::UrgentDown.label(), "급한 내림");
        assert_eq!(RateReason::Up.label(), "올림");
        assert_eq!(RateReason::FastUp.label(), "빠른 올림");
    }

    #[test]
    fn rises_only_after_holding_for_five_seconds() {
        let t0 = Instant::now();
        let mut r = RateAdapter::new(cfg(8000, None));
        r.on_twcc(2000);
        assert_eq!(tick(&mut r, t0), Some(1800));
        // 회복: 3800 × 0.9 = 3420 (1800의 1.9배)
        r.on_twcc(3800);
        assert_eq!(tick(&mut r, t0 + s(1)), None);
        assert_eq!(tick(&mut r, t0 + s(5)), None);
        assert_eq!(tick(&mut r, t0 + s(6)), Some(3420));
        // 1.15배 안(3600 < 3933)이면 오르지 않는다.
        r.on_twcc(4000);
        assert_eq!(tick(&mut r, t0 + s(6) + ms(500)), None);
        // 중간에 내려가면 다시 센다.
        r.on_twcc(6000); // 5400
        assert_eq!(tick(&mut r, t0 + s(7)), None);
        r.on_twcc(4100); // 3690: 1.15배 안이라 대기 취소
        assert_eq!(tick(&mut r, t0 + s(9)), None);
        r.on_twcc(6000);
        assert_eq!(tick(&mut r, t0 + s(10)), None);
        assert_eq!(tick(&mut r, t0 + s(14)), None);
        assert_eq!(tick(&mut r, t0 + s(15)), Some(5400));
        // 상한 근처(1.15배 안)라도 상한까지는 오른다.
        r.on_twcc(9500); // 8550 → 상한 8000, 5400의 1.48배
        assert_eq!(tick(&mut r, t0 + s(16)), None);
        assert_eq!(tick(&mut r, t0 + s(21)), Some(8000));
        assert!(!r.limited());
        let mut r = RateAdapter::new(cfg(8000, None));
        r.on_twcc(7900); // 7110: 8000 × 0.9 = 7200보다 낮아 내린다
        assert_eq!(tick(&mut r, t0), Some(7110));
        r.on_twcc(9000); // 8100 → 8000, 7110의 1.13배
        assert_eq!(tick(&mut r, t0 + s(1)), None);
        assert_eq!(tick(&mut r, t0 + s(6)), Some(8000));
    }

    #[test]
    fn rises_fast_after_a_big_recovery() {
        let t0 = Instant::now();
        let mut r = RateAdapter::new(cfg(8000, None));
        r.on_twcc(300);
        assert_eq!(tick(&mut r, t0), Some(MIN_VIDEO_KBPS));
        // ALR 탐색으로 추정이 크게 돌아왔다(목표의 2배 넘게): 2초 뒤 올린다.
        r.on_twcc(6000);
        assert_eq!(tick(&mut r, t0 + s(1)), None);
        assert_eq!(tick(&mut r, t0 + s(2)), None);
        assert_eq!(tick(&mut r, t0 + s(3)), Some(5400));
    }

    #[test]
    fn remb_caps_until_it_expires() {
        let t0 = Instant::now();
        let mut r = RateAdapter::new(cfg(8000, None));
        r.on_twcc(r.initial_estimate_kbps());
        r.on_remb(3000, t0);
        assert_eq!(r.available_kbps(t0), Some(3000));
        assert_eq!(tick(&mut r, t0), Some(2700));
        // REMB가 3초 넘게 안 오면 TWCC만 본다(시청자가 모두 나감).
        assert_eq!(r.remb_kbps(t0 + s(3)), Some(3000));
        assert_eq!(r.remb_kbps(t0 + s(4)), None);
        assert_eq!(tick(&mut r, t0 + s(4)), None);
        assert_eq!(tick(&mut r, t0 + s(5)), None);
        assert_eq!(tick(&mut r, t0 + s(6)), Some(8000));
    }

    #[test]
    fn no_estimate_keeps_the_target() {
        let t0 = Instant::now();
        let mut r = RateAdapter::new(cfg(5000, Some(128)));
        assert_eq!(r.available_kbps(t0), None);
        assert_eq!(tick(&mut r, t0 + s(10)), None);
        assert_eq!(r.target_kbps(), 5000);
    }

    #[test]
    fn changing_the_cap() {
        let t0 = Instant::now();
        // 낮추고 있지 않을 때 올리면 바로 오르고 BWE를 다시 시작한다.
        let mut r = RateAdapter::new(cfg(5000, None));
        r.on_twcc(r.initial_estimate_kbps());
        assert!(r.set_max_video(8000, t0));
        assert_eq!(r.target_kbps(), 8000);
        assert_eq!(r.twcc_kbps(), Some(r.initial_estimate_kbps()));
        assert_eq!(
            tick(&mut r, t0 + ms(100)),
            None,
            "다시 시작한 추정값으로는 내리지 않는다"
        );
        // 내리면 바로 내린다.
        assert!(!r.set_max_video(3000, t0 + s(1)));
        assert_eq!(r.target_kbps(), 3000);
        // 같은 값은 아무 일도 없다.
        assert!(!r.set_max_video(3000, t0 + s(2)));

        // 네트워크 때문에 낮추고 있으면 올려도 목표는 그대로다.
        let mut r = RateAdapter::new(cfg(5000, None));
        r.on_twcc(2000);
        assert_eq!(tick(&mut r, t0), Some(1800));
        assert!(!r.set_max_video(8000, t0 + s(1)));
        assert_eq!(r.target_kbps(), 1800);
        assert_eq!(r.max_video_kbps(), 8000);

        // REMB가 있으면 그 몫까지만 바로 올린다.
        let mut r = RateAdapter::new(cfg(5000, None));
        r.on_twcc(r.initial_estimate_kbps());
        r.on_remb(7000, t0);
        assert!(r.set_max_video(10_000, t0));
        assert_eq!(r.target_kbps(), 6300);
        // 상한은 최소값 아래로 내려가지 않는다.
        assert!(!r.set_max_video(10, t0 + s(1)));
        assert_eq!(r.max_video_kbps(), MIN_VIDEO_KBPS);
    }
}
