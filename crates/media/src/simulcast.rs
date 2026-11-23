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
use crate::ff::{FrameRef, HwFramesRef};
use crate::video::EncodedVideo;

/// 계층 프레임을 받는 곳. 계층 스레드에서 불린다. 오래 막으면 안 된다(채널로 넘기는 정도).
pub type LayerSink = Arc<dyn Fn(EncodedVideo) + Send + Sync>;

/// 주 인코더가 아닌 계층(주 인코더가 상).
const SUB_LAYERS: [Layer; 2] = [Layer::Mid, Layer::Low];
/// 계층 인코더를 열지 못했을 때 같은 설정으로 다시 열어 보기까지 기다리는 시간.
const REOPEN_BACKOFF: Duration = Duration::from_secs(2);
/// 같은 풀의 hw 프레임이 이만큼보다 많이 살아 있으면(어딘가가 놓지 않는다) 그 프레임은 계층에 넘기지 않는다. 정상일 때는
/// 워커 1 + 계층 대기 칸 2 + 계층 인코딩 중 2 = 5장 안팎이다(D3D11 NV12 QHD 한 장이 약 5.5MB).
pub(crate) const MAX_LIVE_HW_FRAMES: usize = 8;
/// 상한 경고를 되풀이하는 최소 간격.
const LIVE_WARN_INTERVAL: Duration = Duration::from_secs(5);

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
    /// 인코더 호출 시간(평균, ms)
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

/// 계층에 넘기는 입력 한 건: 주 인코더 입력과 **같은 버퍼를 가리키는 프레임 참조**(픽셀 복사 없음).
///
/// 계층 스레드가 쥔 프레임은 워커가 다시 쓰지 않는다: 워커는 프레임마다 풀에서 새 버퍼를 받고, 버퍼는 이 참조가
/// 없어질 때(계층 인코딩이 끝나거나 대기 칸에서 덮어쓰일 때) 풀로 돌아간다.
struct Input {
    /// 주 인코더 입력과 같은 크기·형식의 프레임. 계층마다 해상도가 달라지면(D-099) 크기별 프레임을 담도록 늘리고,
    /// 계층은 [`frame_for`](Self::frame_for)로 자기 것을 받는다.
    frame: FrameRef,
    qpc: i64,
    arrival_qpc: i64,
    codec: VideoCodec,
    /// 주 인코더의 ffmpeg 인코더 이름(계층도 같은 것으로 연다)
    encoder: String,
    fps: u32,
}

impl Input {
    /// `layer`가 인코딩할 프레임. 지금은 모든 계층이 주 인코더 입력과 같은 프레임이다.
    fn frame_for(&mut self, _layer: Layer) -> &mut FrameRef {
        &mut self.frame
    }
}

struct Slot {
    /// 인코딩을 기다리는 입력(한 칸). 새 입력이 오면 덮어쓰고 이전 것은 버린다(그 프레임 참조는 놓인다).
    input: Option<Input>,
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
    /// hw 입력(D3D11) 계층 인코더를 열지 못하면 여기에 이유를 남긴다. 계층 스레드는 CPU 프레임이 없어 스스로 내려갈 수
    /// 없으므로, 워커가 [`Layers::take_hw_failure`]로 보고 파이프라인 전체를 CPU 경로로 내린다. 계층끼리 공유한다.
    hw_failed: Arc<Mutex<Option<String>>>,
}

struct Worker {
    state: Arc<LayerState>,
    thread: Option<JoinHandle<()>>,
}

/// 하위 계층들(중·하). 파이프라인마다 하나.
pub(crate) struct Layers {
    layers: Vec<Worker>,
    hw_failed: Arc<Mutex<Option<String>>>,
    last_live_warn: Mutex<Option<Instant>>,
}

