//! 측정용: 프로세스 트리의 소리에서 테스트 톤 삐의 시작 시각(QPC)을 모은다.
//!
//! 시청 앱(WebView2가 앱 프로세스의 자식이다)이 실제로 소리를 내보내는 시각을 재려고 쓴다.
//! WASAPI 프로세스 루프백(INCLUDE, 또는 장치 루프백)으로 48kHz 스테레오 float를 받아, 버퍼마다 붙은 QPC 위치로
//! 샘플 시각을 계산하고, 조용한 구간(50ms) 뒤에 처음 문턱을 넘은 샘플을 삐의 시작으로 본다.
//! 시각은 [`crate::clock`]과 같은 QPC 100ns 단위라 비디오 캡처 시각·테스트 톤의 정초와 같은 축이다.
//!
//! API 안정성을 보장하지 않는다(개발용 명령과 예제에서만 쓴다).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::MediaError;
use crate::audio::capture::{LoopbackCapture, Target, default_render_device_id};
use crate::audio::timeline::{CHANNELS, SAMPLE_RATE};
use crate::clock::qpc_now_100ns;

/// 삐 시작으로 보는 문턱(절댓값). 루프백 음량을 모르므로 낮게 두고 조용한 구간 조건으로 잡음을 거른다.
pub const DEFAULT_THRESHOLD: f32 = 0.02;
/// 삐 앞에 있어야 하는 조용한 구간(샘플): 50ms
pub const QUIET_SAMPLES: u64 = (SAMPLE_RATE / 20) as u64;
/// 버퍼 사이의 시각 간격이 이보다 크면 소리가 없던 구간으로 센다(100ns 단위, 1ms).
const GAP_QPC: i64 = 10_000;

/// 100ns 단위의 간격을 48kHz 샘플 수로(내림).
fn qpc_to_samples(d: i64) -> u64 {
    (d.max(0) as i128 * 3 / 625) as u64
}

/// 샘플 `k`개 뒤의 QPC 오프셋(100ns, 내림).
fn samples_to_qpc(k: u64) -> i64 {
    (k as i128 * 625 / 3) as i64
}

fn peak_of(frame: &[f32]) -> f32 {
    frame.iter().fold(0f32, |m, v| m.max(v.abs()))
}

/// 삐 시작 감지기(순수 로직). 버퍼를 차례로 넣으면 삐 시작의 QPC를 모아 준다.
#[derive(Debug, Clone)]
pub struct BeepDetector {
    threshold: f32,
    /// 마지막으로 문턱을 넘은 샘플 뒤로 지난 샘플 수
    quiet: u64,
    /// 다음 버퍼가 이어진다면 그 첫 샘플의 QPC
    next_qpc: Option<i64>,
}

impl BeepDetector {
    pub fn new(threshold: f32) -> Self {
        BeepDetector {
            threshold,
            quiet: 0,
            next_qpc: None,
        }
    }

    /// 버퍼 하나를 넣는다. `qpc`는 첫 샘플의 QPC 위치(없으면 앞 버퍼에 이어 붙인다),
    /// `samples`는 스테레오 인터리브. 찾은 삐 시작의 QPC를 `out`에 더한다.
    ///
    /// 버퍼 사이에 시각이 1ms 넘게 벌어지면(루프백은 소리가 없으면 패킷을 주지 않는다) 그 간격을
    /// 조용한 구간으로 센다. 시각을 전혀 모르는 처음 버퍼는 감지하지 않고 상태만 이어 간다.
    pub fn push(&mut self, qpc: Option<i64>, samples: &[f32], out: &mut Vec<i64>) {
        let first = match (qpc, self.next_qpc) {
            (Some(q), Some(next)) => {
                if q - next > GAP_QPC {
                    self.quiet = self.quiet.saturating_add(qpc_to_samples(q - next));
                }
                Some(q)
            }
            (Some(q), None) => Some(q),
            (None, next) => next,
        };
        let mut frames = 0u64;
        for (i, s) in samples.chunks_exact(CHANNELS).enumerate() {
            frames += 1;
            if peak_of(s) > self.threshold {
                if self.quiet >= QUIET_SAMPLES
                    && let Some(first) = first
                {
                    out.push(first + samples_to_qpc(i as u64));
                }
                self.quiet = 0;
            } else {
                self.quiet = self.quiet.saturating_add(1);
            }
        }
        self.next_qpc = first.map(|f| f + samples_to_qpc(frames));
    }
}

