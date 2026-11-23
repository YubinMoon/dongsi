//! 시험용 simulcast 계층(측정용, 2026-11-23): 주 인코더와 같은 코덱·해상도·fps로 비트레이트만 낮춘 인코더를
//! 계층마다 스레드 하나씩 더 돌린다. 인코딩한 프레임은 보내지 않고 버리며 통계만 낸다.
//!
//! - 켜기: 환경 변수 `DONGSI_SIMULCAST_TEST`. `1`이면 설정 비트레이트의 1/2·1/4 두 계층이고, `2,4,8`처럼 쓰면 그
//!   수로 나눈 계층들이다. 없거나 `0`이면 꺼진다.
//! - 워커는 변환한 프레임(주 인코더 입력)을 계층마다 복사해 넘긴다. 계층 스레드가 아직 앞 프레임을 인코딩 중이면
//!   기다리던 프레임은 버린다(`dropped_frames`). 그래서 계층이 주 인코더를 막지 않고, 계층의 fps는 하드웨어가 동시에
//!   낼 수 있는 양을 보여 준다.
//! - 계층 비트레이트는 설정 비트레이트(슬라이더 값)를 나눈 값이다. 네트워크 상한(`set_bitrate_cap`)은 따르지 않는다.
//! - 키프레임은 주 인코더와 같은 프레임에서 요청하고, 계층 인코더를 새로 열면 그 첫 프레임도 키프레임이다.
//! - 2초마다 주 인코더와 계층의 통계를 info 로그로 남긴다(`VideoPipeline::layer_stats`로도 본다).

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dongsi_core::VideoCodec;
use dongsi_core::priority::{ThreadRole, boost_current_thread};
use serde::Serialize;

use crate::encoder::{EncodedPacket, EncoderParams, VideoEncoder, open_first};
use crate::video::VideoStats;
use crate::yuv::{Chroma, YuvView};

/// 시험 계층을 켜는 환경 변수.
pub const SIMULCAST_TEST_ENV: &str = "DONGSI_SIMULCAST_TEST";

/// 계층 비트레이트 하한(kbps).
const MIN_LAYER_KBPS: u32 = 100;
/// 계층 인코더를 열지 못했을 때 같은 설정으로 다시 열어 보기까지 기다리는 시간.
const REOPEN_BACKOFF: Duration = Duration::from_secs(2);
const LOG_INTERVAL: Duration = Duration::from_secs(2);

/// 시험 계층 하나의 통계. 속도 값은 최근 1초 기준이다.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerStats {
    /// 설정 비트레이트를 나눈 수(2 = 1/2)
    pub divisor: u32,
    pub encoder: String,
    pub width: u32,
    pub height: u32,
    /// 인코더에 건 비트레이트(kbps)
    pub target_kbps: u32,
    pub encode_fps: f32,
    /// 인코더 호출 시간(평균, ms). 입력 복사는 들지 않는다.
    pub avg_encode_ms: f32,
    pub bitrate_kbps: f32,
    /// 계층 스레드가 바빠 버린 입력 프레임 수(누적)
    pub dropped_frames: u64,
    /// 인코딩한 프레임 수(누적)
    pub frames: u64,
    pub keyframes: u64,
    pub last_error: Option<String>,
}

