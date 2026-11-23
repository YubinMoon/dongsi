//! QP 기반 자동 해상도 판정 상태 기계(D-099, 안 B).
//!
//! 인코더 하나(simulcast 계층 하나)마다 [`QpScaler`] 하나를 쓴다. 인코더가 프레임마다 내는 평균 QP를
//! 넣으면, QP가 오래 높을 때 출력 상자를 한 단계 줄이라고(내림), 오래 낮을 때 한 단계 되돌리라고(올림) 알려 준다.
//! libwebrtc QualityScaler처럼 비트레이트가 아니라 인코더 QP로 판정한다. ffmpeg·스레드와 무관한 순수 로직이고,
//! 시각은 호출자가 인자로 준다(시험이 결정적이다).
//!
//! 호출자(T3b)가 할 일:
//! 1. 프레임을 낼 때마다 [`QpScaler::push`]에 [`QpFrame`]를 넣는다. QP는 코덱 고유 범위(H.264 0~51, AV1 0~255)로 준다.
//! 2. [`Verdict::next`]가 `Some`이면 그 단계의 상자 크기([`QpScaler::step_box`])로 인코더를 다시 열고
//!    [`QpScaler::switched`]를 부른다. 다시 열지 못하면 부르지 않는다(`push`가 같은 답을 계속 준다).
//! 3. 단계를 안 바꾸고 인코더만 다시 열었을 때(비트레이트 변경으로 다시 연 경우 등)는 [`QpScaler::reset`]를 부른다.
//! 4. 켜기/끄기는 호출자 몫이다. 끄면 `push`를 부르지 않으면 된다.
//!
//! 판정(D-099 초기안, 값은 모두 [`QpScaleConfig`]):
//! - 최근 3초의 이동 평균. 키프레임, 그 직후 짧은 구간, 인코더를 연 직후 짧은 구간의 표본은 뺀다
//!   (nvenc는 비트레이트를 바꿀 때마다 IDR을 걸어 QP가 튄다).
//! - 평균 창 안 표본이 1초분 넘게 퍼져 있고 개수도 충분할 때만 판정한다(멈춘 화면은 프레임이 드물다).
//! - 내림: 평균 ≥ high가 3초 이어질 때. 올림: 평균 ≤ low가 10초 이어지고 직전 전환에서 15초 지났을 때.
//!   한 번에 한 단계.
//! - 올린 뒤 30초 안에 다시 내려가면 올림 대기(15초)를 2배로 늘린다(최대 2분). 올린 지 30초 넘어 내려가면 처음 값으로 돌아간다.
//! - 임계값은 libwebrtc QualityScaler의 값을 그대로 쓴다(H.264 24/37, AV1 qindex 145/205, 출처는 [`QpThresholds::default_for`]).
//!   한 곳에서 바꾼다.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::media::VideoCodec;

/// 코덱별 QP 임계값. 평균 QP가 `high` 이상이면 화질이 모자란 것, `low` 이하이면 여유가 있는 것이다.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QpThresholds {
    pub low: f32,
    pub high: f32,
}

impl QpThresholds {
    /// 코덱별 기본 임계값. libwebrtc QualityScaler가 실제로 쓰는 값이다(R1 조사, 2026-11-23, libwebrtc main a222406d):
    ///
    /// - H.264 24/37: `modules/video_coding/codecs/h264/h264_encoder_impl.cc:81~82`(OpenH264), Android 하드웨어
    ///   `sdk/android/src/java/org/webrtc/HardwareVideoEncoder.java:513~525`. Chrome·네이티브(iOS 제외)는 필드 트라이얼
    ///   `rtc_base/experiments/quality_scaling_experiment.cc:28~45`의 기본 문자열(h264 24/37)이 인코더 값을 덮어쓴다.
    /// - AV1 145/205(qindex 0~255): `modules/video_coding/codecs/av1/libaom_av1_encoder.cc:91~92`
    ///   (`kLowQindex`·`kHighQindex`), Chromium 하드웨어 `rtc_video_encoder.cc:3127~3131`.
    ///
    /// 한 곳에서 바꾼다. 비교는 libwebrtc가 high는 초과, low는 이하이고 여기는 high 이상·low 이하다.
    pub fn default_for(codec: VideoCodec) -> Self {
        match codec {
            // H.264 QP 범위 0~51.
            VideoCodec::H264 => Self {
                low: 24.0,
                high: 37.0,
            },
            // AV1 QP 범위 0~255(nvenc_av1).
            VideoCodec::Av1 => Self {
                low: 145.0,
                high: 205.0,
            },
        }
    }
}