/// [`ToneProbe::take`]가 넘기는 결과.
#[derive(Debug, Clone, Default)]
pub struct ProbeTake {
    /// 새로 찾은 삐 시작(QPC 100ns)
    pub beeps: Vec<i64>,
    /// 지금까지 받은 샘플 수(채널당). 0이면 루프백에서 아무것도 안 온 것이다.
    pub samples: u64,
    /// 지금까지 본 가장 큰 절댓값(루프백 음량 확인용)
    pub peak: f32,
    /// 마지막 캡처 오류
    pub last_error: Option<String>,
}

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    inner: Mutex<ProbeTake>,
}

fn lock(m: &Mutex<ProbeTake>) -> MutexGuard<'_, ProbeTake> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 프로세스 트리의 소리에서 삐 시작 시각을 모으는 스레드.
pub struct ToneProbe {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

/// 무엇의 소리를 받을지.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeTarget {
    /// 이 프로세스와 그 자식 프로세스들(프로세스 루프백 INCLUDE)
    Process(u32),
    /// 기본 출력 장치 전체(장치 루프백). 다른 프로그램 소리도 섞인다.
    Device,
}

impl ProbeTarget {
    fn open(self) -> Result<LoopbackCapture, MediaError> {
        match self {
            ProbeTarget::Process(pid) => LoopbackCapture::open(Target::Include(pid)),
            ProbeTarget::Device => {
                LoopbackCapture::open(Target::Device(default_render_device_id()?))
            }
        }
    }
}

impl ToneProbe {
    /// 대상의 소리를 루프백으로 받기 시작한다.
    pub fn start(target: ProbeTarget, threshold: f32) -> Result<Self, MediaError> {
        let shared = Arc::new(Shared::default());
        let (init_tx, init_rx) = mpsc::sync_channel(1);
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("dongsi-tone-probe".into())
            .spawn(move || {
                let com = wasapi::initialize_mta().is_ok();
                match target.open() {
                    Ok(c) => {
                        let _ = init_tx.send(Ok(()));
                        run(c, target, threshold, &s);
                    }
                    Err(e) => {
                        let _ = init_tx.send(Err(e));
                    }
                }
                if com {
                    wasapi::deinitialize();
                }
            })
            .map_err(|e| MediaError::Thread(e.to_string()))?;
        match init_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(ToneProbe {
                shared,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                shared.stop.store(true, Ordering::SeqCst);
                Err(MediaError::Thread(
                    "측정 스레드가 시작되지 않았습니다".into(),
                ))
            }
        }
    }

    /// 지금까지 찾은 삐를 넘기고 비운다(샘플 수·최댓값은 누적).
    pub fn take(&self) -> ProbeTake {
        let mut g = lock(&self.shared.inner);
        ProbeTake {
            beeps: std::mem::take(&mut g.beeps),
            samples: g.samples,
            peak: g.peak,
            last_error: g.last_error.clone(),
        }
    }

