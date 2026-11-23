//! simulcast 하위 계층(D-098): 주 인코더(상)와 같은 코덱으로 비트레이트만 낮춘 인코더를 계층(중·하)마다 스레드 하나씩 더
//! 돌린다. 해상도는 기본으로 주 인코더와 같고(계층마다 자기 QP로 낮출 수 있다, D-099), fps는 주 fps가 60 이하면 같고 60보다
//! 크면 60으로 제한한다(D-100, [`LAYER_MAX_FPS`]).
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
//! - 중·하 fps 상한(D-100): 주 fps가 60보다 크면(예: 144) 워커가 넘기는 프레임 중에서 캡처 시각(qpc) 기준으로 평균 60fps가
//!   되게 고른다([`FramePicker`]). 중·하가 같은 프레임을 고르고(고르기 상태 하나를 같이 쓴다), 고르지 않은 프레임은 계층에
//!   넘기지 않으며 `dropped_frames`로도 세지 않는다. 계층 인코더는 fps 60으로 열린다(VBV·GOP도 60 기준). 주 fps가 60 이하면
//!   모든 프레임을 계층에 넘긴다.
//! - 키프레임은 계층마다 따로 요청한다([`Layers::request_keyframe`]). 계층 인코더를 새로 열거나 쉬다가 다시 켜면 그
//!   첫 프레임도 키프레임이다.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dongsi_core::priority::{ThreadRole, boost_current_thread};
use dongsi_core::qp_scale::{QpFrame, QpScaler};
use dongsi_core::{Layer, LayerRates, VideoCodec, VideoSettings};
use serde::Serialize;

use crate::autoscale;
use crate::clock::{QPC_PER_SEC, instant_from_qpc};
use crate::encoder::{EncodedPacket, EncoderParams, VideoEncoder, open_named};
use crate::ff::{FrameRef, HwFramesRef};
use crate::video::EncodedVideo;

/// 계층 프레임을 받는 곳. 계층 스레드에서 불린다. 오래 막으면 안 된다(채널로 넘기는 정도).
pub type LayerSink = Arc<dyn Fn(EncodedVideo) + Send + Sync>;

/// 주 인코더가 아닌 계층(주 인코더가 상).
const SUB_LAYERS: [Layer; 2] = [Layer::Mid, Layer::Low];
/// 중·하 계층의 fps 상한(D-100). 주 fps가 이보다 크면 계층은 이 fps로 프레임을 골라 인코딩한다.
pub(crate) const LAYER_MAX_FPS: u32 = 60;
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
    /// 최근 3초 이동 평균 QP(코덱 고유 범위: H.264 0~51, AV1 qindex 0~255). 인코더가 QP를 알려 주지 않거나, 표본이
    /// 모자라거나(연 직후·키프레임 직후·프레임이 드묾), 쉬는 중이면 `None`.
    pub qp: Option<f32>,
    /// 자동 해상도의 현재 단계(0 = 설정한 상자 그대로, 1 = 한 단계 낮춤). 꺼져 있으면 늘 0이다.
    pub auto_step: usize,
    /// 자동 해상도가 이 계층을 설정보다 낮춰 인코딩 중이다(`auto_step` > 0).
    pub auto_lowered: bool,
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
    /// 계층 인코더를 여는 fps: 주 fps를 [`LAYER_MAX_FPS`]로 제한한 값.
    fps: u32,
    /// 이 프레임을 만들 때 계층이 쓰던 자동 해상도 단계. 계층이 단계를 바꾸기 전에 만든 프레임(옛 크기)은 계층 스레드가
    /// 버린다(크기가 다른 프레임으로 인코더를 다시 열지 않게).
    step: usize,
}

impl Input {
    /// `layer`가 인코딩할 프레임. 워커가 계층마다 그 계층의 크기(자동 해상도 단계)로 변환한 프레임을 넘기므로 입력 하나가
    /// 프레임 하나다. 같은 크기를 쓰는 계층은 같은 버퍼를 가리킨다.
    fn frame_for(&mut self, _layer: Layer) -> &mut FrameRef {
        &mut self.frame
    }
}

