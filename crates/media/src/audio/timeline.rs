//! 오디오 샘플 시간축.
//!
//! 파이프라인이 시작될 때의 QPC(`base_qpc`)를 샘플 0으로 두고, 48kHz 샘플 번호와
//! QPC를 서로 바꾼다. `base_qpc`는 625(= 62.5µs = 3샘플)의 배수로 맞추므로
//! 3의 배수 샘플은 정확히 정수 QPC에 놓이고, 모든 정초(QPC가 10^7의 배수)는
//! 정확히 어떤 샘플 위에 놓인다.
//!
//! 캡처 데이터는 버퍼마다 붙은 QPC 위치를 보고 시간축에 맞춰 넣는다. 간격이 생기면
//! 무음으로 채우고, 이미 무음으로 채운 구간과 겹치면 겹친 부분을 버린다.
//! 캡처가 아무것도 주지 않을 때(루프백은 재생 중인 소리가 없으면 패킷이 없다)도
//! 벽시계에 맞춰 무음을 채워 스트림이 끊기지 않게 한다.

pub(crate) const SAMPLE_RATE: i64 = 48_000;
pub(crate) const CHANNELS: usize = 2;
/// Opus 패킷 하나(10ms, D-080. 예전 20ms = 960)의 샘플 수(채널당).
pub(crate) const FRAME_SAMPLES: usize = 480;
/// 정렬 단위: 625 × 100ns = 62.5µs = 정확히 3샘플.
const ALIGN_QPC: i64 = 625;

/// QPC를 625의 배수로 내림한다.
pub(crate) fn align_base_qpc(qpc: i64) -> i64 {
    qpc - qpc.rem_euclid(ALIGN_QPC)
}

/// 샘플 번호 → QPC(내림). `idx`가 3의 배수면 정확하다.
pub(crate) fn qpc_of_sample(base_qpc: i64, idx: i64) -> i64 {
    base_qpc + (idx as i128 * ALIGN_QPC as i128).div_euclid(3) as i64
}

/// QPC → 그 시각 이전(포함)의 마지막 샘플 번호.
pub(crate) fn sample_at_qpc(base_qpc: i64, qpc: i64) -> i64 {
    ((qpc - base_qpc) as i128 * 3).div_euclid(ALIGN_QPC as i128) as i64
}

/// QPC → 가장 가까운 샘플 번호.
fn nearest_sample(base_qpc: i64, qpc: i64) -> i64 {
    ((qpc - base_qpc) as i128 * 6 + ALIGN_QPC as i128).div_euclid(2 * ALIGN_QPC as i128) as i64
}

/// 캡처 데이터를 넣은 결과(통계용).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PushOutcome {
    /// 앞에 채운 무음 샘플 수
    pub silence: u64,
    /// 겹쳐서 버린 샘플 수
    pub dropped: u64,
}

pub(crate) struct Timeline {
    base_qpc: i64,
    /// 지금까지 시간축에 넣은 샘플 수(= 다음 샘플 번호)
    written: u64,
    /// 아직 패킷으로 만들지 않은 샘플(스테레오 인터리브)
    pending: Vec<f32>,
    /// 이 이상 어긋나면 무음을 넣거나 겹친 부분을 버린다(샘플 수)
    tolerance: i64,
}

impl Timeline {
    /// 허용 오차 20ms. 오디오 장치 클럭과 QPC의 차이(드리프트)는 이 안에서 흡수한다.
    pub const TOLERANCE_SAMPLES: i64 = 960;

    pub fn new(base_qpc: i64) -> Self {
        Timeline {
            base_qpc,
            written: 0,
            pending: Vec::with_capacity(FRAME_SAMPLES * CHANNELS * 4),
            tolerance: Self::TOLERANCE_SAMPLES,
        }
    }

    pub fn base_qpc(&self) -> i64 {
        self.base_qpc
    }

    /// 다음에 넣을 샘플 번호.
    #[cfg(test)]
    pub fn written(&self) -> u64 {
        self.written
    }

    fn append_silence(&mut self, n: u64) {
        self.pending
            .resize(self.pending.len() + n as usize * CHANNELS, 0.0);
        self.written += n;
    }

    fn append(&mut self, samples: &[f32]) {
        self.pending.extend_from_slice(samples);
        self.written += (samples.len() / CHANNELS) as u64;
    }

    /// 캡처 버퍼를 넣는다. `qpc`는 버퍼 첫 샘플의 QPC 위치(없으면 이어 붙인다).
    pub fn push(&mut self, qpc: Option<i64>, samples: &[f32]) -> PushOutcome {
        let n = (samples.len() / CHANNELS) as i64;
        let mut out = PushOutcome::default();
        let Some(qpc) = qpc else {
            self.append(samples);
            return out;
        };
        let delta = nearest_sample(self.base_qpc, qpc) - self.written as i64;
        if delta > self.tolerance {
            self.append_silence(delta as u64);
            out.silence = delta as u64;
            self.append(samples);
        } else if delta < -self.tolerance {
            let skip = (-delta).min(n);
            out.dropped = skip as u64;
            self.append(&samples[skip as usize * CHANNELS..]);
        } else {
            self.append(samples);
        }
        out
    }