/// 판정 설정. [`QpScaleConfig::new`]가 D-099 초기안을 준다.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QpScaleConfig {
    pub thresholds: QpThresholds,
    /// 이동 평균 창 길이.
    pub window: Duration,
    /// 평균 창 안 표본의 시간 폭(맨 앞~맨 뒤)이 이보다 짧으면 판정하지 않는다.
    pub min_span: Duration,
    /// 평균 창 안 표본이 이보다 적으면 판정하지 않는다.
    pub min_samples: usize,
    /// 키프레임 뒤 이 시간 안의 프레임은 표본에서 뺀다(키프레임 자신도 뺀다).
    pub key_skip: Duration,
    /// 인코더를 연([`QpScaler::reset`]·[`QpScaler::switched`]) 뒤 이 시간 안의 프레임은 표본에서 뺀다.
    pub open_skip: Duration,
    /// 내림: 평균 ≥ high가 이만큼 이어질 때.
    pub down_hold: Duration,
    /// 올림: 평균 ≤ low가 이만큼 이어질 때.
    pub up_hold: Duration,
    /// 올림: 직전 전환에서 이만큼 지났을 때(처음 값).
    pub up_gap: Duration,
    /// 올림 뒤 이 시간 안에 다시 내려가면 올림 대기를 늘린다.
    pub relapse_window: Duration,
    /// 올림 대기의 최댓값(늘릴 때마다 2배).
    pub up_gap_max: Duration,
}

impl QpScaleConfig {
    /// D-099 초기안.
    pub fn new(codec: VideoCodec) -> Self {
        Self {
            thresholds: QpThresholds::default_for(codec),
            window: Duration::from_secs(3),
            min_span: Duration::from_secs(1),
            min_samples: 10,
            key_skip: Duration::from_millis(500),
            open_skip: Duration::from_secs(1),
            down_hold: Duration::from_secs(3),
            up_hold: Duration::from_secs(10),
            up_gap: Duration::from_secs(15),
            relapse_window: Duration::from_secs(30),
            up_gap_max: Duration::from_secs(120),
        }
    }
}

/// 인코더가 낸 프레임 하나의 정보.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QpFrame {
    /// 프레임을 받은 시각(호출자가 정한다).
    pub at: Instant,
    /// 프레임 평균 QP(코덱 고유 범위: H.264 0~51, AV1 0~255).
    pub qp: f32,
    /// 키프레임(IDR)이면 true. ffmpeg side data의 pict_type이 I이거나 패킷이 key 표시면 true로 준다.
    pub key: bool,
}

/// [`QpScaler::push`]의 답.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    /// 지금 단계(0 = 원래 설정 크기).
    pub index: usize,
    /// 바꿀 단계. 호출자는 그 크기로 인코더를 다시 열고 [`QpScaler::switched`]를 부른다.
    pub next: Option<usize>,
}

/// 인코더 하나의 QP 기반 해상도 판정기.
#[derive(Debug, Clone)]
pub struct QpScaler {
    config: QpScaleConfig,
    steps: Vec<(u32, u32)>,
    index: usize,
    /// 평균 창 안의 표본(시각, QP).
    samples: VecDeque<(Instant, f32)>,
    sum: f64,
    /// 인코더를 연 시각. 이 뒤 `open_skip` 동안은 표본을 뺀다.
    opened_at: Option<Instant>,
    /// 마지막 키프레임 시각.
    last_key: Option<Instant>,
    /// 평균이 high 이상이 된 시각(이어지는 동안만 있다).
    high_since: Option<Instant>,
    /// 평균이 low 이하가 된 시각.
    low_since: Option<Instant>,
    /// 마지막 단계 전환 시각.
    last_change: Option<Instant>,
    /// 마지막으로 올린 시각(재발 판정용).
    last_up: Option<Instant>,
    /// 지금 쓰는 올림 대기.
    up_gap: Duration,
}