/// 환경 변수 값을 나눗수 목록으로 푼다.
pub(crate) fn parse_divisors(v: &str) -> Vec<u32> {
    match v.trim() {
        "" | "0" => Vec::new(),
        "1" | "on" | "true" => vec![2, 4],
        list => list
            .split(',')
            .filter_map(|s| s.trim().parse::<u32>().ok())
            .filter(|d| *d >= 2)
            .collect(),
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn ema(prev: f32, v: f32) -> f32 {
    if prev == 0.0 { v } else { prev * 0.9 + v * 0.1 }
}

/// 계층에 넘기는 입력 프레임 한 장(평면을 빈틈없이 복사한 것).
#[derive(Default)]
struct Input {
    y: Vec<u8>,
    /// NV12면 UV, I420이면 U
    c1: Vec<u8>,
    /// I420의 V(NV12면 비어 있다)
    c2: Vec<u8>,
    nv12: bool,
    width: usize,
    height: usize,
    qpc: i64,
    arrival_qpc: i64,
    keyframe: bool,
    codec: Option<VideoCodec>,
    fps: u32,
    kbps: u32,
}

fn copy_plane(dst: &mut Vec<u8>, src: &[u8], stride: usize, row: usize, rows: usize) {
    dst.clear();
    dst.reserve(row * rows);
    for r in 0..rows {
        dst.extend_from_slice(&src[r * stride..r * stride + row]);
    }
}

fn paste_plane(dst: &mut [u8], stride: usize, src: &[u8], row: usize, rows: usize) {
    for r in 0..rows {
        dst[r * stride..r * stride + row].copy_from_slice(&src[r * row..(r + 1) * row]);
    }
}

impl Input {
    fn fill_from(&mut self, view: &YuvView<'_>) {
        let (w, h) = (view.width, view.height);
        self.width = w;
        self.height = h;
        copy_plane(&mut self.y, view.y, view.y_stride, w, h);
        match &view.chroma {
            Chroma::Nv12 { uv, stride } => {
                self.nv12 = true;
                copy_plane(&mut self.c1, uv, *stride, w, h / 2);
                self.c2.clear();
            }
            Chroma::I420 {
                u,
                u_stride,
                v,
                v_stride,
            } => {
                self.nv12 = false;
                copy_plane(&mut self.c1, u, *u_stride, w / 2, h / 2);
                copy_plane(&mut self.c2, v, *v_stride, w / 2, h / 2);
            }
        }
    }

    /// 인코더 입력 버퍼에 붙인다. 크기나 크로마 배치가 다르면 `false`.
    fn paste_into(&self, view: &mut YuvView<'_>) -> bool {
        let (w, h) = (self.width, self.height);
        if view.width != w || view.height != h {
            return false;
        }
        paste_plane(view.y, view.y_stride, &self.y, w, h);
        match &mut view.chroma {
            Chroma::Nv12 { uv, stride } if self.nv12 => {
                paste_plane(uv, *stride, &self.c1, w, h / 2);
                true
            }
            Chroma::I420 {
                u,
                u_stride,
                v,
                v_stride,
            } if !self.nv12 => {
                paste_plane(u, *u_stride, &self.c1, w / 2, h / 2);
                paste_plane(v, *v_stride, &self.c2, w / 2, h / 2);
                true
            }
            _ => false,
        }
    }
}

#[derive(Default)]
struct Slot {
    input: Option<Input>,
    /// 다 쓴 버퍼(다음 입력에 다시 쓴다)
    spare: Option<Input>,
    stop: bool,
}

#[derive(Default)]
struct StatsState {
    stats: LayerStats,
    encoded: VecDeque<(Instant, usize)>,
}

struct LayerState {
    slot: Mutex<Slot>,
    cond: Condvar,
    stats: Mutex<StatsState>,
    started: Instant,
}

struct Layer {
    state: Arc<LayerState>,
    thread: Option<JoinHandle<()>>,
}

/// 시험 계층들. 파이프라인마다 하나.
pub(crate) struct Layers {
    layers: Vec<Layer>,
    /// (마지막 로그 시각, 워커의 복사 시간 평균 ms)
    worker: Mutex<(Instant, f32)>,
}

impl Layers {
    /// 환경 변수가 켜져 있으면 계층 스레드를 시작한다.
    pub fn from_env() -> Option<Layers> {
        let divisors = parse_divisors(&std::env::var(SIMULCAST_TEST_ENV).ok()?);
        Layers::start(&divisors)
    }

    pub fn start(divisors: &[u32]) -> Option<Layers> {
        if divisors.is_empty() {
            return None;
        }
        let now = Instant::now();
        let layers = divisors
            .iter()
            .filter_map(|&divisor| {
                let state = Arc::new(LayerState {
                    slot: Mutex::new(Slot::default()),
                    cond: Condvar::new(),
                    stats: Mutex::new(StatsState {
                        stats: LayerStats {
                            divisor,
                            ..LayerStats::default()
                        },
                        encoded: VecDeque::new(),
                    }),
                    started: now,
                });
                let s = state.clone();
                let thread = std::thread::Builder::new()
                    .name(format!("dongsi-simulcast-1/{divisor}"))
                    .spawn(move || run(s, divisor));
                match thread {
                    Ok(t) => Some(Layer {
                        state,
                        thread: Some(t),
                    }),
                    Err(e) => {
                        tracing::warn!(
                            divisor,
                            "simulcast 시험 계층 스레드를 시작하지 못했습니다: {e}"
                        );
                        None
                    }
                }
            })
            .collect::<Vec<_>>();
        tracing::info!(
            ?divisors,
            "simulcast 시험 계층 켬(같은 해상도, 비트레이트만 나눔)"
        );
        Some(Layers {
            layers,
            worker: Mutex::new((now, 0.0)),
        })
    }

    /// 주 인코더에 넣을 프레임을 계층마다 복사해 넘긴다. 워커 스레드에서 부른다.
    #[allow(clippy::too_many_arguments)]
    pub fn submit(
        &self,
        view: &YuvView<'_>,
        qpc: i64,
        arrival_qpc: i64,
        keyframe: bool,
        codec: VideoCodec,
        fps: u32,
        kbps: u32,
    ) {
        let t0 = Instant::now();
        for layer in &self.layers {
            let st = &layer.state;
            let mut buf = {
                let mut s = lock(&st.slot);
                match s.input.take() {
                    Some(old) => {
                        lock(&st.stats).stats.dropped_frames += 1;
                        old
                    }
                    None => s.spare.take().unwrap_or_default(),
                }
            };
            buf.fill_from(view);
            buf.qpc = qpc;
            buf.arrival_qpc = arrival_qpc;
            // 버린 프레임의 키프레임 요청은 다음 프레임이 이어받는다.
            buf.keyframe = keyframe;
            buf.codec = Some(codec);
            buf.fps = fps;
            buf.kbps = kbps;
            let mut s = lock(&st.slot);
            s.input = Some(buf);
            st.cond.notify_one();
        }
        let ms = t0.elapsed().as_secs_f32() * 1000.0;
        let mut w = lock(&self.worker);
        w.1 = ema(w.1, ms);
    }

    pub fn stats(&self) -> Vec<LayerStats> {
        let now = Instant::now();
        self.layers
            .iter()
            .map(|l| {
                let mut st = lock(&l.state.stats);
                let window = Duration::from_secs(1);
                while st.encoded.front().is_some_and(|(t, _)| now - *t > window) {
                    st.encoded.pop_front();
                }
                let span = (now - l.state.started).as_secs_f32().clamp(0.001, 1.0);
                let mut s = st.stats.clone();
                s.encode_fps = st.encoded.len() as f32 / span;
                s.bitrate_kbps =
                    st.encoded.iter().map(|(_, b)| *b).sum::<usize>() as f32 * 8.0 / 1000.0 / span;
                s
            })
            .collect()
    }

    /// 로그를 남길 때가 됐으면 주 인코더 통계와 같이 남긴다.
    pub fn log_if_due(&self, main: impl FnOnce() -> VideoStats) {
        let copy_ms = {
            let mut w = lock(&self.worker);
            if w.0.elapsed() < LOG_INTERVAL {
                return;
            }
            w.0 = Instant::now();
            w.1
        };
        let m = main();
        tracing::info!(
            "simulcast 시험 주 인코더: {} {}x{} 캡처 {:.1} 인코딩 {:.1} fps, 인코딩 {:.2} ms, 계층 복사 {:.2} ms, {:.0} kbps, 버림 {}",
            m.encoder,
            m.width,
            m.height,
            m.capture_fps,
            m.encode_fps,
            m.avg_encode_ms,
            copy_ms,
            m.bitrate_kbps,
            m.dropped_frames
        );
        for l in self.stats() {
            tracing::info!(
                "simulcast 시험 계층 1/{}: {} {}x{} 목표 {} kbps, 인코딩 {:.1} fps, 인코딩 {:.2} ms, {:.0} kbps, 버림 {}, 프레임 {}, 키프레임 {}{}",
                l.divisor,
                l.encoder,
                l.width,
                l.height,
                l.target_kbps,
                l.encode_fps,
                l.avg_encode_ms,
                l.bitrate_kbps,
                l.dropped_frames,
                l.frames,
                l.keyframes,
                l.last_error
                    .map(|e| format!(", 오류: {e}"))
                    .unwrap_or_default()
            );
        }
    }
}

impl Drop for Layers {
    fn drop(&mut self) {
        for l in &self.layers {
            lock(&l.state.slot).stop = true;
            l.state.cond.notify_all();
        }
        for l in &mut self.layers {
            if let Some(t) = l.thread.take() {
                let _ = t.join();
            }
        }
    }
}

/// 계층 스레드: 입력을 기다렸다가 자기 인코더로 인코딩한다.
fn run(state: Arc<LayerState>, divisor: u32) {
    let _boost = boost_current_thread(ThreadRole::Video);
    let mut enc: Option<VideoEncoder> = None;
    let mut failed: Option<(VideoCodec, EncoderParams, Instant)> = None;
    let mut packets: Vec<EncodedPacket> = Vec::new();
    loop {
        let input = {
            let mut s = lock(&state.slot);
            loop {
                if s.stop {
                    return;
                }
                if let Some(i) = s.input.take() {
                    break i;
                }
                s = state.cond.wait(s).unwrap_or_else(|e| e.into_inner());
            }
        };
        encode_one(&state, divisor, &input, &mut enc, &mut failed, &mut packets);
        lock(&state.slot).spare = Some(input);
    }
}

fn set_error(state: &LayerState, e: String) {
    lock(&state.stats).stats.last_error = Some(e);
}

fn encode_one(
    state: &LayerState,
    divisor: u32,
    input: &Input,
    enc: &mut Option<VideoEncoder>,
    failed: &mut Option<(VideoCodec, EncoderParams, Instant)>,
    packets: &mut Vec<EncodedPacket>,
) {
    let Some(codec) = input.codec else { return };
    let params = EncoderParams {
        width: input.width as u32,
        height: input.height as u32,
        fps: input.fps,
        bitrate_kbps: (input.kbps / divisor).max(MIN_LAYER_KBPS),
    };
    let mut key = input.keyframe;
    let current = enc
        .as_ref()
        .filter(|e| e.info.codec == codec)
        .map(|e| e.params);
    if current != Some(params) {
        let only_bitrate = current.is_some_and(|p| {
            EncoderParams {
                bitrate_kbps: p.bitrate_kbps,
                ..params
            } == p
        });
        let kept = only_bitrate
            && enc
                .as_mut()
                .is_some_and(|e| e.set_bitrate(params.bitrate_kbps));
        if !kept {
            *enc = None;
            if failed
                .is_some_and(|(c, p, t)| c == codec && p == params && t.elapsed() < REOPEN_BACKOFF)
            {
                return;
            }
            match open_first(codec, &params, false) {
                Ok(e) => {
                    tracing::info!(
                        divisor,
                        encoder = %e.info.name,
                        width = params.width,
                        height = params.height,
                        fps = params.fps,
                        kbps = params.bitrate_kbps,
                        "simulcast 시험 계층 인코더 열림"
                    );
                    *enc = Some(e);
                    *failed = None;
                    key = true;
                }
                Err(e) => {
                    tracing::warn!(divisor, "simulcast 시험 계층 인코더를 열지 못했습니다: {e}");
                    set_error(state, e.to_string());
                    *failed = Some((codec, params, Instant::now()));
                    return;
                }
            }
        }
        let mut st = lock(&state.stats);
        if let Some(e) = enc.as_ref() {
            st.stats.encoder = e.info.name.clone();
        }
        st.stats.width = params.width;
        st.stats.height = params.height;
        st.stats.target_kbps = params.bitrate_kbps;
    }
    let Some(e) = enc.as_mut() else { return };
    match e.frame_view() {
        Ok(mut view) => {
            if !input.paste_into(&mut view) {
                set_error(state, "입력 프레임 배치가 인코더와 다릅니다".into());
                return;
            }
        }
        Err(err) => {
            set_error(state, err.to_string());
            *enc = None;
            return;
        }
    }
    let t = Instant::now();
    let r = e.encode(input.qpc, input.arrival_qpc, key, &mut |p| packets.push(p));
    let ms = t.elapsed().as_secs_f32() * 1000.0;
    let now = Instant::now();
    let mut st = lock(&state.stats);
    st.stats.avg_encode_ms = ema(st.stats.avg_encode_ms, ms);
    for p in packets.drain(..) {
        st.encoded.push_back((now, p.data.len()));
        st.stats.frames += 1;
        if p.keyframe {
            st.stats.keyframes += 1;
        }
    }
    let window = Duration::from_secs(1);
    while st.encoded.front().is_some_and(|(t, _)| now - *t > window) {
        st.encoded.pop_front();
    }
    if let Err(err) = r {
        st.stats.last_error = Some(err.to_string());
        drop(st);
        *enc = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divisors() {
        assert_eq!(parse_divisors(""), Vec::<u32>::new());
        assert_eq!(parse_divisors("0"), Vec::<u32>::new());
        assert_eq!(parse_divisors("1"), vec![2, 4]);
        assert_eq!(parse_divisors("2, 4,8"), vec![2, 4, 8]);
        assert_eq!(parse_divisors("x,1,3"), vec![3]);
    }

    #[test]
    fn input_roundtrip_nv12() {
        let (w, h) = (6usize, 4usize);
        let (ys, cs) = (8usize, 8usize);
        let mut y: Vec<u8> = (0..ys * h).map(|i| i as u8).collect();
        let mut uv: Vec<u8> = (0..cs * h / 2).map(|i| 100 + i as u8).collect();
        let src = YuvView {
            width: w,
            height: h,
            y: &mut y,
            y_stride: ys,
            chroma: Chroma::Nv12 {
                uv: &mut uv,
                stride: cs,
            },
        };
        let mut input = Input::default();
        input.fill_from(&src);
        assert_eq!(input.y.len(), w * h);
        assert_eq!(input.c1.len(), w * h / 2);
        let (mut y2, mut uv2) = (vec![0u8; 10 * h], vec![0u8; 10 * h / 2]);
        let mut dst = YuvView {
            width: w,
            height: h,
            y: &mut y2,
            y_stride: 10,
            chroma: Chroma::Nv12 {
                uv: &mut uv2,
                stride: 10,
            },
        };
        assert!(input.paste_into(&mut dst));
        for r in 0..h {
            assert_eq!(&y2[r * 10..r * 10 + w], &y[r * ys..r * ys + w]);
        }
        for r in 0..h / 2 {
            assert_eq!(&uv2[r * 10..r * 10 + w], &uv[r * cs..r * cs + w]);
        }
    }
}