impl Layers {
    /// 계층 스레드를 시작한다. 계층 비트레이트는 `high_kbps`(상의 비트레이트)를 나눈 값으로 시작한다.
    pub fn start(sink: LayerSink, high_kbps: u32) -> Layers {
        let now = Instant::now();
        let hw_failed = Arc::new(Mutex::new(None));
        let layers = SUB_LAYERS
            .iter()
            .filter_map(|&layer| {
                let state = Arc::new(LayerState {
                    layer,
                    slot: Mutex::new(Slot {
                        input: None,
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
                    hw_failed: hw_failed.clone(),
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
        Layers {
            layers,
            hw_failed,
            last_live_warn: Mutex::new(None),
        }
    }

    /// hw 입력 계층 인코더가 열리지 않았다면 그 이유(한 번 가져가면 비운다). 워커가 보고 CPU 경로로 내린다.
    pub fn take_hw_failure(&self) -> Option<String> {
        lock(&self.hw_failed).take()
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

    /// 주 인코더에 넣은 프레임을 계층마다 **참조로** 넘긴다(`av_frame_clone`, 픽셀 복사 없음). 워커 스레드에서 부른다.
    /// 주 인코더가 소프트웨어면(`hardware` false) 넘기지 않는다.
    ///
    /// 워커는 기다리지 않는다: 계층이 아직 앞 입력을 인코딩 중이면 대기 칸의 입력을 덮어쓴다(`dropped_frames`).
    /// 이 호출 뒤에 워커는 `frame`의 픽셀을 다시 쓰지 않는다(다음 프레임은 풀에서 새 버퍼를 받는다).
    ///
    /// `hw_live`는 `frame`이 D3D11 hw 프레임일 때 그 풀에서 지금 살아 있는 프레임 수다. [`MAX_LIVE_HW_FRAMES`]를 넘으면
    /// 경고하고(5초에 한 번) 이 프레임은 계층에 넘기지 않는다(`dropped_frames`). CPU 프레임이면 0을 준다.
    #[allow(clippy::too_many_arguments)]
    pub fn submit(
        &self,
        frame: &FrameRef,
        qpc: i64,
        arrival_qpc: i64,
        codec: VideoCodec,
        encoder: (&str, bool),
        fps: u32,
        hw_live: usize,
    ) {
        let (name, hardware) = encoder;
        let over_cap = hw_live > MAX_LIVE_HW_FRAMES;
        if over_cap && hardware {
            let mut last = lock(&self.last_live_warn);
            if last.is_none_or(|t| t.elapsed() >= LIVE_WARN_INTERVAL) {
                *last = Some(Instant::now());
                tracing::warn!(
                    live = hw_live,
                    cap = MAX_LIVE_HW_FRAMES,
                    "살아 있는 hw 프레임이 너무 많아 simulcast 계층에 넘기지 않습니다(어딘가가 프레임을 놓지 않는 것 같습니다)"
                );
            }
        }
        for w in &self.layers {
            let st = &w.state;
            let resumed = {
                let mut stats = lock(&st.stats);
                let flipped = stats.stats.software_paused == hardware;
                if flipped {
                    stats.stats.software_paused = !hardware;
                    if !hardware {
                        tracing::info!(layer = %st.layer, encoder = name, "주 인코더가 소프트웨어라 simulcast 계층을 쉽니다");
                    }
                }
                flipped && hardware
            };
            if resumed {
                // 쉬는 동안 계층 인코더는 그대로 남아 있어(코덱·이름·크기가 같으면 다시 열지 않는다) 이어 인코딩하면 첫 프레임이
                // 쉬기 전 스트림의 P 프레임이 된다. 다시 시작하는 계층은 키프레임부터 나가야 시청자가 받을 수 있다.
                lock(&st.slot).keyframe = true;
            }
            if !hardware || lock(&st.slot).kbps == 0 {
                continue;
            }
            if over_cap {
                lock(&st.stats).stats.dropped_frames += 1;
                continue;
            }
            let Some(copy) = frame.clone_ref() else {
                tracing::warn!(layer = %st.layer, "simulcast 프레임 참조를 만들지 못해 이 프레임을 건너뜁니다");
                continue;
            };
            offer(
                st,
                Input {
                    frame: copy,
                    qpc,
                    arrival_qpc,
                    codec,
                    encoder: name.to_owned(),
                    fps,
                },
            );
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

/// 계층의 대기 칸에 입력을 넣고 깨운다. 칸에 앞 입력이 아직 있으면 덮어쓴다(그 프레임 참조는 놓는다).
fn offer(st: &LayerState, input: Input) {
    let old = {
        let mut s = lock(&st.slot);
        let old = s.input.replace(input);
        if old.is_some() {
            lock(&st.stats).stats.dropped_frames += 1;
        }
        st.cond.notify_one();
        old
    };
    // 놓는 일(av_frame_free)은 잠금 밖에서 한다.
    drop(old);
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
    /// 마지막으로 열지 못한 (인코더 이름, 설정, 입력 풀 세대, 시각)
    failed: Option<(String, EncoderParams, Option<u64>, Instant)>,
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
        let (mut input, kbps, key) = {
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
        let done = kbps == 0 || encode_one(&state, &sink, &mut input, kbps, key, &mut e);
        // 프레임 참조는 인코딩이 끝나면 바로 놓는다(버퍼가 풀로 돌아간다). 인코더가 아직 쥐고 있으면 그쪽이 놓을 때 돌아간다.
        drop(input);
        if key && !done {
            // 인코딩하지 못했으면 요청을 다음 프레임에 넘긴다.
            lock(&state.slot).keyframe = true;
        }
    }
}

fn set_error(state: &LayerState, e: String) {
    lock(&state.stats).stats.last_error = Some(e);
}

/// 한 프레임을 인코딩한다. 인코딩했으면(출력이 없어도) `true`.
fn encode_one(
    state: &LayerState,
    sink: &LayerSink,
    input: &mut Input,
    kbps: u32,
    mut key: bool,
    e: &mut Encoding,
) -> bool {
    let layer = state.layer;
    let codec = input.codec;
    let (qpc, arrival_qpc) = (input.qpc, input.arrival_qpc);
    // 인코더 크기는 입력 프레임의 크기를 따른다(다시 열기 판단은 아래에서 그대로).
    let (width, height) = {
        let f = input.frame_for(layer);
        (f.width() as u32, f.height() as u32)
    };
    let params = EncoderParams {
        width,
        height,
        fps: input.fps,
        bitrate_kbps: kbps,
    };
    // hw 프레임이면 그 풀의 세대. 인코더는 연 frames ctx의 프레임만 받으므로(풀이 다시 만들어지면 같은 크기여도 세대가
    // 다르다) 세대가 다르면 다시 연다.
    let input_gen = input.frame_for(layer).pool_generation();
    let current = e
        .enc
        .as_ref()
        .filter(|x| {
            x.info.codec == codec && x.info.name == input.encoder && x.hw_generation() == input_gen
        })
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
            if e.failed.as_ref().is_some_and(|(n, p, g, t)| {
                *n == input.encoder
                    && *p == params
                    && *g == input_gen
                    && t.elapsed() < REOPEN_BACKOFF
            }) {
                return false;
            }
            // hw 프레임이면 그 프레임의 frames ctx로 연다(D3D11 텍스처 입력).
            let hw = HwFramesRef::of_frame(input.frame_for(layer));
            let opened = if input_gen.is_some() && hw.is_none() {
                Err(crate::MediaError::Encoder {
                    encoder: input.encoder.clone(),
                    message: "hw 프레임의 frames ctx 참조를 만들지 못했습니다".into(),
                })
            } else {
                open_named(codec, &input.encoder, &params, hw.as_ref())
            };
            match opened {
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
                    e.failed = Some((input.encoder.clone(), params, input_gen, Instant::now()));
                    if input_gen.is_some() {
                        // 계층은 CPU 프레임이 없어 스스로 내려갈 수 없다. 워커가 파이프라인 전체를 CPU 경로로 내린다.
                        lock(&state.hw_failed).get_or_insert_with(|| err.to_string());
                    }
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
    let frame = input.frame_for(layer);
    if !enc.accepts(frame) {
        // 크기는 위에서 맞췄으니 픽셀 형식이 다른 경우다(같은 인코더라 보통 없다).
        set_error(state, "입력 프레임 형식이 인코더와 다릅니다".into());
        return false;
    }
    let t = Instant::now();
    let packets = &mut e.packets;
    let r = enc.encode(frame, qpc, arrival_qpc, key, &mut |p| packets.push(p));
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
    use crate::ff::FramePool;
    use crate::yuv::Yuv;

    fn test_state() -> LayerState {
        LayerState {
            layer: Layer::Mid,
            slot: Mutex::new(Slot {
                input: None,
                stop: false,
                kbps: 4000,
                keyframe: true,
            }),
            cond: Condvar::new(),
            stats: Mutex::new(StatsState {
                stats: LayerStats {
                    layer: Layer::Mid,
                    encoder: String::new(),
                    width: 0,
                    height: 0,
                    target_kbps: 4000,
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
            started: Instant::now(),
            hw_failed: Arc::new(Mutex::new(None)),
        }
    }

    fn input_of(frame: FrameRef) -> Input {
        Input {
            frame,
            qpc: 0,
            arrival_qpc: 0,
            codec: VideoCodec::H264,
            encoder: "h264_nvenc".into(),
            fps: 60,
        }
    }

    /// 계층 입력은 픽셀을 복사하지 않고 같은 버퍼를 참조한다. 워커가 새 프레임을 받아 써도 계층이 쥔 프레임은 그대로다.
    #[test]
    fn layer_input_shares_buffer_and_worker_does_not_overwrite_it() {
        let mut pool =
            FramePool::new(64, 32, ffmpeg_sys_next::AVPixelFormat::AV_PIX_FMT_NV12).expect("풀");
        let mut a = pool.get().expect("프레임");
        a.view_mut().expect("쓰기").fill(Yuv(100, 128, 128));
        let mut input = input_of(a.clone_ref().expect("참조"));
        // 같은 버퍼(복사 없음). 나눠 쥐는 동안에는 쓰기 보기를 줄 수 없다.
        // SAFETY: 읽기만 한다.
        unsafe {
            assert_eq!((*a.as_ptr()).data[0], (*input.frame.as_ptr()).data[0]);
        }
        assert!(a.view_mut().is_none() && !input.frame_for(Layer::Mid).is_writable());
        // 워커는 원래 프레임을 놓고 다음 프레임을 새 버퍼에 쓴다.
        drop(a);
        let mut b = pool.get().expect("프레임");
        b.view_mut().expect("쓰기").fill(Yuv(200, 128, 128));
        assert_eq!(pool.allocated(), 2);
        let view = input.frame_for(Layer::Mid);
        // SAFETY: 읽기만 한다(첫 휘도 바이트).
        assert_eq!(unsafe { *(*view.as_ptr()).data[0] }, 100);
        // 계층이 놓으면 버퍼는 풀로 돌아와 다시 쓰인다(새로 만들지 않는다).
        drop(input);
        let _c = pool.get().expect("프레임");
        assert_eq!(pool.allocated(), 2);
    }

    /// 계층이 느려도 워커는 기다리지 않고 대기 칸을 덮어쓴다. 살아 있는 프레임 수는 늘지 않는다.
    #[test]
    fn slow_layer_keeps_only_the_latest_and_live_frames_stay_bounded() {
        let mut pool =
            FramePool::new(64, 32, ffmpeg_sys_next::AVPixelFormat::AV_PIX_FMT_NV12).expect("풀");
        let st = test_state();
        for _ in 0..200 {
            let mut f = pool.get().expect("프레임");
            f.view_mut().expect("쓰기").fill(Yuv(90, 128, 128));
            offer(&st, input_of(f.clone_ref().expect("참조")));
        }
        assert_eq!(lock(&st.stats).stats.dropped_frames, 199);
        // 워커 쪽 프레임 하나 + 칸의 입력 하나만 살아 있었다(풀에서 새로 만든 버퍼는 2개 이하).
        assert!(pool.allocated() <= 2, "{}", pool.allocated());
        // 스레드가 가져가 놓으면 마지막 버퍼도 돌아온다.
        drop(lock(&st.slot).input.take());
        let _a = pool.get().expect("프레임");
        let _b = pool.get().expect("프레임");
        assert!(pool.allocated() <= 2, "{}", pool.allocated());
    }

    /// 풀을 먼저 버려도(인코더를 다시 열 때) 계층이 쥔 프레임은 유효하다.
    #[test]
    fn frame_outlives_its_pool() {
        let mut pool =
            FramePool::new(64, 32, ffmpeg_sys_next::AVPixelFormat::AV_PIX_FMT_NV12).expect("풀");
        let mut f = pool.get().expect("프레임");
        f.view_mut().expect("쓰기").fill(Yuv(77, 128, 128));
        let mut kept = f.clone_ref().expect("참조");
        drop(f);
        drop(pool);
        assert_eq!(unsafe { *(*kept.as_mut_ptr()).data[0] }, 77);
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

    /// 주 인코더가 소프트웨어라 계층이 쉬었다가(코덱 바꾸기 h264 → av1 → h264) 다시 hw로 돌아오면, 남아 있던 계층 인코더가 이어
    /// 인코딩하므로 키프레임을 요청한 상태가 되어야 한다.
    #[test]
    fn software_pause_resume_requests_keyframe() {
        let sink: LayerSink = Arc::new(|_| {});
        let layers = Layers::start(sink, 8000);
        let mut pool =
            FramePool::new(64, 32, ffmpeg_sys_next::AVPixelFormat::AV_PIX_FMT_NV12).expect("풀");
        let frame = pool.get().expect("프레임");
        let paused = |l: &Layers| {
            l.stats()
                .iter()
                .map(|s| s.software_paused)
                .collect::<Vec<_>>()
        };
        let submit = |hardware: bool| {
            // hw 프레임 상한을 넘겨(`over_cap`) 계층 스레드에 일을 주지 않고 쉼 상태만 바꾼다.
            let live = if hardware { MAX_LIVE_HW_FRAMES + 1 } else { 0 };
            layers.submit(
                &frame,
                0,
                0,
                VideoCodec::H264,
                ("h264_nvenc", hardware),
                60,
                live,
            );
        };
        submit(false);
        assert_eq!(paused(&layers), [true, true]);
        for w in &layers.layers {
            lock(&w.state.slot).keyframe = false;
        }
        assert!(!layers.wants_keyframe());
        // 소프트웨어 상태가 이어지는 동안은 요청이 없다.
        submit(false);
        assert!(!layers.wants_keyframe());
        // hw로 돌아오면 모든 계층이 키프레임을 요청한다.
        submit(true);
        assert_eq!(paused(&layers), [false, false]);
        assert!(layers.layers.iter().all(|w| lock(&w.state.slot).keyframe));
        // 이미 hw인 채로 계속되면 새 요청은 없다.
        for w in &layers.layers {
            lock(&w.state.slot).keyframe = false;
        }
        submit(true);
        assert!(!layers.wants_keyframe());
    }
}
