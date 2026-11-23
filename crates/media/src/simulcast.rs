//! simulcast 하위 계층(D-098): 주 인코더(상)와 같은 코덱·해상도·fps로 비트레이트만 낮춘 인코더를 계층(중·하)마다
//! 스레드 하나씩 더 돌린다.
//!
//! - 켜기: [`VideoPipeline::start_simulcast`](crate::VideoPipeline::start_simulcast). 계층 프레임은 그때 준
//!   [`LayerSink`]로 나간다(`EncodedVideo::layer`). 주 인코더 프레임(상)은 늘 쓰던 `on_frame`으로 나간다.
//! - 워커는 변환한 프레임(주 인코더 입력)을 계층마다 복사해 넘긴다. 계층 스레드가 아직 앞 프레임을 인코딩 중이면
//!   기다리던 프레임은 버린다(`dropped_frames`). 그래서 계층이 주 인코더를 막지 않는다. 1440p60까지 세 계층이
//!   60fps를 낸다(2026-11-23 측정, `.docs/research.md`).
//! - 계층 비트레이트는 앱이 송출의 계층 목표로 정한다([`VideoPipeline::set_layer_rates`](crate::VideoPipeline::set_layer_rates)).
//!   정하기 전에는 설정 비트레이트의 1/2·1/4다. 0이면 그 계층은 인코딩하지 않고(인코더는 열어 둔다), 다시 켜면 첫
//!   프레임이 키프레임이다.
//! - 계층 인코더는 주 인코더와 같은 인코더(이름)로만 연다. 주 인코더가 소프트웨어(libopenh264·libsvtav1)면 계층은
//!   쉰다: CPU로 셋을 인코딩하지 않는다(사용자 결정, D-098).
//! - 키프레임은 계층마다 따로 요청한다([`Layers::request_keyframe`]). 계층 인코더를 새로 열거나 쉬다가 다시 켜면 그
//!   첫 프레임도 키프레임이다.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dongsi_core::priority::{ThreadRole, boost_current_thread};
use dongsi_core::{Layer, LayerRates, VideoCodec};
use serde::Serialize;

use crate::clock::instant_from_qpc;
use crate::encoder::{EncodedPacket, EncoderParams, VideoEncoder, open_named};
use crate::video::EncodedVideo;
use crate::yuv::{Chroma, YuvView};

/// 계층 프레임을 받는 곳. 계층 스레드에서 불린다. 오래 막으면 안 된다(채널로 넘기는 정도).
pub type LayerSink = Arc<dyn Fn(EncodedVideo) + Send + Sync>;

/// 주 인코더가 아닌 계층(주 인코더가 상).
const SUB_LAYERS: [Layer; 2] = [Layer::Mid, Layer::Low];
/// 계층 인코더를 열지 못했을 때 같은 설정으로 다시 열어 보기까지 기다리는 시간.
const REOPEN_BACKOFF: Duration = Duration::from_secs(2);

/// 계층 하나의 통계. 속도 값은 최근 1초 기준이다.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerStats {
    pub layer: Layer,
    pub encoder: String,
    pub width: u32,
    pub height: u32,
    /// 앱이 정한 비트레이트(kbps). 0이면 쉬는 중이다.
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
    /// 주 인코더가 소프트웨어라 쉬는 중이다.
    pub software_paused: bool,
    pub last_error: Option<String>,
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
    codec: Option<VideoCodec>,
    /// 주 인코더의 ffmpeg 인코더 이름(계층도 같은 것으로 연다)
    encoder: String,
    fps: u32,
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

struct Slot {
    input: Option<Input>,
    /// 다 쓴 버퍼(다음 입력에 다시 쓴다)
    spare: Option<Input>,
    stop: bool,
    /// 앱이 정한 비트레이트(kbps). 0이면 쉰다.
    kbps: u32,
    /// 다음 프레임을 키프레임으로(요청을 받았고 아직 인코딩하지 않음).
    keyframe: bool,
}

struct StatsState {
    stats: LayerStats,
    encoded: VecDeque<(Instant, usize)>,
}

struct LayerState {
    layer: Layer,
    slot: Mutex<Slot>,
    cond: Condvar,
    stats: Mutex<StatsState>,
    started: Instant,
}