impl QpScaler {
    /// `steps`는 단계별 출력 상자 크기다(0 = 원래 설정 크기, 뒤로 갈수록 작다). 비어 있으면 단계 하나짜리로 본다.
    /// `now`는 인코더를 연 시각이다.
    pub fn new(config: QpScaleConfig, steps: Vec<(u32, u32)>, now: Instant) -> Self {
        let steps = if steps.is_empty() {
            vec![(0, 0)]
        } else {
            steps
        };
        Self {
            up_gap: config.up_gap,
            config,
            steps,
            index: 0,
            samples: VecDeque::new(),
            sum: 0.0,
            opened_at: Some(now),
            last_key: None,
            high_since: None,
            low_since: None,
            last_change: None,
            last_up: None,
        }
    }

    /// 지금 단계.
    pub fn index(&self) -> usize {
        self.index
    }

    /// 단계 수(1 이상).
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    /// `index`단계의 출력 상자 크기. 범위 밖이면 `None`.
    pub fn step_box(&self, index: usize) -> Option<(u32, u32)> {
        self.steps.get(index).copied()
    }

    /// 평균 창이 판정할 만큼 찼으면 그 평균 QP(통계 표시용). 모자라면 `None`.
    pub fn average_qp(&self) -> Option<f32> {
        self.average()
    }

    /// 지금 쓰는 올림 대기(재발하면 늘어난다).
    pub fn up_gap(&self) -> Duration {
        self.up_gap
    }

    /// 평균 창의 표본 수(시험·로그용).
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    /// 인코더를 (단계는 그대로) 다시 열었다. 표본과 이어지던 판정을 비우고 연 직후 구간을 센다.
    pub fn reset(&mut self, now: Instant) {
        self.samples.clear();
        self.sum = 0.0;
        self.opened_at = Some(now);
        self.last_key = None;
        self.high_since = None;
        self.low_since = None;
    }

    /// 호출자가 `to`단계로 인코더를 다시 열었다. 단계를 바꾸고 전환 시각·올림 대기를 갱신한 뒤 [`reset`](Self::reset)한다.
    /// 범위 밖 단계는 무시한다.
    pub fn switched(&mut self, to: usize, now: Instant) {
        if to >= self.steps.len() || to == self.index {
            self.reset(now);
            return;
        }
        if to > self.index {
            // 내림. 올린 지 얼마 안 됐으면 재발이므로 올림 대기를 2배로, 아니면 처음 값으로.
            let relapse = self
                .last_up
                .is_some_and(|up| now.saturating_duration_since(up) < self.config.relapse_window);
            self.up_gap = if relapse {
                (self.up_gap * 2).min(self.config.up_gap_max)
            } else {
                self.config.up_gap
            };
        } else {
            self.last_up = Some(now);
        }
        self.index = to;
        self.last_change = Some(now);
        self.reset(now);
    }

    /// 프레임 하나를 넣고 판정한다. 표본으로 쓰지 않는 프레임(키프레임 등)도 시간을 흘리므로 계속 불러 준다.
    pub fn push(&mut self, frame: QpFrame) -> Verdict {
        let now = frame.at;
        if frame.key {
            self.last_key = Some(now);
        }
        if self.counts(&frame) {
            self.samples.push_back((now, frame.qp));
            self.sum += f64::from(frame.qp);
        }
        self.trim(now);
        let next = self.evaluate(now);
        Verdict {
            index: self.index,
            next,
        }
    }