    /// 멈추고 스레드가 끝날 때까지 기다린다.
    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for ToneProbe {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(capture: LoopbackCapture, target: ProbeTarget, threshold: f32, shared: &Shared) {
    let mut detector = BeepDetector::new(threshold);
    let mut found = Vec::new();
    let mut capture = Some(capture);
    let mut reopen_at = Instant::now();
    while !shared.stop.load(Ordering::SeqCst) {
        if capture.is_none() && Instant::now() >= reopen_at {
            match target.open() {
                Ok(c) => capture = Some(c),
                Err(e) => {
                    lock(&shared.inner).last_error = Some(e.to_string());
                    reopen_at = Instant::now() + Duration::from_secs(1);
                }
            }
        }
        if let Some(c) = &mut capture {
            let mut samples = 0u64;
            let mut peak = 0f32;
            let r = c.read(|qpc, s| {
                let n = (s.len() / CHANNELS) as u64;
                // QPC가 없는 버퍼는 지금 시각에서 버퍼 길이를 뺀 값으로 대신한다(대략).
                let qpc = qpc.or_else(|| Some(qpc_now_100ns() - samples_to_qpc(n)));
                detector.push(qpc, s, &mut found);
                samples += n;
                peak = peak.max(peak_of(s));
            });
            let mut g = lock(&shared.inner);
            g.samples += samples;
            g.peak = g.peak.max(peak);
            g.beeps.append(&mut found);
            if let Err(e) = r {
                g.last_error = Some(e.to_string());
                drop(g);
                capture = None;
                reopen_at = Instant::now() + Duration::from_millis(500);
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: i64 = 10_000_000;

    /// 정초에서 시작하는 100ms 1kHz 삐(진폭 0.5)의 샘플 값. `n`은 삐 안의 샘플 번호.
    fn tone(n: u64) -> f32 {
        if n < 4800 {
            0.5 * (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / 48_000.0).sin()
        } else {
            0.0
        }
    }

    /// `total`샘플의 모노 신호(스테레오로 늘림). `beeps`는 삐가 시작하는 샘플 번호.
    fn signal(total: usize, beeps: &[usize]) -> Vec<f32> {
        let mut v = vec![0f32; total * 2];
        for &b in beeps {
            for n in 0..4800usize {
                if b + n < total {
                    let x = tone(n as u64);
                    v[(b + n) * 2] = x;
                    v[(b + n) * 2 + 1] = x;
                }
            }
        }
        v
    }

    /// 신호를 `chunk`샘플씩 잘라 넣는다. 버퍼의 QPC는 base + 첫 샘플 번호의 시각.
    fn run_chunks(sig: &[f32], base: i64, chunk: usize, skip: impl Fn(usize) -> bool) -> Vec<i64> {
        let mut det = BeepDetector::new(DEFAULT_THRESHOLD);
        let mut out = Vec::new();
        let frames = sig.len() / 2;
        let mut i = 0;
        while i < frames {
            let n = chunk.min(frames - i);
            if !skip(i) {
                det.push(
                    Some(base + samples_to_qpc(i as u64)),
                    &sig[i * 2..(i + n) * 2],
                    &mut out,
                );
            }
            i += n;
        }
        out
    }

    // 삐 안 첫 샘플(값 0)은 문턱 아래라 둘째 샘플(0.5·sin(2π/48) ≈ 0.065)이 시작으로 잡힌다.
    fn expect(base: i64, start: usize) -> i64 {
        base + samples_to_qpc(start as u64 + 1)
    }

    #[test]
    fn detects_exact_start_sample() {
        let base = 12_345 * SEC;
        let sig = signal(48_000 * 3, &[48_000, 96_000]);
        let got = run_chunks(&sig, base, 480, |_| false);
        assert_eq!(got, vec![expect(base, 48_000), expect(base, 96_000)]);
    }

    #[test]
    fn start_across_buffer_boundary() {
        let base = 777 * SEC;
        // 삐가 버퍼 경계(441 샘플 단위) 안쪽 한가운데에서 시작
        let start = 441 * 120 + 200;
        let sig = signal(48_000 * 2, &[start]);
        let got = run_chunks(&sig, base, 441, |_| false);
        assert_eq!(got.len(), 1);
        assert!((got[0] - expect(base, start)).abs() <= 1, "{got:?}");
        // 삐가 정확히 버퍼의 첫 샘플에서 시작
        let start = 441 * 90;
        let sig = signal(48_000 * 2, &[start]);
        let got = run_chunks(&sig, base, 441, |_| false);
        assert_eq!(got.len(), 1);
        assert!((got[0] - expect(base, start)).abs() <= 1, "{got:?}");
    }

    #[test]
    fn noise_below_threshold_is_ignored() {
        let base = SEC;
        let mut sig = signal(48_000 * 2, &[48_000]);
        // 삐 앞쪽에 ±0.01짜리 잡음(문턱 0.02 아래)
        let mut seed = 12345u32;
        for v in sig.iter_mut().take(48_000 * 2 - 2) {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            if *v == 0.0 {
                *v = ((seed >> 16) as f32 / 65_535.0 - 0.5) * 0.02;
            }
        }
        let got = run_chunks(&sig, base, 480, |_| false);
        assert_eq!(got.len(), 1, "{got:?}");
        assert!((got[0] - expect(base, 48_000)).abs() <= 1);
    }

    #[test]
    fn click_close_to_a_beep_is_not_a_new_beep() {
        let base = SEC;
        let mut sig = signal(48_000 * 3, &[48_000]);
        // 삐가 끝나고 20ms 뒤의 짧은 잡음(조용한 구간 50ms 미만)
        let click = 48_000 + 4800 + 960;
        sig[click * 2] = 0.3;
        sig[click * 2 + 1] = 0.3;
        // 그 뒤 1초 조용하다가 진짜 삐
        let sig2 = {
            let mut s = sig.clone();
            let b = 48_000 * 2 + 10;
            for n in 0..4800usize {
                s[(b + n) * 2] = tone(n as u64);
                s[(b + n) * 2 + 1] = tone(n as u64);
            }
            s
        };
        let got = run_chunks(&sig2, base, 480, |_| false);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!((got[0] - expect(base, 48_000)).abs() <= 1);
        assert!((got[1] - expect(base, 48_000 * 2 + 10)).abs() <= 1);
    }

    #[test]
    fn gap_without_packets_counts_as_quiet() {
        let base = 5 * SEC;
        // 루프백은 소리가 없으면 패킷을 주지 않는다: 0.5~1.0초 구간의 버퍼를 통째로 건너뛴다.
        let sig = signal(48_000 * 2, &[48_000 + 100]);
        let got = run_chunks(&sig, base, 480, |i| (24_000..48_000).contains(&i));
        assert_eq!(got.len(), 1);
        assert!((got[0] - expect(base, 48_000 + 100)).abs() <= 1);
        // 시작 직후(첫 버퍼가 이미 삐 안)는 조용한 구간이 없으므로 잡지 않는다.
        let sig = signal(48_000, &[0]);
        assert!(run_chunks(&sig, base, 480, |_| false).is_empty());
        // 앞 버퍼를 못 받아 삐 직전에 간격이 생겨도 간격이 50ms 이상이면 잡는다.
        let sig = signal(48_000, &[24_000]);
        let got = run_chunks(&sig, base, 480, |i| (480..24_000).contains(&i));
        assert_eq!(got.len(), 1, "{got:?}");
    }

    #[test]
    fn buffer_without_qpc_continues_previous() {
        let base = 3 * SEC;
        let sig = signal(48_000 * 2, &[60_000]);
        let mut det = BeepDetector::new(DEFAULT_THRESHOLD);
        let mut out = Vec::new();
        let frames = sig.len() / 2;
        let mut i = 0;
        while i < frames {
            let n = 480.min(frames - i);
            // 첫 버퍼만 QPC가 있고 나머지는 없다.
            let q = (i == 0).then_some(base);
            det.push(q, &sig[i * 2..(i + n) * 2], &mut out);
            i += n;
        }
        assert_eq!(out.len(), 1);
        assert!((out[0] - expect(base, 60_000)).abs() <= 2);
    }
}