struct Worker {
    state: Arc<LayerState>,
    thread: Option<JoinHandle<()>>,
}

/// 하위 계층들(중·하). 파이프라인마다 하나.
pub(crate) struct Layers {
    layers: Vec<Worker>,
}

impl Layers {
    /// 계층 스레드를 시작한다. 계층 비트레이트는 `high_kbps`(상의 비트레이트)를 나눈 값으로 시작한다.
    pub fn start(sink: LayerSink, high_kbps: u32) -> Layers {
        let now = Instant::now();
        let layers = SUB_LAYERS
            .iter()
            .filter_map(|&layer| {
                let state = Arc::new(LayerState {
                    layer,
                    slot: Mutex::new(Slot {
                        input: None,
                        spare: None,
                        stop: false,
                        kbps: layer.kbps(high_kbps),
                        keyframe: true,
                    }),
                    cond: Condvar::new(),
                    stats: Mutex::new(StatsState {
                        stats: LayerStats {
                            layer,
                            encoder: String::new(),
                            width: 0,
                            height: 0,
                            target_kbps: layer.kbps(high_kbps),
                            encode_fps: 0.0,
                            avg_encode_ms: 0.0,
                            bitrate_kbps: 0.0,
                            dropped_frames: 0,
                            frames: 0,
                            keyframes: 0,
                            software_paused: false,
                            last_error: None,
                        },
                        encoded: VecDeque::new(),
                    }),
                    started: now,
                });
                let s = state.clone();
                let sink = sink.clone();
                let thread = std::thread::Builder::new()
                    .name(format!("dongsi-simulcast-{}", layer.rid()))
                    .spawn(move || run(s, sink));
                match thread {
                    Ok(t) => Some(Worker {
                        state,
                        thread: Some(t),
                    }),
                    Err(e) => {
                        tracing::warn!(%layer, "simulcast 계층 스레드를 시작하지 못했습니다: {e}");
                        None
                    }
                }
            })
            .collect::<Vec<_>>();
        tracing::info!(high_kbps, "simulcast 계층 켬(중·하, 같은 해상도·fps)");
        Layers { layers }
    }

    fn worker(&self, layer: Layer) -> Option<&Worker> {
        self.layers.iter().find(|w| w.state.layer == layer)
    }

    /// 계층 비트레이트(kbps, 상·중·하 순서, 상은 쓰지 않는다). 0인 계층은 쉰다. 쉬다가 다시 켜면 키프레임부터 낸다.
    pub fn set_rates(&self, rates: LayerRates) {
        for w in &self.layers {
            let kbps = rates[w.state.layer.index()];
            let mut s = lock(&w.state.slot);
            if s.kbps == kbps {
                continue;
            }
            if s.kbps == 0 {
                s.keyframe = true;
            }
            tracing::info!(layer = %w.state.layer, from = s.kbps, to = kbps, "simulcast 계층 비트레이트 변경");
            s.kbps = kbps;
            lock(&w.state.stats).stats.target_kbps = kbps;
        }
    }

    /// 계층의 다음 프레임을 키프레임으로 만든다. 상은 여기서 다루지 않는다(주 인코더).
    pub fn request_keyframe(&self, layer: Layer) {
        if let Some(w) = self.worker(layer) {
            lock(&w.state.slot).keyframe = true;
        }
    }

    /// 아직 인코딩하지 않은 키프레임 요청이 있는 계층이 있는지(쉬는 계층은 빼고). 화면이 멈춰 새 프레임이 없으면
    /// 워커가 마지막 화면을 다시 넘긴다.
    pub fn wants_keyframe(&self) -> bool {
        self.layers.iter().any(|w| {
            let s = lock(&w.state.slot);
            s.keyframe && s.kbps > 0
        })
    }