    /// 이 프레임을 표본으로 쓰는가.
    fn counts(&self, frame: &QpFrame) -> bool {
        if frame.key || !frame.qp.is_finite() {
            return false;
        }
        let after = |t: Option<Instant>, d: Duration| {
            t.is_some_and(|t| frame.at.saturating_duration_since(t) < d)
        };
        !(after(self.last_key, self.config.key_skip)
            || after(self.opened_at, self.config.open_skip))
    }

    /// 평균 창 밖의 옛 표본을 버린다.
    fn trim(&mut self, now: Instant) {
        while let Some(&(t, qp)) = self.samples.front() {
            if now.saturating_duration_since(t) < self.config.window {
                break;
            }
            self.samples.pop_front();
            self.sum -= f64::from(qp);
        }
        if self.samples.is_empty() {
            self.sum = 0.0;
        }
    }

    /// 평균 창이 판정할 만큼 찼으면 평균 QP.
    fn average(&self) -> Option<f32> {
        let (first, _) = self.samples.front()?;
        let (last, _) = self.samples.back()?;
        if self.samples.len() < self.config.min_samples
            || last.saturating_duration_since(*first) < self.config.min_span
        {
            return None;
        }
        Some((self.sum / self.samples.len() as f64) as f32)
    }

    fn evaluate(&mut self, now: Instant) -> Option<usize> {
        let Some(avg) = self.average() else {
            // 표본이 모자라면(멈춘 화면, 연 직후) 이어지던 판정을 끊는다.
            self.high_since = None;
            self.low_since = None;
            return None;
        };
        let th = self.config.thresholds;
        self.high_since = if avg >= th.high {
            self.high_since.or(Some(now))
        } else {
            None
        };
        self.low_since = if avg <= th.low {
            self.low_since.or(Some(now))
        } else {
            None
        };

        let held = |since: Option<Instant>, d: Duration| {
            since.is_some_and(|s| now.saturating_duration_since(s) >= d)
        };
        if held(self.high_since, self.config.down_hold) && self.index + 1 < self.steps.len() {
            return Some(self.index + 1);
        }
        let gap_passed = self
            .last_change
            .is_none_or(|c| now.saturating_duration_since(c) >= self.up_gap);
        if held(self.low_since, self.config.up_hold) && gap_passed && self.index > 0 {
            return Some(self.index - 1);
        }
        None
    }
}

