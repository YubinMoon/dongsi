//! 소리 증폭(방송자가 정한 배율)과 깨짐 방지 리미터.
//!
//! 게임 소리는 Discord 목소리보다 10 dB 넘게 작게 나오는 일이 흔하다(롤 실측: 최대 순간 크기 -19 dBFS, Discord -6 dBFS).
//! 방송자가 방송 중 화면에서 배율(100~800%)을 정하면 Opus로 보내기 직전에 곱한다.
//!
//! - 배율이 바뀌면 패킷(10ms) 안에서 선형으로 옮겨 가서 툭 튀는 소리가 나지 않는다.
//! - 키운 소리가 [`CEILING`]을 넘으면 넘는 만큼만 배율을 줄인다(그 패킷 첫 샘플부터 줄이고 [`RELEASE_SECS`]에 걸쳐 되돌린다).
//!   리미터는 **키운 몫만** 줄이므로 결과가 원래 소리보다 작아지지 않는다.
//! - 배율 1(100%)이면 샘플을 건드리지 않는다.

use super::timeline::{CHANNELS, SAMPLE_RATE};

/// 배율 상한(800%).
pub const MAX_AUDIO_BOOST: f32 = 8.0;
/// 리미터 문턱(약 -0.3 dBFS).
const CEILING: f32 = 0.966;
/// 리미터가 줄인 배율을 되돌리는 시간 상수(초).
const RELEASE_SECS: f32 = 0.2;

/// 배율을 1~[`MAX_AUDIO_BOOST`]로 맞춘다. 숫자가 아니면 1.
pub(crate) fn clamp_boost(gain: f32) -> f32 {
    if gain.is_finite() {
        gain.clamp(1.0, MAX_AUDIO_BOOST)
    } else {
        1.0
    }
}

pub(crate) struct Boost {
    /// 지난 패킷 끝의 배율
    gain: f32,
    /// 지난 패킷 끝의 리미터 몫(1 이하). 넘칠 패킷이 오면 바로 줄이고 [`RELEASE_SECS`]에 걸쳐 1로 되돌린다.
    limit: f32,
}

impl Boost {
    pub fn new() -> Self {
        Boost {
            gain: 1.0,
            limit: 1.0,
        }
    }

    /// 스테레오 인터리브 `pcm`(패킷 하나)을 `target` 배율로 키운다. 배율은 지난 패킷의 값에서 이 패킷 끝까지 선형으로 옮겨 간다.
    ///
    /// 리미터는 패킷 전체의 최댓값을 먼저 보고 몫을 정한다. 패킷을 이미 다 들고 있으므로 지연을 더하지 않고 10ms 앞을 보는 셈이라,
    /// 줄일 때는 패킷 첫 샘플부터 줄여 큰 소리의 앞머리가 깎이지 않는다.
    pub fn process(&mut self, target: f32, pcm: &mut [f32]) {
        let target = clamp_boost(target);
        let from = self.gain;
        self.gain = target;
        if from <= 1.0 && target <= 1.0 {
            self.limit = 1.0;
            return;
        }
        let frames = (pcm.len() / CHANNELS).max(1) as f32;
        let peak = pcm.iter().fold(0f32, |m, v| m.max(v.abs())) * from.max(target);
        let need = if peak > CEILING { CEILING / peak } else { 1.0 };
        let (lim_from, lim_to) = if need < self.limit {
            (need, need)
        } else {
            let recover = (frames / (RELEASE_SECS * SAMPLE_RATE as f32)).exp();
            (self.limit, need.min(self.limit * recover))
        };
        self.limit = lim_to;
        for (i, frame) in pcm.as_chunks_mut::<CHANNELS>().0.iter_mut().enumerate() {
            let t = (i + 1) as f32 / frames;
            let g = (from + (target - from) * t) * (lim_from + (lim_to - lim_from) * t);
            // 리미터는 키운 몫만 줄인다(원래보다 작게 만들지 않는다).
            let g = g.max(1.0);
            for v in frame {
                *v *= g;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::timeline::FRAME_SAMPLES;

    /// 1kHz 사인 패킷 하나(양쪽 채널 같음). `k`번째 패킷.
    fn sine(amp: f32, k: usize) -> Vec<f32> {
        (0..FRAME_SAMPLES)
            .flat_map(|i| {
                let t = (k * FRAME_SAMPLES + i) as f32 / SAMPLE_RATE as f32;
                let v = amp * (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
                [v, v]
            })
            .collect()
    }

    fn peak(p: &[f32]) -> f32 {
        p.iter().fold(0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn unity_leaves_samples_untouched() {
        let mut b = Boost::new();
        for k in 0..5 {
            let input = sine(0.9, k);
            let mut out = input.clone();
            b.process(1.0, &mut out);
            assert_eq!(out, input);
        }
    }

    #[test]
    fn quiet_sound_is_boosted_by_target() {
        let mut b = Boost::new();
        let mut last = Vec::new();
        for k in 0..20 {
            last = sine(0.05, k);
            b.process(4.0, &mut last);
        }
        let p = peak(&last);
        assert!((p - 0.2).abs() < 0.002, "peak {p}");
    }

    #[test]
    fn gain_change_ramps_within_one_packet() {
        let mut b = Boost::new();
        let mut pcm = vec![0.01f32; FRAME_SAMPLES * CHANNELS];
        b.process(4.0, &mut pcm);
        for (i, f) in pcm.as_chunks::<CHANNELS>().0.iter().enumerate() {
            let want = 0.01 * (1.0 + 3.0 * (i + 1) as f32 / FRAME_SAMPLES as f32);
            assert!((f[0] - want).abs() < 1e-6, "frame {i}: {} != {want}", f[0]);
        }
        // 다음 패킷은 4배 그대로
        let mut pcm = vec![0.01f32; FRAME_SAMPLES * CHANNELS];
        b.process(4.0, &mut pcm);
        assert!(pcm.iter().all(|v| (v - 0.04).abs() < 1e-6));
    }

    #[test]
    fn limiter_keeps_peaks_under_ceiling_and_never_below_original() {
        let mut b = Boost::new();
        for k in 0..50 {
            let input = sine(0.5, k);
            let mut out = input.clone();
            b.process(8.0, &mut out);
            for (o, i) in out.iter().zip(&input) {
                assert!(o.abs() <= CEILING + 1e-5, "packet {k}: {o} > ceiling");
                assert!(
                    o.abs() >= i.abs() - 1e-6,
                    "packet {k}: {o} quieter than {i}"
                );
            }
        }
    }

    #[test]
    fn already_loud_input_passes_unchanged() {
        let mut b = Boost::new();
        b.process(2.0, &mut sine(0.1, 0));
        for k in 1..10 {
            let input = sine(1.2, k);
            let mut out = input.clone();
            b.process(2.0, &mut out);
            assert_eq!(out, input, "packet {k}");
        }
    }

    #[test]
    fn target_is_clamped() {
        assert_eq!(clamp_boost(100.0), MAX_AUDIO_BOOST);
        assert_eq!(clamp_boost(0.5), 1.0);
        assert_eq!(clamp_boost(f32::NAN), 1.0);
        let mut b = Boost::new();
        let input = vec![0.001f32; FRAME_SAMPLES * CHANNELS];
        let mut out = input.clone();
        b.process(0.2, &mut out);
        assert_eq!(out, input);
    }
}
