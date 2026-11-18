//! A/V 동기 측정용 테스트 톤.
//!
//! 규칙: 무음이다가, 매 정초(QPC가 10^7의 배수, 즉 정확히 1초 경계)마다
//! 1000Hz 사인파를 100ms(4800샘플) 동안 낸다. 진폭은 0.5(-6 dBFS).
//!
//! 시간축의 `base_qpc`가 625의 배수라서 모든 정초는 정확히 한 샘플 위에 놓인다.
//! 그 샘플(시각 == 정초)이 버스트의 첫 샘플이고 값은 `sin(0) = 0`이며,
//! 버스트 안의 n번째 샘플 값은 `0.5 · sin(2π · 1000 · n / 48000)`이다.
//! 테스트 패턴 비디오는 같은 정초에 흰 프레임을 내므로, 시청 쪽에서
//! 흰 화면과 삐 소리의 도착 시각 차이로 A/V 어긋남을 잴 수 있다.

use super::timeline::SAMPLE_RATE;

pub(crate) const TONE_HZ: f64 = 1000.0;
pub(crate) const TONE_AMPLITUDE: f32 = 0.5;
/// 버스트 길이(샘플): 100ms
pub(crate) const TONE_SAMPLES: i64 = SAMPLE_RATE / 10;

const SEC: i128 = 10_000_000;

/// 샘플 `idx`가 가장 최근 정초로부터 몇 번째 샘플인지(`base_qpc`는 625의 배수).
pub(crate) fn samples_since_second(base_qpc: i64, idx: u64) -> i64 {
    // 3 × 샘플 시각 = 3·base + 625·idx (정수, 정확)
    let t3 = base_qpc as i128 * 3 + idx as i128 * 625;
    let second = t3.div_euclid(3 * SEC);
    let boundary_idx = (second * 3 * SEC - base_qpc as i128 * 3) / 625;
    (idx as i128 - boundary_idx) as i64
}

/// 샘플 `idx`의 테스트 톤 값.
pub(crate) fn tone_sample(base_qpc: i64, idx: u64) -> f32 {
    let n = samples_since_second(base_qpc, idx);
    if (0..TONE_SAMPLES).contains(&n) {
        let phase = 2.0 * std::f64::consts::PI * TONE_HZ * n as f64 / SAMPLE_RATE as f64;
        TONE_AMPLITUDE * phase.sin() as f32
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::super::timeline::{align_base_qpc, qpc_of_sample, sample_at_qpc};
    use super::*;

    #[test]
    fn burst_starts_exactly_on_second() {
        let base = align_base_qpc(98_765_432_109);
        let boundary = (base / 10_000_000 + 1) * 10_000_000;
        let i0 = sample_at_qpc(base, boundary) as u64;
        assert_eq!(qpc_of_sample(base, i0 as i64), boundary);
        assert_eq!(samples_since_second(base, i0), 0);
        assert_eq!(tone_sample(base, i0), 0.0);
        // 직전 샘플은 무음(이전 버스트에서 한참 지남)
        assert_eq!(tone_sample(base, i0 - 1), 0.0);
        // 버스트 안: 12샘플(1/4 주기) 뒤가 최대
        assert!((tone_sample(base, i0 + 12) - TONE_AMPLITUDE).abs() < 1e-6);
        assert!(tone_sample(base, i0 + 1) > 0.0);
        // 100ms 뒤부터 다시 무음
        assert!(tone_sample(base, i0 + 4799).abs() > 0.0);
        assert_eq!(tone_sample(base, i0 + 4800), 0.0);
        // 다음 정초
        assert_eq!(samples_since_second(base, i0 + 48_000), 0);
    }
}