/// 설정한 출력 상자(너비, 높이)에서 기본 단계 목록을 만든다(사용자 결정 2026-11-23, D-099: QHD→FHD 한 단계만).
///
/// 상자가 2560×1440(QHD)일 때만 [2560×1440, 1920×1080]이고, 그 밖(FHD 등)은 단계 하나라 판정이 아무것도 바꾸지 않는다.
/// 0이거나 상한([`VideoSettings::MAX_WIDTH`]×[`VideoSettings::MAX_HEIGHT`])보다 큰 축은 상한으로 본다(D-097, `VideoSettings::output_size`와 같은 규칙).
pub fn default_steps(width: u32, height: u32) -> Vec<(u32, u32)> {
    use crate::media::VideoSettings as V;
    let side = |v: u32, max: u32| if v == 0 || v > max { max } else { v };
    let (w, h) = (side(width, V::MAX_WIDTH), side(height, V::MAX_HEIGHT));
    if (w, h) == (V::MAX_WIDTH, V::MAX_HEIGHT) {
        vec![(w, h), (1920, 1080)]
    } else {
        vec![(w, h)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEP_MS: u64 = 50; // 20fps. 시각이 정확히 떨어지게 한다.
    const HIGH_QP: f32 = 45.0;
    const MID_QP: f32 = 30.0;
    const LOW_QP: f32 = 15.0;

    /// 시험용 설정: 연 직후·키프레임 뒤 제외를 끄고 정확한 시각을 본다.
    fn cfg() -> QpScaleConfig {
        QpScaleConfig {
            key_skip: Duration::ZERO,
            open_skip: Duration::ZERO,
            ..QpScaleConfig::new(VideoCodec::H264)
        }
    }

    fn steps() -> Vec<(u32, u32)> {
        default_steps(2560, 1440)
    }

    /// 시각 `ms`를 흘려 가며 프레임을 넣는 도구.
    struct Rig {
        base: Instant,
        ms: u64,
        s: QpScaler,
    }

    impl Rig {
        fn new(cfg: QpScaleConfig, steps: Vec<(u32, u32)>) -> Self {
            let base = Instant::now();
            Self {
                base,
                ms: 0,
                s: QpScaler::new(cfg, steps, base),
            }
        }

        fn now(&self) -> Instant {
            self.base + Duration::from_millis(self.ms)
        }

        fn frame(&mut self, qp: f32, key: bool) -> Verdict {
            let v = self.s.push(QpFrame {
                at: self.now(),
                qp,
                key,
            });
            self.ms += STEP_MS;
            v
        }

        /// `dur_ms`만큼 같은 QP를 넣는다. 처음으로 바꾸라는 답이 나오면 (그 시각 ms, 단계)로 멈춘다.
        fn run(&mut self, qp: f32, dur_ms: u64) -> Option<(u64, usize)> {
            let end = self.ms + dur_ms;
            while self.ms < end {
                let at = self.ms;
                if let Some(n) = self.frame(qp, false).next {
                    return Some((at, n));
                }
            }
            None
        }

        /// 바꾸라는 답을 받은 것으로 치고 인코더를 다시 연다.
        fn switch(&mut self, to: usize) {
            let now = self.now();
            self.s.switched(to, now);
        }
    }

    #[test]
    fn down_after_high_for_3s_not_at_2_9s() {
        // 표본 폭 1초가 차는 1.0초에 판정이 시작되고, 그 3초 뒤인 4.0초에 내린다.
        let mut r = Rig::new(cfg(), steps());
        assert_eq!(r.run(HIGH_QP, 3_950), None); // 판정 시작 뒤 2.9초(3.9초)까지는 안 내림
        assert_eq!(r.s.index(), 0);
        assert_eq!(r.run(HIGH_QP, 1_000), Some((4_000, 1)));
        // 답은 호출자가 switched를 부를 때까지 같다. 단계도 아직 안 바뀐다.
        assert_eq!(r.frame(HIGH_QP, false).next, Some(1));
        assert_eq!(r.s.index(), 0);
    }

    #[test]
    fn short_high_burst_does_not_count() {
        // 높음 2초 → 중간 → 높음 2초. 이어지지 않았으니 안 내린다.
        let mut r = Rig::new(cfg(), steps());
        assert_eq!(r.run(MID_QP, 1_500), None);
        assert_eq!(r.run(HIGH_QP, 2_000), None);
        assert_eq!(r.run(MID_QP, 4_000), None);
        assert_eq!(r.run(HIGH_QP, 2_000), None);
        assert_eq!(r.s.index(), 0);
    }

    #[test]
    fn keyframes_and_after_open_are_excluded() {
        // 기본 설정(키프레임 뒤 0.5초, 연 직후 1초 제외). 키프레임만 QP가 높고 나머지는 낮다.
        let mut r = Rig::new(QpScaleConfig::new(VideoCodec::H264), steps());
        let end = 20_000;
        while r.ms < end {
            let key = r.ms.is_multiple_of(2_000);
            let qp = if key || r.ms % 2_000 < 500 {
                HIGH_QP // 키프레임과 그 직후 0.5초는 QP가 높다
            } else {
                LOW_QP
            };
            let v = r.frame(qp, key);
            assert_eq!(v.next, None, "ms={}", r.ms);
        }
        assert_eq!(r.s.index(), 0);

        // 연 직후 1초는 높고 그 뒤는 낮다: reset 뒤 표본이 안 들어가므로 내리지 않는다.
        let mut r = Rig::new(QpScaleConfig::new(VideoCodec::H264), steps());
        assert_eq!(r.run(HIGH_QP, 1_000), None);
        assert_eq!(r.s.sample_count(), 0);
        assert_eq!(r.run(LOW_QP, 5_000), None);
    }

    #[test]
    fn high_qp_after_exclusion_counts_normally() {
        // 제외 구간이 지나면 같은 설정으로도 내려간다(제외가 판정을 영영 막지 않는다).
        let mut r = Rig::new(QpScaleConfig::new(VideoCodec::H264), steps());
        let got = r.run(HIGH_QP, 10_000).expect("내려가야 한다");
        // 연 직후 1초 제외 → 표본 시작 1.0초 → 판정 시작 2.0초 → 5.0초에 내림.
        assert_eq!(got, (5_000, 1));
    }

    #[test]
    fn sparse_frames_do_not_decide() {
        // 멈춘 화면: 1초에 한 프레임. 3초 창에 표본이 3개뿐이라 높은 QP여도 판정 안 함.
        let mut r = Rig::new(cfg(), steps());
        let mut n = 0;
        while r.ms < 60_000 {
            assert_eq!(r.frame(HIGH_QP, false).next, None);
            r.ms += 1_000 - STEP_MS;
            n += 1;
        }
        assert!(n > 50);
        assert_eq!(r.s.index(), 0);
        // 표본이 개수는 되지만(10개) 시간 폭이 1초 미만이어도 판정 안 함: 20fps로 0.45초만 넣고 멈춘다.
        let mut r = Rig::new(cfg(), steps());
        assert_eq!(r.run(HIGH_QP, 500), None);
        assert_eq!(r.s.sample_count(), 10);
        assert_eq!(r.s.average(), None);
    }

    #[test]
    fn gap_in_frames_breaks_the_streak() {
        // 높음이 이어지다 프레임이 4초 끊기면(멈춘 화면) 이어짐이 끊겨 다시 처음부터 센다.
        let mut r = Rig::new(cfg(), steps());
        assert_eq!(r.run(HIGH_QP, 3_000), None);
        r.ms += 4_000;
        // 끊긴 뒤 표본 폭 1초가 다시 차는 데 1초, 거기서 3초 더.
        let start = r.ms;
        let got = r.run(HIGH_QP, 6_000).expect("내려가야 한다");
        assert_eq!(got.0, start + 1_000 + 3_000);
    }

    #[test]
    fn up_needs_10s_low_and_15s_since_change() {
        let mut r = Rig::new(cfg(), steps());
        let (down_at, to) = r.run(HIGH_QP, 6_000).unwrap();
        assert_eq!((down_at, to), (4_000, 1));
        r.switch(1);
        let switched_at = r.ms;
        // 낮음은 1초 뒤 판정 시작, 거기서 10초 → 전환 후 11초에 낮음 조건 충족. 하지만 전환 후 15초가 지나야 한다.
        assert_eq!(r.run(LOW_QP, 14_000), None);
        let got = r.run(LOW_QP, 5_000).expect("올라가야 한다");
        assert_eq!(got, (switched_at + 15_000, 0));
    }

    #[test]
    fn up_needs_10s_low_streak_even_after_gap() {
        let mut r = Rig::new(cfg(), steps());
        r.run(HIGH_QP, 6_000).unwrap();
        r.switch(1);
        // 20초 동안 중간 QP(내림도 올림도 아님) → 15초는 지났다.
        assert_eq!(r.run(MID_QP, 20_000), None);
        let start = r.ms;
        // 3초 평균이 low 아래로 내려오는 데 1.2초(중간 30 → 낮음 15, 평균 24 이하는 낮음이 40%일 때),
        // 거기서 10초 이어져야 올린다. 낮음 10초만으로는 못 올린다.
        assert_eq!(r.run(LOW_QP, 10_950), None);
        let (at, to) = r.run(LOW_QP, 1_000).expect("올라가야 한다");
        assert_eq!(to, 0);
        assert!(
            (start + 11_000..=start + 11_500).contains(&at),
            "at={at} start={start}"
        );
    }

    #[test]
    fn relapse_doubles_up_gap_up_to_2min() {
        let mut r = Rig::new(cfg(), steps());
        assert_eq!(r.s.up_gap(), Duration::from_secs(15));
        r.run(HIGH_QP, 6_000).unwrap();
        r.switch(1);
        // 올린다(전환 후 15초).
        let (_, to) = r.run(LOW_QP, 20_000).unwrap();
        assert_eq!(to, 0);
        r.switch(0);
        // 올린 뒤 30초 안(여기서는 4초 뒤)에 다시 내려가면 대기가 2배.
        r.run(HIGH_QP, 10_000).unwrap();
        r.switch(1);
        assert_eq!(r.s.up_gap(), Duration::from_secs(30));
        // 이제 올리려면 전환 후 30초.
        let start = r.ms;
        assert_eq!(r.run(LOW_QP, 29_000), None);
        let (at, _) = r.run(LOW_QP, 3_000).unwrap();
        assert_eq!(at, start + 30_000);
        r.switch(0);
        // 계속 재발하면 60초, 120초(최대)에서 멈춘다.
        for want in [60, 120, 120, 120] {
            r.run(HIGH_QP, 10_000).unwrap();
            r.switch(1);
            assert_eq!(r.s.up_gap(), Duration::from_secs(want));
            let start = r.ms;
            let (at, _) = r.run(LOW_QP, 130_000).unwrap();
            assert_eq!(at, start + want * 1_000);
            r.switch(0);
        }
    }

    #[test]
    fn no_relapse_after_30s_resets_up_gap() {
        let mut r = Rig::new(cfg(), steps());
        r.run(HIGH_QP, 6_000).unwrap();
        r.switch(1);
        r.run(LOW_QP, 20_000).unwrap();
        r.switch(0);
        r.run(HIGH_QP, 10_000).unwrap();
        r.switch(1);
        assert_eq!(r.s.up_gap(), Duration::from_secs(30));
        r.run(LOW_QP, 40_000).unwrap();
        r.switch(0);
        // 올린 지 30초 넘게 버틴 뒤 내려가면 재발이 아니다 → 처음 값 15초.
        assert_eq!(r.run(MID_QP, 40_000), None);
        r.run(HIGH_QP, 10_000).unwrap();
        r.switch(1);
        assert_eq!(r.s.up_gap(), Duration::from_secs(15));
    }

    #[test]
    fn stops_at_last_and_first_step() {
        // 단계 셋: 마지막 단계에서 더 안 내려간다.
        let three = vec![(2560, 1440), (1920, 1080), (1280, 720)];
        let mut r = Rig::new(cfg(), three);
        assert_eq!(r.s.step_count(), 3);
        r.run(HIGH_QP, 6_000).unwrap();
        r.switch(1);
        let (_, to) = r.run(HIGH_QP, 6_000).unwrap();
        assert_eq!(to, 2);
        r.switch(2);
        assert_eq!(r.run(HIGH_QP, 30_000), None);
        assert_eq!(r.s.index(), 2);
        // 올라갈 때도 한 단계씩, 단계 0에서 더 안 올라간다.
        assert_eq!(r.run(LOW_QP, 40_000).unwrap().1, 1);
        r.switch(1);
        assert_eq!(r.run(LOW_QP, 40_000).unwrap().1, 0);
        r.switch(0);
        assert_eq!(r.run(LOW_QP, 60_000), None);
        assert_eq!(r.s.index(), 0);
    }

    #[test]
    fn single_step_never_changes() {
        // FHD로 고른 방송: 단계가 하나뿐이라 아무것도 안 바뀐다.
        let mut r = Rig::new(cfg(), default_steps(1920, 1080));
        assert_eq!(r.s.step_count(), 1);
        assert_eq!(r.run(HIGH_QP, 30_000), None);
        assert_eq!(r.run(LOW_QP, 30_000), None);
        assert_eq!(r.s.index(), 0);
    }

    #[test]
    fn codecs_use_their_own_thresholds() {
        // QP 100: H.264(0~51)에서는 한참 높지만 AV1(0~255, high 205)에서는 중간이다.
        let mut h = Rig::new(
            QpScaleConfig {
                key_skip: Duration::ZERO,
                open_skip: Duration::ZERO,
                ..QpScaleConfig::new(VideoCodec::H264)
            },
            steps(),
        );
        assert!(h.run(100.0, 10_000).is_some());
        let mut a = Rig::new(
            QpScaleConfig {
                key_skip: Duration::ZERO,
                open_skip: Duration::ZERO,
                ..QpScaleConfig::new(VideoCodec::Av1)
            },
            steps(),
        );
        assert_eq!(a.run(100.0, 10_000), None);
        // AV1도 205 이상이면 내린다.
        assert!(a.run(220.0, 10_000).is_some());
        // 임계값 경계: 평균이 low와 high 사이면 아무 일도 없다.
        let t = QpThresholds::default_for(VideoCodec::H264);
        assert!(t.low < t.high && t.high <= 51.0);
        let t = QpThresholds::default_for(VideoCodec::Av1);
        assert!(t.low < t.high && t.high <= 255.0);
    }

    #[test]
    fn reset_clears_samples() {
        let mut r = Rig::new(cfg(), steps());
        r.run(HIGH_QP, 2_500).unwrap_or_default();
        assert!(r.s.sample_count() > 0);
        let now = r.now();
        r.s.reset(now);
        assert_eq!(r.s.sample_count(), 0);
        assert_eq!(r.s.index(), 0);
        // 이어지던 높음도 사라져, 다시 1초 + 3초가 필요하다.
        let start = r.ms;
        assert_eq!(r.run(HIGH_QP, 3_950), None);
        assert_eq!(r.run(HIGH_QP, 1_000), Some((start + 4_000, 1)));
    }

    #[test]
    fn switched_clears_samples_and_sets_index() {
        let mut r = Rig::new(cfg(), steps());
        r.run(HIGH_QP, 6_000).unwrap();
        r.switch(1);
        assert_eq!(r.s.index(), 1);
        assert_eq!(r.s.sample_count(), 0);
        assert_eq!(r.s.step_box(1), Some((1920, 1080)));
        assert_eq!(r.s.step_box(2), None);
        // 범위 밖 단계는 단계를 안 바꾼다.
        r.switch(7);
        assert_eq!(r.s.index(), 1);
    }

    #[test]
    fn non_finite_qp_is_ignored() {
        let mut r = Rig::new(cfg(), steps());
        for _ in 0..100 {
            assert_eq!(r.frame(f32::NAN, false).next, None);
        }
        assert_eq!(r.s.sample_count(), 0);
    }

    #[test]
    fn default_steps_only_qhd_goes_down() {
        assert_eq!(default_steps(2560, 1440), vec![(2560, 1440), (1920, 1080)]);
        // 0이나 상한 초과는 상한(QHD)으로 본다.
        assert_eq!(default_steps(0, 0), vec![(2560, 1440), (1920, 1080)]);
        assert_eq!(default_steps(3840, 2160), vec![(2560, 1440), (1920, 1080)]);
        // FHD·HD·그 밖의 상자는 단계 하나.
        assert_eq!(default_steps(1920, 1080), vec![(1920, 1080)]);
        assert_eq!(default_steps(1280, 720), vec![(1280, 720)]);
        assert_eq!(default_steps(2560, 1080), vec![(2560, 1080)]);
    }

    #[test]
    fn average_qp_needs_a_full_enough_window() {
        let mut r = Rig::new(cfg(), steps());
        assert_eq!(r.s.average_qp(), None);
        for _ in 0..3 {
            r.frame(30.0, false);
        }
        assert_eq!(r.s.average_qp(), None, "표본이 모자라면 평균을 내지 않는다");
        for _ in 0..40 {
            r.frame(30.0, false);
        }
        assert!((r.s.average_qp().unwrap() - 30.0).abs() < 0.01);
    }
}