    /// 주 인코더에 넣을 프레임을 계층마다 복사해 넘긴다. 워커 스레드에서 부른다. 주 인코더가 소프트웨어면
    /// (`hardware` false) 넘기지 않는다.
    pub fn submit(
        &self,
        view: &YuvView<'_>,
        qpc: i64,
        arrival_qpc: i64,
        codec: VideoCodec,
        encoder: (&str, bool),
        fps: u32,
    ) {
        let (name, hardware) = encoder;
        for w in &self.layers {
            let st = &w.state;
            {
                let mut stats = lock(&st.stats);
                if stats.stats.software_paused == hardware {
                    stats.stats.software_paused = !hardware;
                    if !hardware {
                        tracing::info!(layer = %st.layer, encoder = name, "주 인코더가 소프트웨어라 simulcast 계층을 쉽니다");
                    }
                }
            }
            if !hardware {
                continue;
            }
            let mut buf = {
                let mut s = lock(&st.slot);
                if s.kbps == 0 {
                    continue;
                }
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
            buf.codec = Some(codec);
            if buf.encoder != name {
                buf.encoder = name.to_owned();
            }
            buf.fps = fps;
            let mut s = lock(&st.slot);
            s.input = Some(buf);
            st.cond.notify_one();
        }
    }

    pub fn stats(&self) -> Vec<LayerStats> {
        let now = Instant::now();
        self.layers
            .iter()
            .map(|l| {
                let mut st = lock(&l.state.stats);
                prune(&mut st.encoded, now);
                let span = (now - l.state.started).as_secs_f32().clamp(0.001, 1.0);
                let mut s = st.stats.clone();
                s.encode_fps = st.encoded.len() as f32 / span;
                s.bitrate_kbps =
                    st.encoded.iter().map(|(_, b)| *b).sum::<usize>() as f32 * 8.0 / 1000.0 / span;
                s
            })
            .collect()
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

fn prune(encoded: &mut VecDeque<(Instant, usize)>, now: Instant) {
    let window = Duration::from_secs(1);
    while encoded.front().is_some_and(|(t, _)| now - *t > window) {
        encoded.pop_front();
    }
}

/// 계층 인코더 상태.
struct Encoding {
    enc: Option<VideoEncoder>,
    /// 마지막으로 열지 못한 (인코더 이름, 설정, 시각)
    failed: Option<(String, EncoderParams, Instant)>,
    packets: Vec<EncodedPacket>,
}

/// 계층 스레드: 입력을 기다렸다가 자기 인코더로 인코딩한다.
fn run(state: Arc<LayerState>, sink: LayerSink) {
    let _boost = boost_current_thread(ThreadRole::Video);
    let mut e = Encoding {
        enc: None,
        failed: None,
        packets: Vec::new(),
    };
    loop {
        let (input, kbps, key) = {
            let mut s = lock(&state.slot);
            loop {
                if s.stop {
                    return;
                }
                if let Some(i) = s.input.take() {
                    let key = std::mem::take(&mut s.keyframe);
                    break (i, s.kbps, key);
                }
                s = state.cond.wait(s).unwrap_or_else(|e| e.into_inner());
            }
        };
        let done = kbps == 0 || encode_one(&state, &sink, &input, kbps, key, &mut e);
        let mut s = lock(&state.slot);
        if key && !done {
            // 인코딩하지 못했으면 요청을 다음 프레임에 넘긴다.
            s.keyframe = true;
        }
        s.spare = Some(input);
    }
}

fn set_error(state: &LayerState, e: String) {
    lock(&state.stats).stats.last_error = Some(e);
}

/// 한 프레임을 인코딩한다. 인코딩했으면(출력이 없어도) `true`.
fn encode_one(
    state: &LayerState,
    sink: &LayerSink,
    input: &Input,
    kbps: u32,
    mut key: bool,
    e: &mut Encoding,
) -> bool {
    let layer = state.layer;
    let Some(codec) = input.codec else {
        return false;
    };
    let params = EncoderParams {
        width: input.width as u32,
        height: input.height as u32,
        fps: input.fps,
        bitrate_kbps: kbps,
    };
    let current = e
        .enc
        .as_ref()
        .filter(|x| x.info.codec == codec && x.info.name == input.encoder)
        .map(|x| x.params);
    if current != Some(params) {
        let only_bitrate = current.is_some_and(|p| {
            EncoderParams {
                bitrate_kbps: p.bitrate_kbps,
                ..params
            } == p
        });
        let kept = only_bitrate
            && e.enc
                .as_mut()
                .is_some_and(|x| x.set_bitrate(params.bitrate_kbps));
        if !kept {
            e.enc = None;
            if e.failed.as_ref().is_some_and(|(n, p, t)| {
                *n == input.encoder && *p == params && t.elapsed() < REOPEN_BACKOFF
            }) {
                return false;
            }
            match open_named(codec, &input.encoder, &params) {
                Ok(x) => {
                    tracing::info!(
                        %layer,
                        encoder = %x.info.name,
                        width = params.width,
                        height = params.height,
                        fps = params.fps,
                        kbps = params.bitrate_kbps,
                        "simulcast 계층 인코더 열림"
                    );
                    e.enc = Some(x);
                    e.failed = None;
                    key = true;
                    lock(&state.stats).stats.last_error = None;
                }
                Err(err) => {
                    tracing::warn!(%layer, "simulcast 계층 인코더를 열지 못했습니다: {err}");
                    set_error(state, err.to_string());
                    e.failed = Some((input.encoder.clone(), params, Instant::now()));
                    return false;
                }
            }
        }
        let mut st = lock(&state.stats);
        if let Some(x) = e.enc.as_ref() {
            st.stats.encoder = x.info.name.clone();
        }
        st.stats.width = params.width;
        st.stats.height = params.height;
    }
    let Some(enc) = e.enc.as_mut() else {
        return false;
    };
    match enc.frame_view() {
        Ok(mut view) => {
            if !input.paste_into(&mut view) {
                set_error(state, "입력 프레임 배치가 인코더와 다릅니다".into());
                return false;
            }
        }
        Err(err) => {
            set_error(state, err.to_string());
            e.enc = None;
            return false;
        }
    }
    let t = Instant::now();
    let packets = &mut e.packets;
    let r = enc.encode(input.qpc, input.arrival_qpc, key, &mut |p| packets.push(p));
    let ms = t.elapsed().as_secs_f32() * 1000.0;
    let (codec, width, height) = (enc.info.codec, enc.params.width, enc.params.height);
    let now = Instant::now();
    {
        let mut st = lock(&state.stats);
        st.stats.avg_encode_ms = ema(st.stats.avg_encode_ms, ms);
        for p in &e.packets {
            st.encoded.push_back((now, p.data.len()));
            st.stats.frames += 1;
            if p.keyframe {
                st.stats.keyframes += 1;
            }
        }
        prune(&mut st.encoded, now);
        if let Err(err) = &r {
            st.stats.last_error = Some(err.to_string());
        }
    }
    for p in e.packets.drain(..) {
        sink(EncodedVideo {
            layer,
            codec,
            data: p.data,
            keyframe: p.keyframe,
            capture_qpc_100ns: p.qpc,
            capture: instant_from_qpc(p.qpc),
            width,
            height,
        });
    }
    if r.is_err() {
        e.enc = None;
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// 인코더 없이 계층 상태만 본다: 쉬는 계층은 입력을 받지 않고, 다시 켜면 키프레임을 요청한 상태가 된다.
    #[test]
    fn rates_pause_and_resume_with_keyframe() {
        let sink: LayerSink = Arc::new(|_| {});
        let layers = Layers::start(sink, 8000);
        let rates = |l: &Layers| {
            l.stats()
                .iter()
                .map(|s| (s.layer, s.target_kbps))
                .collect::<Vec<_>>()
        };
        assert_eq!(rates(&layers), vec![(Layer::Mid, 4000), (Layer::Low, 2000)]);
        // 처음에는 첫 프레임이 키프레임이어야 한다.
        assert!(layers.wants_keyframe());
        layers.set_rates([8000, 4000, 0]);
        assert_eq!(rates(&layers), vec![(Layer::Mid, 4000), (Layer::Low, 0)]);
        for w in &layers.layers {
            lock(&w.state.slot).keyframe = false;
        }
        assert!(!layers.wants_keyframe());
        // 쉬는 계층의 요청은 세지 않는다.
        layers.request_keyframe(Layer::Low);
        assert!(!layers.wants_keyframe());
        // 다시 켜면 키프레임부터.
        layers.set_rates([8000, 4000, 2000]);
        assert!(layers.wants_keyframe());
        // 상은 주 인코더 몫이라 무시한다.
        layers.request_keyframe(Layer::High);
    }
}