    /// `qpc` 시각까지 무음을 채운다(이미 넘었으면 아무것도 하지 않는다). 채운 샘플 수.
    pub fn fill_silence_until(&mut self, qpc: i64) -> u64 {
        let target = sample_at_qpc(self.base_qpc, qpc);
        let n = target - self.written as i64;
        if n > 0 {
            self.append_silence(n as u64);
            n as u64
        } else {
            0
        }
    }

    /// `qpc` 시각까지 `gen(샘플 번호)`로 만든 샘플을 채운다(양 채널 같은 값).
    pub fn generate_until(&mut self, qpc: i64, mut generator: impl FnMut(u64) -> f32) {
        let target = sample_at_qpc(self.base_qpc, qpc);
        while (self.written as i64) < target {
            let v = generator(self.written);
            self.pending.extend_from_slice(&[v, v]);
            self.written += 1;
        }
    }

    /// `FRAME_SAMPLES`샘플이 모였으면 패킷 하나를 꺼낸다: (첫 샘플 번호, 스테레오 샘플 `FRAME_SAMPLES` × 2개).
    pub fn pop_packet(&mut self) -> Option<(u64, Vec<f32>)> {
        let len = FRAME_SAMPLES * CHANNELS;
        if self.pending.len() < len {
            return None;
        }
        let first = self.written - (self.pending.len() / CHANNELS) as u64;
        let rest = self.pending.split_off(len);
        let packet = std::mem::replace(&mut self.pending, rest);
        Some((first, packet))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: i64 = 10_000_000;

    #[test]
    fn sample_qpc_conversion_is_exact_on_seconds() {
        let base = align_base_qpc(123_456_789_012);
        assert_eq!(base % 625, 0);
        // 다음 정초가 정확히 샘플 위에 놓인다
        let boundary = (base / SEC + 1) * SEC;
        let idx = sample_at_qpc(base, boundary);
        assert_eq!(qpc_of_sample(base, idx), boundary);
        assert_eq!(qpc_of_sample(base, 48_000), base + SEC);
        assert_eq!(qpc_of_sample(base, 960), base + 200_000);
        assert_eq!(sample_at_qpc(base, base + 208), 0);
        assert_eq!(sample_at_qpc(base, base + 209), 1);
        assert_eq!(nearest_sample(base, base + 100), 0);
        assert_eq!(nearest_sample(base, base + 110), 1);
    }

    #[test]
    fn packets_are_contiguous() {
        let base = 0;
        let mut t = Timeline::new(base);
        let buf = vec![0.5f32; 480 * CHANNELS];
        for k in 0..5 {
            t.push(Some(qpc_of_sample(base, k * 480)), &buf);
        }
        let n = FRAME_SAMPLES as u64;
        let firsts: Vec<u64> = std::iter::from_fn(|| t.pop_packet())
            .map(|(i, p)| {
                assert_eq!(p.len(), FRAME_SAMPLES * CHANNELS);
                i
            })
            .collect();
        assert_eq!(firsts, (0..2400 / n).map(|k| k * n).collect::<Vec<_>>());
        assert!(t.pop_packet().is_none());
        assert_eq!(t.written(), 2400);
    }

    #[test]
    fn gap_is_filled_with_silence() {
        let mut t = Timeline::new(0);
        let buf = vec![1.0f32; 480 * CHANNELS];
        t.push(Some(0), &buf);
        // 100ms 뒤에 다음 데이터
        let o = t.push(Some(SEC / 10), &buf);
        assert_eq!(o.silence, 4800 - 480);
        assert_eq!(t.written(), 4800 + 480);
        // 작은 지터(5ms)는 흡수한다
        let o = t.push(Some(SEC / 10 + 10 * 10_000 + 50_000), &buf);
        assert_eq!(o, PushOutcome::default());
    }

    #[test]
    fn overlap_after_silence_fill_is_dropped() {
        let mut t = Timeline::new(0);
        // 캡처가 없어서 100ms까지 무음을 채웠다
        assert_eq!(t.fill_silence_until(SEC / 10), 4800);
        assert_eq!(t.fill_silence_until(SEC / 20), 0);
        // 늦게 도착한 60ms 시점 데이터 960샘플: 앞 40ms(1920샘플 중 960샘플만 있음)는 버린다
        let buf = vec![1.0f32; 960 * CHANNELS];
        let o = t.push(Some(SEC * 6 / 100), &buf);
        assert_eq!(o.dropped, 960);
        assert_eq!(t.written(), 4800);
        // 지속 데이터는 다시 이어 붙는다
        let o = t.push(Some(SEC / 10), &buf);
        assert_eq!(o, PushOutcome::default());
        assert_eq!(t.written(), 5760);
    }

    #[test]
    fn generate_until_matches_clock() {
        let mut t = Timeline::new(0);
        t.generate_until(SEC / 50, |i| i as f32);
        assert_eq!(t.written(), 960);
        let (first, p) = t.pop_packet().unwrap();
        assert_eq!(first, 0);
        assert_eq!(p[2], 1.0);
        assert_eq!(p[3], 1.0);
    }
}