/// 계층의 자동 해상도(D-099) 공유 상태: 워커(켜기·끄기·단계 목록·크기 읽기)와 계층 스레드(판정·단계 바꾸기)가 같이 쓴다.
struct LayerAuto {
    enabled: bool,
    codec: VideoCodec,
    /// 단계별 출력 상자(0 = 설정 상자). 비어 있으면 자동 해상도를 쓰지 않는다(설정 상자 그대로).
    steps: Vec<(u32, u32)>,
    /// `steps`·`codec`이 바뀔 때마다 는다. 계층 스레드가 판정기를 다시 만들 때를 안다.
    epoch: u64,
    /// 지금 단계.
    step: usize,
}

/// 워커가 계층에 넘길 프레임 하나(크기별로 하나).
pub(crate) struct LayerFrame<'a> {
    pub size: (u32, u32),
    pub frame: &'a FrameRef,
    /// hw 프레임이면 그 풀에서 지금 살아 있는 프레임 수(CPU 프레임이면 0).
    pub live: usize,
}

/// 계층이 이번 프레임에 원하는 크기.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LayerWant {
    pub layer: Layer,
    pub step: usize,
    pub size: (u32, u32),
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
    auto: Mutex<LayerAuto>,
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
    /// 중·하가 같이 쓰는 fps 상한 고르기 상태(D-100).
    picker: Mutex<FramePicker>,
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
                            qp: None,
                            auto_step: 0,
                            auto_lowered: false,
                        },
                        encoded: VecDeque::new(),
                    }),
                    auto: Mutex::new(LayerAuto {
                        enabled: false,
                        codec: VideoCodec::H264,
                        steps: Vec::new(),
                        epoch: 0,
                        step: 0,
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
        tracing::info!(
            high_kbps,
            max_fps = LAYER_MAX_FPS,
            "simulcast 계층 켬(중·하, 주 인코더와 같은 해상도에서 시작, fps는 주 fps와 같되 최대 {LAYER_MAX_FPS})"
        );
        Layers {
            layers,
            hw_failed,
            last_live_warn: Mutex::new(None),
            picker: Mutex::new(FramePicker::default()),
        }
    }

    /// hw 입력 계층 인코더가 열리지 않았다면 그 이유(한 번 가져가면 비운다). 워커가 보고 CPU 경로로 내린다.
    pub fn take_hw_failure(&self) -> Option<String> {
        lock(&self.hw_failed).take()
    }

    fn worker(&self, layer: Layer) -> Option<&Worker> {
        self.layers.iter().find(|w| w.state.layer == layer)
    }

    /// 자동 해상도의 단계 목록(상자 크기, 0 = 설정 상자)과 코덱(임계값)을 정한다. 모든 계층이 처음 단계로 돌아가고 판정기를
    /// 다시 만든다. 설정(코덱·해상도 상자)이 바뀔 때 부른다.
    pub fn configure_auto(&self, codec: VideoCodec, steps: &[(u32, u32)]) {
        for w in &self.layers {
            let mut a = lock(&w.state.auto);
            a.codec = codec;
            a.steps = steps.to_vec();
            a.epoch += 1;
            a.step = 0;
        }
    }

    /// 자동 해상도를 켜고 끈다. 끄면 낮춰 둔 계층은 설정 크기로 돌아간다(다음 프레임부터).
    pub fn set_auto(&self, enabled: bool) {
        for w in &self.layers {
            lock(&w.state.auto).enabled = enabled;
        }
    }

    /// 인코딩할 계층(주 인코더가 하드웨어이고 비트레이트가 0이 아닌 계층)이 이번 프레임에 원하는 크기. 원본 크기
    /// `src`(`settings`의 상자에서 계층 단계의 상자로 바꿔 계산한다).
    pub fn wants(
        &self,
        settings: &VideoSettings,
        src: (u32, u32),
        hardware: bool,
    ) -> Vec<LayerWant> {
        if !hardware {
            return Vec::new();
        }
        self.layers
            .iter()
            .filter(|w| lock(&w.state.slot).kbps > 0)
            .map(|w| {
                let a = lock(&w.state.auto);
                let (bw, bh) = a
                    .steps
                    .get(a.step)
                    .copied()
                    .unwrap_or((settings.width, settings.height));
                let size = VideoSettings {
                    width: bw,
                    height: bh,
                    ..*settings
                }
                .output_size(src.0, src.1);
                LayerWant {
                    layer: w.state.layer,
                    step: a.step,
                    size,
                }
            })
            .collect()
    }

    /// [`wants`](Self::wants)와 같되, 이번 프레임이 중·하의 fps 상한(D-100)에서 고르지 않은 프레임이면 아무 계층도 원하지
    /// 않는다(워커가 그 크기 프레임을 만들지 않는다). 프레임 `qpc`·`arrival_qpc`와 주 fps `fps`로 고르며, 이 결정은 같은
    /// 프레임의 [`submit`](Self::submit)이 그대로 쓴다(둘이 어긋나지 않는다). 프레임마다 한 번만 부른다.
    pub fn wants_for(
        &self,
        settings: &VideoSettings,
        src: (u32, u32),
        hardware: bool,
        qpc: i64,
        arrival_qpc: i64,
        fps: u32,
    ) -> Vec<LayerWant> {
        if !lock(&self.picker).pick(qpc, arrival_qpc, fps) {
            return Vec::new();
        }
        self.wants(settings, src, hardware)
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
    /// `frames`는 크기별 변환 결과다. 계층은 `wants`(워커가 [`wants`](Self::wants)로 구해 그 크기의 프레임을 만들었다)에 적힌
    /// 크기의 프레임을 받는다. 프레임의 `live`는 D3D11 hw 프레임일 때 그 풀에서 지금 살아 있는 프레임 수다.
    /// [`MAX_LIVE_HW_FRAMES`]를 넘으면 경고하고(5초에 한 번) 그 프레임은 계층에 넘기지 않는다(`dropped_frames`).
    ///
    /// 주 fps(`fps`)가 [`LAYER_MAX_FPS`]보다 크면 이 프레임을 계층이 골랐는지([`FramePicker`])에 따라 넘긴다. 고르지 않은
    /// 프레임은 대기 칸에 넣지 않고 `dropped_frames`로도 세지 않는다. 결정은 [`wants_for`](Self::wants_for)가 이미 했으면
    /// 그대로 쓴다(같은 `qpc`·`arrival_qpc`·`fps`).
    #[allow(clippy::too_many_arguments)]
    pub fn submit(
        &self,
        frames: &[LayerFrame<'_>],
        wants: &[LayerWant],
        qpc: i64,
        arrival_qpc: i64,
        codec: VideoCodec,
        encoder: (&str, bool),
        fps: u32,
    ) {
        let (name, hardware) = encoder;
        let picked = lock(&self.picker).pick(qpc, arrival_qpc, fps);
        let hw_live = frames.iter().map(|f| f.live).max().unwrap_or(0);
        if hw_live > MAX_LIVE_HW_FRAMES && hardware {
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
            if !hardware || !picked || lock(&st.slot).kbps == 0 {
                continue;
            }
            let Some(want) = wants.iter().find(|x| x.layer == st.layer) else {
                continue;
            };
            let Some(lf) = frames.iter().find(|f| f.size == want.size) else {
                tracing::warn!(layer = %st.layer, size = ?want.size, "계층이 원하는 크기의 프레임이 없어 건너뜁니다");
                continue;
            };
            if lf.live > MAX_LIVE_HW_FRAMES {
                lock(&st.stats).stats.dropped_frames += 1;
                continue;
            }
            let Some(copy) = lf.frame.clone_ref() else {
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
                    fps: fps.min(LAYER_MAX_FPS),
                    step: want.step,
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
                if st.encoded.is_empty() {
                    s.qp = None; // 쉬는 중(옛 값을 보이지 않는다)
                }
                let a = lock(&l.state.auto);
                s.auto_step = a.step;
                s.auto_lowered = a.step > 0;
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

/// 중·하 계층이 주 프레임 중 어느 것을 인코딩할지 캡처 시각(qpc, 100ns) 기준으로 고른다(D-100). 중·하가 같이 쓴다.
///
/// - 주 fps가 [`LAYER_MAX_FPS`] 이하면 모든 프레임을 고른다.
/// - 아니면 60Hz 격자(다음 기한 = 직전 기한 + 1/60초)에서 **캡처 시각이 기한에서 `slack`을 뺀 시각 이상**인 첫 프레임을
///   고른다. `slack`은 주 프레임 간격의 절반(`1/(2·fps)`)이다: 기한에 가장 가까운 프레임을 고르는 셈이라 144 → 60은
///   3·2·3·2 장 간격(프레임이 기한 앞뒤 3.5ms 안)으로, 120 → 60은 한 장 걸러 하나로 고르게 나온다. 캡처 시각이 조금 흔들려
///   (120fps에서 프레임이 16.66ms보다 3ms 일찍 와도) 한 장 어긋나지 않는다. 기한은 `+= 1/60초`로 나아가므로 평균이 60fps다.
/// - 기한보다 한 주기 넘게 늦은 프레임(오래 쉬었다 옴, 화면이 멈췄다 움직임)이거나 기한이 한 주기+`slack` 넘게 앞서 있으면
///   (캡처 시각이 뒤로 갔다: 대상 바꾸기) 그 프레임을 고르고 `프레임 시각 + 1/60초`로 격자를 다시 맞춘다.
/// - 멈춘 화면 다시 보내기 프레임(워커가 마지막 화면을 지금 시각으로 다시 넘긴다: `qpc == arrival_qpc`)은 늘 고른다. 이
///   프레임의 시각은 옛 캡처가 아니라 지금이라 격자에서 어긋나 영영 빠지면 키프레임을 기다리는 계층이 못 받는다.
/// - 같은 프레임(같은 `qpc`·`arrival_qpc`·`fps`)을 다시 물으면 앞의 결정을 돌려준다(`wants_for`와 `submit`).
#[derive(Debug, Default)]
pub(crate) struct FramePicker {
    /// 다음 기한(qpc). 아직 고른 프레임이 없거나 격자를 쓰지 않으면 `None`.
    due: Option<i64>,
    /// 마지막 질문과 답.
    last: Option<((i64, i64, u32), bool)>,
}

impl FramePicker {
    pub fn pick(&mut self, qpc: i64, arrival_qpc: i64, fps: u32) -> bool {
        let key = (qpc, arrival_qpc, fps);
        if let Some((k, picked)) = self.last
            && k == key
        {
            return picked;
        }
        let picked = self.decide(qpc, arrival_qpc, fps);
        self.last = Some((key, picked));
        picked
    }

    fn decide(&mut self, qpc: i64, arrival_qpc: i64, fps: u32) -> bool {
        if fps <= LAYER_MAX_FPS {
            self.due = None;
            return true;
        }
        let period = QPC_PER_SEC / LAYER_MAX_FPS as i64;
        let slack = QPC_PER_SEC / (2 * fps as i64);
        let Some(due) = self.due else {
            self.due = Some(qpc + period);
            return true;
        };
        let resend = qpc == arrival_qpc;
        let backwards = due - qpc > period + slack;
        if !resend && !backwards && qpc + slack < due {
            return false;
        }
        // 기한이 한 주기 넘게 늦었거나 격자를 믿을 수 없으면 이 프레임으로 다시 맞춘다.
        self.due = Some(if resend || backwards || qpc - due > period {
            qpc + period
        } else {
            due + period
        });
        true
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
    /// 이 계층의 QP 판정기(D-099). 단계 목록이 정해지기 전에는 `None`.
    scaler: Option<QpScaler>,
    /// `scaler`를 만든 `LayerAuto::epoch`.
    scaler_epoch: u64,
    /// 마지막으로 인코딩한 시각. 오래 쉬었다 돌아오면(쉼·소프트웨어 쉼·멈춘 화면) 판정을 처음부터 센다.
    last_encoded: Option<Instant>,
    enc: Option<VideoEncoder>,
    /// 마지막으로 열지 못한 (인코더 이름, 설정, 입력 풀 세대, 시각)
    failed: Option<(String, EncoderParams, Option<u64>, Instant)>,
    packets: Vec<EncodedPacket>,
}

/// 계층 스레드: 입력을 기다렸다가 자기 인코더로 인코딩한다.
fn run(state: Arc<LayerState>, sink: LayerSink) {
    let _boost = boost_current_thread(ThreadRole::Video);
    let mut e = Encoding {
        scaler: None,
        scaler_epoch: 0,
        last_encoded: None,
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
        let step = sync_auto(&state, &mut e);
        let done = if kbps == 0 {
            true
        } else if input.step != step {
            // 단계를 바꾸기 전에 만든 프레임(옛 크기)이다. 새 크기 프레임이 올 때까지 기다린다.
            false
        } else {
            if e.last_encoded.is_some_and(|t| t.elapsed() > RESUME_GAP)
                && let Some(sc) = e.scaler.as_mut()
            {
                sc.reset(Instant::now());
            }
            encode_one(&state, &sink, &mut input, kbps, key, &mut e)
        };
        // 프레임 참조는 인코딩이 끝나면 바로 놓는다(버퍼가 풀로 돌아간다). 인코더가 아직 쥐고 있으면 그쪽이 놓을 때 돌아간다.
        drop(input);
        if key && !done {
            // 인코딩하지 못했으면 요청을 다음 프레임에 넘긴다.
            lock(&state.slot).keyframe = true;
        }
    }
}

/// 이만큼 인코딩하지 못하다 돌아오면 QP 판정을 처음부터 센다.
const RESUME_GAP: Duration = Duration::from_secs(1);

/// 자동 해상도 공유 상태를 계층 스레드의 판정기에 맞춘다. 지금 단계를 돌려준다.
///
/// - 단계 목록·코덱이 바뀌었으면(`epoch`) 판정기를 다시 만든다(그때 단계는 0으로 돌아가 있다).
/// - 꺼져 있는데 낮춰 둔 단계면 0으로 되돌린다(켜 있는 동안만 단계를 바꾼다).
fn sync_auto(state: &LayerState, e: &mut Encoding) -> usize {
    let mut a = lock(&state.auto);
    if e.scaler_epoch != a.epoch {
        e.scaler = (!a.steps.is_empty())
            .then(|| QpScaler::new(autoscale::config(a.codec), a.steps.clone(), Instant::now()));
        e.scaler_epoch = a.epoch;
    }
    if !a.enabled && a.step != 0 {
        a.step = 0;
        if let Some(sc) = e.scaler.as_mut() {
            sc.switched(0, Instant::now());
        }
    }
    a.step
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
                    if let Some(sc) = e.scaler.as_mut() {
                        sc.reset(Instant::now());
                    }
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
    e.last_encoded = Some(Instant::now());
    let mut next_step = None;
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
            if let (Some(q), Some(sc)) = (p.qp, e.scaler.as_mut()) {
                let v = sc.push(QpFrame {
                    at: now,
                    qp: q.qp,
                    key: p.keyframe || q.intra,
                });
                if v.next.is_some() {
                    next_step = v.next;
                }
            }
        }
        st.stats.qp = e.scaler.as_ref().and_then(QpScaler::average_qp);
        prune(&mut st.encoded, now);
        if let Err(err) = &r {
            st.stats.last_error = Some(err.to_string());
        }
    }
    if let Some(to) = next_step {
        // 켜 있을 때만 단계를 바꾼다(꺼져 있으면 판정은 통계용이다). 워커가 다음 프레임부터 새 크기로 넘긴다.
        let mut a = lock(&state.auto);
        if a.enabled
            && a.epoch == e.scaler_epoch
            && let Some(sc) = e.scaler.as_mut()
        {
            let from = a.step;
            a.step = to;
            sc.switched(to, Instant::now());
            tracing::info!(%layer, from, to, qp = ?lock(&state.stats).stats.qp, "QP 기반 해상도 자동 낮추기·올리기(simulcast 계층)");
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
                    qp: None,
                    auto_step: 0,
                    auto_lowered: false,
                },
                encoded: VecDeque::new(),
            }),
            auto: Mutex::new(LayerAuto {
                enabled: false,
                codec: VideoCodec::H264,
                steps: Vec::new(),
                epoch: 0,
                step: 0,
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
            step: 0,
        }
    }

    /// fps에 맞는 캡처 시각 열(100ns). `jitter`는 프레임마다 ±`jitter`(100ns) 안에서 흔든다(고정 시드).
    fn capture_times(fps: u32, count: usize, jitter: i64) -> Vec<i64> {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        (0..count)
            .map(|i| {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let j = if jitter == 0 {
                    0
                } else {
                    ((seed >> 33) as i64 % (2 * jitter + 1)) - jitter
                };
                1_000_000_000 + i as i64 * QPC_PER_SEC / fps as i64 + j
            })
            .collect()
    }

    /// 고른 프레임의 캡처 시각과 인덱스.
    fn picked(p: &mut FramePicker, times: &[i64], fps: u32) -> Vec<(usize, i64)> {
        times
            .iter()
            .enumerate()
            .filter(|&(_, &t)| p.pick(t, t + 20_000, fps))
            .map(|(i, &t)| (i, t))
            .collect()
    }

    fn gaps_ms(p: &[(usize, i64)]) -> Vec<f64> {
        p.windows(2)
            .map(|w| (w[1].1 - w[0].1) as f64 / 10_000.0)
            .collect()
    }

    /// 60fps 이하는 모든 프레임이 계층으로 간다.
    #[test]
    fn picker_keeps_every_frame_at_or_below_60() {
        for fps in [60, 30, 24, 1] {
            let times = capture_times(fps, 200, 0);
            let mut p = FramePicker::default();
            assert_eq!(picked(&mut p, &times, fps).len(), 200, "{fps}fps");
        }
        // 흔들려도 같다.
        let times = capture_times(60, 200, 30_000);
        assert_eq!(picked(&mut FramePicker::default(), &times, 60).len(), 200);
    }

    /// 120 → 60: 한 장 걸러 하나, 간격 16.7ms. 캡처 시각이 ±3ms 흔들려도 같다(여유 `1/(2·fps)` = 4.17ms 안).
    #[test]
    fn picker_takes_every_other_frame_at_120() {
        let times = capture_times(120, 1200, 0);
        let got = picked(&mut FramePicker::default(), &times, 120);
        assert_eq!(got.len(), 600);
        assert!(got.iter().all(|&(i, _)| i % 2 == 0), "한 장 걸러 하나");
        assert!(gaps_ms(&got).iter().all(|g| (g - 16.666).abs() < 0.01));
        let times = capture_times(120, 1200, 30_000);
        let got = picked(&mut FramePicker::default(), &times, 120);
        assert_eq!(got.len(), 600, "흔들림이 있어도 600장(10초)");
        assert!(got.iter().all(|&(i, _)| i % 2 == 0));
        // 간격은 기한 격자를 따라 16.7ms ± 흔들림(양쪽 합쳐 6ms).
        assert!(gaps_ms(&got).iter().all(|g| (g - 16.666).abs() <= 6.1));
    }

    /// 144 → 60: 10초에 600장, 간격은 2·3장 번갈아(13.9ms·20.8ms), 격자에서 3.5ms 넘게 벗어나지 않는다.
    #[test]
    fn picker_holds_60_at_144() {
        let times = capture_times(144, 1440, 0);
        let got = picked(&mut FramePicker::default(), &times, 144);
        assert!((599..=601).contains(&got.len()), "{}", got.len());
        let gaps = gaps_ms(&got);
        assert!(
            gaps.iter().all(|g| (13.8..=20.9).contains(g)),
            "{:?}",
            gaps.iter()
                .fold((99.0f64, 0.0f64), |a, g| (a.0.min(*g), a.1.max(*g)))
        );
        // 격자에서 벗어난 정도(첫 프레임 기준 k/60초와의 차이)가 반 프레임 간격(3.5ms) 안이다.
        let t0 = got[0].1;
        for (k, &(_, t)) in got.iter().enumerate() {
            let ideal = t0 + k as i64 * QPC_PER_SEC / 60;
            assert!(
                (t - ideal).abs() <= 36_000,
                "{k}번째: {}ms",
                (t - ideal) as f64 / 1e4
            );
        }
        let times = capture_times(144, 1440, 20_000);
        let got = picked(&mut FramePicker::default(), &times, 144);
        assert!((595..=605).contains(&got.len()), "흔들림 {}", got.len());
    }

    /// 240, 90, 75fps도 평균 60이다.
    #[test]
    fn picker_averages_60_for_other_rates() {
        for fps in [240u32, 165, 90, 75, 61] {
            let times = capture_times(fps, fps as usize * 10, 0);
            let got = picked(&mut FramePicker::default(), &times, fps);
            assert!((595..=605).contains(&got.len()), "{fps}fps: {}", got.len());
        }
    }

    /// 오래 쉬었다 오면(기한이 한 주기 넘게 지났다) 그 프레임으로 격자를 다시 맞춘다: 바로 다음 프레임을 또 고르지 않고
    /// 한 주기 뒤를 기한으로 삼는다.
    #[test]
    fn picker_resyncs_after_a_long_gap() {
        let mut p = FramePicker::default();
        let times = capture_times(120, 120, 0);
        assert_eq!(picked(&mut p, &times, 120).len(), 60);
        // 2.5초 비었다가 120fps가 다시 온다.
        let t0 = times[119] + 25_000_000;
        let after: Vec<i64> = (0..20).map(|i| t0 + i * QPC_PER_SEC / 120).collect();
        let got = picked(&mut p, &after, 120);
        assert_eq!(
            got.first().map(|g| g.0),
            Some(0),
            "공백 뒤 첫 프레임은 고른다"
        );
        assert_eq!(
            got.iter().map(|g| g.0).collect::<Vec<_>>(),
            (0..20).step_by(2).collect::<Vec<_>>()
        );
        // 기한이 한 주기 이내로 늦은 프레임은 격자를 유지한다(다음 기한 = 직전 기한 + 1/60초).
        let mut p = FramePicker::default();
        assert!(p.pick(0, 1, 144));
        assert!(!p.pick(60_000, 60_001, 144));
        assert!(p.pick(300_000, 300_001, 144)); // 기한 166_666보다 133_334 늦다(한 주기 이내)
        assert!(p.pick(333_332, 333_333, 144)); // 기한 = 166_666 + 166_666, 바로 이어 고른다
        assert!(!p.pick(340_000, 340_001, 144));
    }

    /// 캡처 시각이 뒤로 가면(대상 바꾸기) 그 프레임으로 다시 맞춘다.
    #[test]
    fn picker_resyncs_when_time_goes_backwards() {
        let mut p = FramePicker::default();
        assert!(p.pick(1_000_000_000, 1_000_000_001, 120));
        assert!(!p.pick(1_000_083_333, 1_000_083_334, 120));
        // 기한(1_000_166_666)보다 한 주기+여유 넘게 앞선 시각.
        assert!(p.pick(900_000_000, 900_000_001, 120));
        assert!(!p.pick(900_083_333, 900_083_334, 120));
        assert!(p.pick(900_166_666, 900_166_667, 120));
    }

    /// 멈춘 화면 다시 보내기(`qpc == arrival_qpc`)는 격자 위치와 상관없이 늘 고르고 격자를 거기서 다시 맞춘다.
    #[test]
    fn picker_always_takes_a_resent_frame() {
        let mut p = FramePicker::default();
        assert!(p.pick(1_000_000, 1_000_001, 144));
        // 기한(1_166_666)보다 한참 앞인 지금 시각의 다시 보내기.
        assert!(p.pick(1_050_000, 1_050_000, 144));
        // 격자는 다시 보내기 프레임에서 한 주기 뒤다.
        assert!(!p.pick(1_100_000, 1_100_001, 144));
        assert!(p.pick(1_216_666, 1_216_667, 144));
        // 60fps 이하면 어차피 모든 프레임이다.
        assert!(FramePicker::default().pick(5, 5, 60));
    }

    /// 같은 프레임을 다시 물으면(`wants_for` → `submit`) 앞의 결정을 돌려주고 격자를 또 옮기지 않는다.
    #[test]
    fn picker_repeats_its_decision_for_the_same_frame() {
        let mut p = FramePicker::default();
        assert!(p.pick(0, 10, 144));
        for _ in 0..3 {
            assert!(p.pick(0, 10, 144));
        }
        assert!(!p.pick(69_444, 69_454, 144));
        for _ in 0..3 {
            assert!(!p.pick(69_444, 69_454, 144));
        }
        // 주 fps가 60 이하로 바뀌면 격자를 버리고 모두 고른다.
        assert!(p.pick(80_000, 80_010, 30));
        assert!(p.pick(90_000, 90_010, 144)); // 144로 돌아오면 새 격자
    }

    /// `wants_for`: 고르지 않은 프레임이면 아무 계층도 원하지 않고, 고른 프레임이면 `wants`와 같다. 60 이하면 늘 `wants`와 같다.
    #[test]
    fn wants_for_follows_the_picker() {
        let sink: LayerSink = Arc::new(|_| {});
        let layers = Layers::start(sink, 8000);
        let settings = VideoSettings {
            width: 2560,
            height: 1440,
            ..VideoSettings::DEFAULT
        };
        let src = (2560, 1440);
        let all = layers.wants(&settings, src, true);
        assert_eq!(all.len(), 2);
        let w = |t: i64, fps: u32| layers.wants_for(&settings, src, true, t, t + 5, fps);
        assert_eq!(w(0, 144), all);
        assert!(w(69_444, 144).is_empty());
        assert!(w(138_888, 144).len() == 2); // 기한(166_666) − 여유(34_722) 안
        assert!(w(208_332, 144).is_empty());
        // 소프트웨어 주 인코더는 고르든 말든 원하지 않는다.
        assert!(
            layers
                .wants_for(&settings, src, false, 400_000, 400_005, 144)
                .is_empty()
        );
        // 60 이하는 모든 프레임.
        for i in 0..5 {
            assert_eq!(w(1_000_000 + i * 166_666, 60), all);
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

    /// 계층이 원하는 크기는 자기 단계의 상자로 정한다: 단계 0은 설정 상자, 단계 1은 FHD 상자. 원본이 상자보다 작으면 그대로다.
    /// 소프트웨어 주 인코더이거나 쉬는 계층(비트레이트 0)은 원하지 않는다.
    #[test]
    fn layers_want_their_own_step_size() {
        let sink: LayerSink = Arc::new(|_| {});
        let layers = Layers::start(sink, 8000);
        let settings = VideoSettings {
            width: 2560,
            height: 1440,
            ..VideoSettings::DEFAULT
        };
        let steps = dongsi_core::qp_scale::default_steps(2560, 1440);
        layers.configure_auto(VideoCodec::H264, &steps);
        let sizes = |l: &Layers, src: (u32, u32), hw: bool| {
            l.wants(&settings, src, hw)
                .iter()
                .map(|w| (w.layer, w.step, w.size))
                .collect::<Vec<_>>()
        };
        let qhd = (2560, 1440);
        let fhd = (1920, 1080);
        assert_eq!(
            sizes(&layers, qhd, true),
            vec![(Layer::Mid, 0, qhd), (Layer::Low, 0, qhd)]
        );
        assert!(sizes(&layers, qhd, false).is_empty());
        // Low만 한 단계 낮춘다.
        lock(&layers.layers[1].state.auto).step = 1;
        assert_eq!(
            sizes(&layers, qhd, true),
            vec![(Layer::Mid, 0, qhd), (Layer::Low, 1, fhd)]
        );
        // 원본이 FHD면 두 단계가 같은 크기다(상자 안).
        assert_eq!(
            sizes(&layers, fhd, true),
            vec![(Layer::Mid, 0, fhd), (Layer::Low, 1, fhd)]
        );
        // 쉬는 계층은 원하지 않는다.
        layers.set_rates([8000, 4000, 0]);
        assert_eq!(sizes(&layers, qhd, true), vec![(Layer::Mid, 0, qhd)]);
        // configure하면 처음 단계로 돌아간다.
        layers.configure_auto(VideoCodec::H264, &steps);
        assert_eq!(lock(&layers.layers[1].state.auto).step, 0);
        // 통계가 단계를 보인다.
        lock(&layers.layers[0].state.auto).step = 1;
        let st = layers.stats();
        assert_eq!((st[0].auto_step, st[0].auto_lowered), (1, true));
        assert_eq!((st[1].auto_step, st[1].auto_lowered), (0, false));
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
            // 계층이 원하는 크기(`wants`)를 주지 않아 계층 스레드에 일을 주지 않고 쉼 상태만 바꾼다.
            let live = if hardware { MAX_LIVE_HW_FRAMES + 1 } else { 0 };
            let frames = [LayerFrame {
                size: (64, 32),
                frame: &frame,
                live,
            }];
            layers.submit(
                &frames,
                &[],
                0,
                0,
                VideoCodec::H264,
                ("h264_nvenc", hardware),
                60,
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
