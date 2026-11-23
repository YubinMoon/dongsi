//! 비디오 파이프라인: 캡처(WGC 또는 테스트 패턴) → YUV 변환 → 인코딩.
//!
//! 스레드 구성
//! - WGC 콜백(시스템 스레드): 프레임 수 제한(fps)을 적용하고, 통과한 프레임을
//!   "최신 프레임 칸" 하나에 넣는다. 워커가 아직 이전 프레임을 처리 중이면 칸의
//!   이전 프레임은 버린다(dropped). 그래서 밀린 프레임이 쌓이지 않는다.
//! - 워커 스레드: 칸에서 프레임을 꺼내 보관 텍스처로 복사하고(캡처 프레임은 바로 반납),
//!   변환·인코딩한 뒤 `on_frame`을 부른다. 테스트 패턴은 워커가 직접 틱마다 그린다.
//! - simulcast(D-098, [`VideoPipeline::start_simulcast`]): 워커가 변환한 프레임을 계층(중·하) 스레드에도 넘기고,
//!   계층 스레드가 자기 인코더로 인코딩해 계층 `sink`를 부른다([`crate::simulcast`]).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dongsi_core::priority::{ThreadRole, boost_current_thread};
use dongsi_core::{Layer, LayerRates, VideoCodec, VideoSettings};
use serde::Serialize;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};

use crate::capture::{CapturedFrame, WgcCapture};
use crate::clock::{QPC_PER_SEC, instant_from_qpc, qpc_ms, qpc_now_100ns};
use crate::convert::Converter;
use crate::d3d::{D3d, create_d3d};
use crate::encoder::{EncodedPacket, EncoderParams, VideoEncoder, candidates_for, open_best};
use crate::preview::{PreviewFrame, PreviewScaler, PreviewSlot};
use crate::simulcast::{LayerSink, LayerStats, Layers};
use crate::source::refresh_rate;
use crate::yuv::{pattern_is_white, pattern_next_tick, pattern_source_size, render_pattern};
use crate::{MediaError, SourceId};

/// 인코딩된 비디오 프레임 하나.
#[derive(Debug, Clone)]
pub struct EncodedVideo {
    /// simulcast 계층(D-098). 주 인코더 프레임과 simulcast를 켜지 않은 파이프라인은 상(`Layer::High`)이다.
    pub layer: Layer,
    pub codec: VideoCodec,
    /// - H.264: Annex B(4바이트 시작 코드). 키프레임마다 SPS/PPS가 IDR 앞에 들어 있다.
    ///   AUD·filler NAL은 없다.
    /// - AV1: 시간 단위(temporal unit) 하나의 OBU들. low-overhead 형식(모든 OBU에
    ///   `obu_has_size_field = 1`), temporal delimiter OBU는 없다. 키프레임에는
    ///   sequence header OBU가 들어 있다.
    pub data: Vec<u8>,
    pub keyframe: bool,
    /// 원본 프레임의 캡처 시각(QPC 100ns, WGC `SystemRelativeTime`).
    pub capture_qpc_100ns: i64,
    /// `capture_qpc_100ns`를 [`instant_from_qpc`]로 바꾼 값.
    pub capture: Instant,
    pub width: u32,
    pub height: u32,
}

/// 비디오 파이프라인 통계. 속도 값은 최근 1초 기준이다.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoStats {
    pub encoder: String,
    pub hardware: bool,
    pub codec: Option<VideoCodec>,
    /// 출력(인코딩) 크기
    pub width: u32,
    pub height: u32,
    /// 캡처 원본 크기
    pub source_width: u32,
    pub source_height: u32,
    /// fps 제한을 통과한 캡처 프레임 수/초
    pub capture_fps: f32,
    pub encode_fps: f32,
    /// 인코더 호출 시간(평균, ms)
    pub avg_encode_ms: f32,
    pub bitrate_kbps: f32,
    /// 워커가 바빠서 버린 캡처 프레임 수(누적)
    pub dropped_frames: u64,
    /// (추가) BGRA→YUV 변환 시간(평균, ms). 테스트 패턴은 그리기 시간.
    pub avg_convert_ms: f32,
    /// (추가) 캡처 시각(`capture_qpc_100ns`) → 인코딩 완료(on_frame 직전)(평균, ms).
    /// WGC 시각은 DWM 합성 기준이라 프레임이 실제로 도착한 시각보다 조금 앞설 수 있어서
    /// 이 값은 처리 지연보다 작게(때로 음수로) 나온다.
    pub avg_latency_ms: f32,
    /// (추가) 프레임 도착(WGC 콜백·패턴 틱) → 인코딩 완료(평균, ms). 순수 처리 지연.
    pub avg_pipeline_ms: f32,
    /// (추가) 변환이 GPU(D3D11 Video Processor) 경로인지
    pub gpu_convert: bool,
    /// (추가) 인코딩한 프레임 수(누적)
    pub frames: u64,
    /// (추가) 그중 키프레임 수(누적)
    pub keyframes: u64,
}

/// 프레임 수 제한기. 평균 fps가 설정값을 넘지 않게 프레임을 고른다.
///
/// 다음 허용 시각(`next_due`)을 간격만큼씩 늘려 가며, 그보다 간격의 1/4 이상 이른
/// 프레임은 버린다. 화면이 멈췄다가 다시 움직이면 곧바로 받는다.
#[derive(Debug, Clone)]
pub(crate) struct Pacer {
    interval: i64,
    next_due: Option<i64>,
}

impl Pacer {
    pub fn new(fps: u32) -> Self {
        Pacer {
            interval: QPC_PER_SEC / fps.max(1) as i64,
            next_due: None,
        }
    }

    pub fn set_fps(&mut self, fps: u32) {
        self.interval = QPC_PER_SEC / fps.max(1) as i64;
        self.next_due = None;
    }

    pub fn accept(&mut self, ts: i64) -> bool {
        let tol = self.interval / 4;
        match self.next_due {
            Some(due) if ts + tol < due => false,
            Some(due) => {
                self.next_due = Some((due + self.interval).max(ts + self.interval - tol));
                true
            }
            None => {
                self.next_due = Some(ts + self.interval);
                true
            }
        }
    }
}

struct Mail {
    /// (캡처 번호, 프레임). 바꾸거나 닫은 캡처의 프레임은 워커가 쓰지 않고 버린다(그 프레임 풀은 이미 닫혔다).
    frame: Option<(u64, CapturedFrame)>,
    pacer: Pacer,
    /// 닫힌 캡처의 번호(`Worker::generation`). 지난 캡처의 늦은 알림은 무시한다.
    closed: Option<u64>,
    stop: bool,
    keyframe: bool,
    update: Option<(VideoSettings, SyncSender<Result<(), MediaError>>)>,
    switch: Option<(SourceId, SyncSender<Result<(), MediaError>>)>,
    /// 대상이 닫혀도 방송을 끝내지 않고 새 대상을 기다리는 최대 시간(게임 창 따라가기). `None`이면 바로 끝낸다.
    hold: Option<Duration>,
    /// 새 비트레이트 상한(`set_bitrate_cap`). 워커가 가져가면 `None`.
    cap: Option<Option<u32>>,
    /// simulcast 계층이 키프레임을 요청했다(워커를 깨워 화면이 멈춰 있으면 마지막 화면을 다시 넘기게 한다).
    layer_keyframe: bool,
}

struct StatsState {
    stats: VideoStats,
    captured: VecDeque<Instant>,
    encoded: VecDeque<(Instant, usize)>,
}

impl StatsState {
    fn prune(&mut self, now: Instant) {
        let window = Duration::from_secs(1);
        while self.captured.front().is_some_and(|t| now - *t > window) {
            self.captured.pop_front();
        }
        while self.encoded.front().is_some_and(|(t, _)| now - *t > window) {
            self.encoded.pop_front();
        }
    }
}

struct Shared {
    mail: Mutex<Mail>,
    cond: Condvar,
    settings: Mutex<VideoSettings>,
    /// 지금 쓰는 fps. 주사율 원본(`settings.fps` 0)이면 대상 모니터의 주사율로 풀어 쓴 값이다.
    fps: AtomicU32,
    running: AtomicBool,
    /// 지금 인코더에 건 비트레이트(kbps).
    encoder_kbps: AtomicU32,
    /// 대상이 닫혀 새 대상을 기다리는 중(`hold`).
    waiting: AtomicBool,
    error: Mutex<Option<String>>,
    stats: Mutex<StatsState>,
    started: Instant,
    /// 송출 화면 미리 보기 칸(화면이 요청할 때만 워커가 채운다).
    preview: PreviewSlot,
    /// simulcast 하위 계층(중·하, [`crate::simulcast`]). simulcast가 아니면 `None`.
    layers: Option<Layers>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn set_error(&self, e: &MediaError) {
        tracing::error!("비디오 파이프라인 오류: {e}");
        *lock(&self.error) = Some(e.to_string());
    }

    fn stats(&self) -> VideoStats {
        let now = Instant::now();
        let mut st = lock(&self.stats);
        st.prune(now);
        let span = (now - self.started).as_secs_f32().clamp(0.001, 1.0);
        let mut s = st.stats.clone();
        s.capture_fps = st.captured.len() as f32 / span;
        s.encode_fps = st.encoded.len() as f32 / span;
        s.bitrate_kbps =
            st.encoded.iter().map(|(_, b)| *b).sum::<usize>() as f32 * 8.0 / 1000.0 / span;
        s
    }
}

/// 인코더에 걸 비트레이트: 설정 비트레이트와 네트워크 상한 중 작은 쪽.
fn capped_bitrate(settings: &VideoSettings, cap: Option<u32>) -> u32 {
    cap.map_or(settings.bitrate_kbps, |c| c.min(settings.bitrate_kbps))
}

fn ema(prev: f32, v: f32) -> f32 {
    if prev == 0.0 { v } else { prev * 0.9 + v * 0.1 }
}

/// 캡처하고 인코딩하는 비디오 파이프라인.
pub struct VideoPipeline {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl VideoPipeline {
    /// `source` 캡처와 `settings` 인코딩을 시작한다.
    ///
    /// `on_frame`은 내부 스레드에서 인코딩된 프레임마다 불린다. 오래 막으면 안 된다
    /// (채널로 넘기는 정도). 캡처 대상이나 인코더를 열 수 없으면 오류를 돌려준다.
    /// 처음 부르면 [`available_encoders`](crate::available_encoders) 탐색(최대 몇 초)을 기다린다.
    ///
    /// `settings.fps`가 0(주사율 원본)이면 대상 모니터의 주사율(창이면 창이 있는 모니터, 테스트 패턴은 60,
    /// 최대 240)로 풀어 fps 제한과 인코더에 쓴다. 풀어 쓴 값은 [`fps`](Self::fps)로 본다.
    pub fn start(
        source: SourceId,
        settings: VideoSettings,
        on_frame: Box<dyn FnMut(EncodedVideo) + Send>,
    ) -> Result<Self, MediaError> {
        Self::start_inner(source, settings, on_frame, None)
    }

    /// [`start`](Self::start)와 같고, simulcast 하위 계층(중·하, D-098)도 인코딩한다. 상(주 인코더) 프레임은
    /// `on_frame`으로, 중·하 프레임은 `layer_sink`(계층 스레드에서 불림)로 나간다. 계층 비트레이트는 처음에 설정
    /// 비트레이트의 1/2·1/4이고 [`set_layer_rates`](Self::set_layer_rates)로 바꾼다. 주 인코더가 소프트웨어면 계층은 쉰다.
    pub fn start_simulcast(
        source: SourceId,
        settings: VideoSettings,
        on_frame: Box<dyn FnMut(EncodedVideo) + Send>,
        layer_sink: LayerSink,
    ) -> Result<Self, MediaError> {
        Self::start_inner(source, settings, on_frame, Some(layer_sink))
    }

    fn start_inner(
        source: SourceId,
        settings: VideoSettings,
        on_frame: Box<dyn FnMut(EncodedVideo) + Send>,
        layer_sink: Option<LayerSink>,
    ) -> Result<Self, MediaError> {
        let settings = settings.clamped();
        if candidates_for(settings.codec).is_empty() {
            return Err(MediaError::NoEncoder(settings.codec));
        }
        let fps = settings.resolve_fps(refresh_rate(&source));
        // 워커가 끝날 때 RoUninitialize가 MTA를 내리지 않게(다음 방송의 WGC 팩터리가 죽은 MTA를 쓰지 않게).
        crate::com::keep_mta();
        let shared = Arc::new(Shared {
            mail: Mutex::new(Mail {
                frame: None,
                pacer: Pacer::new(fps),
                closed: None,
                stop: false,
                keyframe: false,
                update: None,
                switch: None,
                hold: None,
                cap: None,
                layer_keyframe: false,
            }),
            cond: Condvar::new(),
            settings: Mutex::new(settings),
            fps: AtomicU32::new(fps),
            running: AtomicBool::new(true),
            encoder_kbps: AtomicU32::new(0),
            waiting: AtomicBool::new(false),
            error: Mutex::new(None),
            stats: Mutex::new(StatsState {
                stats: VideoStats::default(),
                captured: VecDeque::new(),
                encoded: VecDeque::new(),
            }),
            started: Instant::now(),
            preview: PreviewSlot::default(),
            layers: layer_sink.map(|sink| Layers::start(sink, settings.bitrate_kbps)),
        });
        let (init_tx, init_rx) = mpsc::sync_channel(1);
        let worker_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("dongsi-video".into())
            .spawn(move || {
                let _boost = boost_current_thread(ThreadRole::Video);
                // SAFETY: 이 스레드에서 WinRT(MTA)를 쓰고 끝날 때 해제한다.
                let com = unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_ok();
                match Worker::new(source, settings, fps, worker_shared.clone(), on_frame) {
                    Ok(mut w) => {
                        let _ = init_tx.send(Ok(()));
                        w.run();
                    }
                    Err(e) => {
                        worker_shared.set_error(&e);
                        let _ = init_tx.send(Err(e));
                    }
                }
                worker_shared.running.store(false, Ordering::SeqCst);
                if com {
                    // SAFETY: 위에서 성공한 RoInitialize와 짝을 맞춘다.
                    unsafe { RoUninitialize() };
                }
            })
            .map_err(|e| MediaError::Thread(e.to_string()))?;
        match init_rx.recv_timeout(Duration::from_secs(15)) {
            Ok(Ok(())) => Ok(VideoPipeline {
                shared,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let p = VideoPipeline {
                    shared,
                    thread: Some(thread),
                };
                p.shutdown();
                Err(MediaError::Thread(
                    "비디오 워커가 시작되지 않았습니다".into(),
                ))
            }
        }
    }

    /// 다음 프레임을 키프레임(IDR)으로 만든다. 화면이 멈춰 새 프레임이 없으면
    /// 20ms 뒤 마지막 화면을 다시 인코딩해 키프레임을 낸다(이때 캡처 시각은 지금).
    pub fn request_keyframe(&self) {
        let mut m = lock(&self.shared.mail);
        m.keyframe = true;
        self.shared.cond.notify_all();
    }

    /// 캡처를 멈추지 않고 코덱·해상도·fps·비트레이트를 바꾼다.
    ///
    /// 워커가 새 설정을 적용(필요하면 인코더를 다시 만듦)할 때까지 기다리고, 그 결과를
    /// 돌려준다. 실패하면 이전 설정이 유지된다. 바뀐 뒤 첫 프레임은 키프레임이다.
    /// 비트레이트만 바뀌고 인코더가 지원하면(nvenc) 인코더를 그대로 두고 바꾼다
    /// (이때는 키프레임을 강제하지 않는다). fps 0(주사율 원본)은 지금 대상의 모니터 주사율로 푼다.
    /// 풀어 쓴 값이 그대로면(예: 60Hz 모니터에서 60 → 원본) 인코더도 그대로다.
    pub fn update(&self, settings: VideoSettings) -> Result<(), MediaError> {
        let settings = settings.clamped();
        if !self.is_running() {
            return Err(MediaError::Thread(
                "비디오 파이프라인이 멈춰 있습니다".into(),
            ));
        }
        if candidates_for(settings.codec).is_empty() {
            return Err(MediaError::NoEncoder(settings.codec));
        }
        let (tx, rx) = mpsc::sync_channel(1);
        {
            let mut m = lock(&self.shared.mail);
            m.update = Some((settings, tx));
            self.shared.cond.notify_all();
        }
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(r) => r,
            Err(_) => Err(MediaError::Thread("비디오 워커가 응답하지 않습니다".into())),
        }
    }

    /// 네트워크에 맞춘 비트레이트 상한(D-081). 인코더 비트레이트는 `min(설정 비트레이트, 상한)`이다. `None`이면 상한이
    /// 없다. 기다리지 않는다(워커가 다음에 깨어날 때 적용한다). 값이 실제로 바뀌면 nvenc는 인코더를 그대로 두고
    /// 비트레이트만 바꾸고(키프레임이 나간다), 다른 인코더는 다시 연다. 다시 열지 못하면 이전 인코더·상한이 그대로다.
    ///
    /// 설정 비트레이트와 상한을 같이 바꿀 때는 이것을 먼저 부르고 [`update`](Self::update)를 부른다. 그러면 인코더를
    /// 한 번만 바꾼다.
    pub fn set_bitrate_cap(&self, kbps: Option<u32>) {
        let mut m = lock(&self.shared.mail);
        m.cap = Some(kbps);
        self.shared.cond.notify_all();
    }

    /// 지금 인코더에 건 비트레이트(kbps, `min(설정, 상한)`). 인코더가 아직 없으면 0.
    pub fn encoder_bitrate_kbps(&self) -> u32 {
        self.shared.encoder_kbps.load(Ordering::SeqCst)
    }

    /// 방송을 멈추지 않고 캡처 대상을 바꾼다(화면·창끼리, 게임 창 따라가기).
    ///
    /// 워커가 새 대상을 열 때까지 기다리고 그 결과를 돌려준다. 열지 못하면 지금 대상이 그대로다.
    /// 바뀐 뒤 첫 프레임은 키프레임이고, 크기가 다르면 인코더를 새 크기로 다시 연다. 주사율 원본이면
    /// 새 대상의 모니터 주사율로 다시 풀고, 값이 바뀌면 인코더를 그 fps로 다시 연다.
    /// 테스트 패턴 방송에서는 쓸 수 없다.
    pub fn switch_source(&self, source: SourceId) -> Result<(), MediaError> {
        if !self.is_running() {
            return Err(MediaError::Thread(
                "비디오 파이프라인이 멈춰 있습니다".into(),
            ));
        }
        let (tx, rx) = mpsc::sync_channel(1);
        {
            let mut m = lock(&self.shared.mail);
            m.switch = Some((source, tx));
            self.shared.cond.notify_all();
        }
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(r) => r,
            Err(_) => Err(MediaError::Thread("비디오 워커가 응답하지 않습니다".into())),
        }
    }

    /// 캡처 대상이 닫혀도 `max`까지는 방송을 끝내지 않고 [`switch_source`](Self::switch_source)를 기다린다.
    /// 그동안 새 프레임은 없고, 키프레임 요청에는 마지막 화면을 다시 인코딩한다. `None`(기본)이면
    /// 닫히는 즉시 [`MediaError::SourceClosed`]로 끝난다.
    pub fn set_hold_on_close(&self, max: Option<Duration>) {
        let mut m = lock(&self.shared.mail);
        m.hold = max;
        self.shared.cond.notify_all();
    }

    /// 대상이 닫혀 새 대상을 기다리는 중인지([`set_hold_on_close`](Self::set_hold_on_close)).
    pub fn is_waiting_source(&self) -> bool {
        self.shared.waiting.load(Ordering::SeqCst)
    }

    /// 지금 적용된 설정(요청한 값 그대로라 주사율 원본이면 `fps`가 0이다).
    pub fn settings(&self) -> VideoSettings {
        *lock(&self.shared.settings)
    }

    /// 지금 쓰는 fps(프레임 수 제한·인코더 기준). 주사율 원본이면 대상 모니터의 주사율로 풀어 쓴 값이다.
    pub fn fps(&self) -> u32 {
        self.shared.fps.load(Ordering::SeqCst)
    }

    pub fn stats(&self) -> VideoStats {
        self.shared.stats()
    }

    /// simulcast 계층의 다음 프레임을 키프레임으로 만든다(D-098). 상은 [`request_keyframe`](Self::request_keyframe)과
    /// 같다. simulcast가 아니면 중·하 요청은 무시한다. 화면이 멈춰 있으면 20ms 뒤 마지막 화면을 다시 넘긴다.
    pub fn request_layer_keyframe(&self, layer: Layer) {
        if layer == Layer::High {
            self.request_keyframe();
            return;
        }
        let Some(layers) = &self.shared.layers else {
            return;
        };
        layers.request_keyframe(layer);
        let mut m = lock(&self.shared.mail);
        m.layer_keyframe = true;
        self.shared.cond.notify_all();
    }

    /// simulcast 하위 계층(중·하)의 비트레이트(kbps, 상·중·하 순서, D-098). 0인 계층은 인코딩하지 않는다. 상은 쓰지
    /// 않는다: 주 인코더는 [`set_bitrate_cap`](Self::set_bitrate_cap)으로 맞춘다. simulcast가 아니면 아무 일도 없다.
    pub fn set_layer_rates(&self, rates: LayerRates) {
        if let Some(layers) = &self.shared.layers {
            layers.set_rates(rates);
        }
    }

    /// simulcast 하위 계층을 돌리는지([`start_simulcast`](Self::start_simulcast)로 시작했는지).
    pub fn is_simulcast(&self) -> bool {
        self.shared.layers.is_some()
    }

    /// simulcast 하위 계층(중·하) 통계. simulcast가 아니면 빈 목록.
    pub fn layer_stats(&self) -> Vec<LayerStats> {
        self.shared
            .layers
            .as_ref()
            .map_or_else(Vec::new, Layers::stats)
    }

    /// 송출 화면 미리 보기: 인코더에 넣는 프레임을 줄인 RGBA(소리 없음).
    ///
    /// 마지막 그림의 번호가 `after`(화면이 마지막으로 받은 번호)와 다르면 그 그림을 돌려준다. 부를 때마다
    /// 앞으로 1초 동안 [`PREVIEW_INTERVAL`](crate::PREVIEW_INTERVAL)마다 한 장씩 만들게 하므로, 화면이
    /// 부르지 않으면 비용이 없다. 그림은 `max_width`×`max_height`(기기 픽셀) 안에 비율을 지켜 맞추고
    /// 키우지 않는다. 화면이 멈춰 새 프레임이 없으면 마지막 그림이 그대로 남는다.
    pub fn preview(
        &self,
        after: u64,
        max_width: u32,
        max_height: u32,
    ) -> Option<Arc<PreviewFrame>> {
        self.shared
            .preview
            .request(Instant::now(), after, max_width, max_height)
    }

    /// 캡처·인코딩이 돌고 있는지. 대상 창이 닫히거나 오류가 나면 `false`.
    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::SeqCst)
    }

    /// 마지막 오류(예: 캡처 대상이 닫힘).
    pub fn last_error(&self) -> Option<String> {
        lock(&self.shared.error).clone()
    }

    /// 멈추고 내부 스레드가 끝날 때까지 기다린다.
    pub fn stop(self) {
        drop(self);
    }

    fn shutdown(&self) {
        let mut m = lock(&self.shared.mail);
        m.stop = true;
        self.shared.cond.notify_all();
    }
}

impl Drop for VideoPipeline {
    fn drop(&mut self) {
        self.shutdown();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// 파이프라인마다 하나뿐이라 크기 차이는 문제가 아니다.
#[allow(clippy::large_enum_variant)]
enum Source {
    Wgc {
        /// 대상이 닫혀 기다리는 중이면 `None`(`hold`).
        capture: Option<WgcCapture>,
        converter: Converter,
        d3d: D3d,
        /// 대상이 닫힌 시각(기다리는 중).
        lost_at: Option<Instant>,
    },
    Pattern {
        next_tick: i64,
        prev: Option<i64>,
        index: u64,
        white: bool,
        size: (u32, u32),
    },
}

/// 키프레임 요청 뒤 새 프레임을 기다리는 시간. 넘으면 마지막 화면을 다시 인코딩한다.
const KEYFRAME_WAIT: Duration = Duration::from_millis(20);

struct Worker {
    shared: Arc<Shared>,
    source: Source,
    /// 요청한 설정(주사율 원본이면 `fps` 0).
    settings: VideoSettings,
    /// 지금 쓰는 fps(`settings.fps`를 풀어 쓴 값).
    fps: u32,
    /// 네트워크에 맞춘 비트레이트 상한(`VideoPipeline::set_bitrate_cap`).
    cap: Option<u32>,
    encoder: Option<VideoEncoder>,
    on_frame: Box<dyn FnMut(EncodedVideo) + Send>,
    force_key: bool,
    key_requested_at: Option<Instant>,
    packets: Vec<EncodedPacket>,
    source_id: SourceId,
    /// 지금 WGC 캡처의 번호. 대상을 바꿀 때마다 1씩 는다.
    generation: u64,
    alive_checked: Instant,
    preview: PreviewScaler,
    /// 아주 작은 프레임([`MIN_SOURCE_SIDE`](crate::convert::MIN_SOURCE_SIDE))을 건너뛰는 중(기록을 한 번만 남기려고).
    tiny_frames: bool,
}

impl Worker {
    fn new(
        source: SourceId,
        settings: VideoSettings,
        fps: u32,
        shared: Arc<Shared>,
        on_frame: Box<dyn FnMut(EncodedVideo) + Send>,
    ) -> Result<Self, MediaError> {
        let src = match &source {
            SourceId::TestPattern => Source::Pattern {
                next_tick: pattern_next_tick(qpc_now_100ns(), fps),
                prev: None,
                index: 0,
                white: false,
                size: pattern_source_size(&settings),
            },
            _ => {
                let d3d = create_d3d()?;
                let converter = Converter::new(d3d.clone());
                let capture = start_wgc(&d3d, &source, &shared, 0)?;
                Source::Wgc {
                    capture: Some(capture),
                    converter,
                    d3d,
                    lost_at: None,
                }
            }
        };
        let mut w = Worker {
            shared,
            source: src,
            settings,
            fps,
            cap: None,
            encoder: None,
            on_frame,
            force_key: true,
            key_requested_at: None,
            packets: Vec::new(),
            source_id: source.clone(),
            generation: 0,
            alive_checked: Instant::now(),
            preview: PreviewScaler::new(),
            tiny_frames: false,
        };
        // 인코더를 미리 열어 본다(코덱·해상도 문제를 start()에서 알리기 위해).
        // 최소화된 창은 아주 작은 크기를 알려 주므로 하한을 둔다. 첫 프레임 크기가 다르면
        // 그때 다시 연다.
        let (sw, sh) = w.source_size();
        let (ow, oh) = settings.output_size(sw.max(320), sh.max(180));
        if let Some(sink) = crate::trace::video() {
            sink.mark(
                "pipeline",
                &format!(
                    "source={} codec={} size={}x{} fps={} (요청 {}) kbps={}",
                    w.source_id,
                    settings.codec,
                    settings.width,
                    settings.height,
                    fps,
                    settings.fps,
                    settings.bitrate_kbps
                ),
            );
        }
        w.set_encoder(open_best(settings.codec, &w.params(ow, oh))?);
        w.publish_encoder_stats(sw, sh);
        // 인코더가 준비된 뒤에 프레임을 받기 시작한다(그 전 프레임이 버려진 것으로 세지 않게).
        if let Source::Wgc {
            capture: Some(capture),
            ..
        } = &w.source
        {
            capture.begin()?;
        }
        tracing::info!(%source, ?settings, fps, "비디오 파이프라인 시작");
        Ok(w)
    }

    fn params(&self, w: u32, h: u32) -> EncoderParams {
        EncoderParams {
            width: w,
            height: h,
            fps: self.fps,
            bitrate_kbps: capped_bitrate(&self.settings, self.cap),
        }
    }

    fn set_encoder(&mut self, enc: VideoEncoder) {
        self.shared
            .encoder_kbps
            .store(enc.params.bitrate_kbps, Ordering::SeqCst);
        self.encoder = Some(enc);
    }

    /// 비트레이트 상한을 바꾼다(`VideoPipeline::set_bitrate_cap`). 실패하면 이전 인코더와 상한이 그대로다.
    fn apply_cap(&mut self, cap: Option<u32>) -> Result<(), MediaError> {
        if cap == self.cap {
            return Ok(());
        }
        let kbps = capped_bitrate(&self.settings, cap);
        let Some(enc) = &mut self.encoder else {
            self.cap = cap; // 다음에 여는 인코더가 쓴다.
            return Ok(());
        };
        if enc.params.bitrate_kbps == kbps {
            self.cap = cap;
            return Ok(());
        }
        let from = enc.params.bitrate_kbps;
        if enc.set_bitrate(kbps) {
            self.cap = cap;
            self.shared.encoder_kbps.store(kbps, Ordering::SeqCst);
            tracing::info!(
                from,
                to = kbps,
                "네트워크에 맞춰 비트레이트 변경(인코더 유지)"
            );
            return Ok(());
        }
        let params = EncoderParams {
            bitrate_kbps: kbps,
            ..enc.params
        };
        let new = open_best(self.settings.codec, &params)?;
        tracing::info!(from, to = kbps, encoder = %new.info.name, "네트워크에 맞춰 비트레이트 변경(인코더 재생성)");
        self.set_encoder(new);
        self.cap = cap;
        let (sw, sh) = self.source_size();
        self.publish_encoder_stats(sw, sh);
        self.force_key = true;
        // 화면이 멈춰 있어도 새 인코더의 키프레임이 바로 나가게 마지막 화면을 다시 인코딩한다.
        if let Source::Wgc { converter, .. } = &self.source
            && converter.source_size().is_some()
        {
            let now = qpc_now_100ns();
            self.encode_current(now, now)?;
        }
        Ok(())
    }

    /// 지금 쓰는 fps를 바꾼다(fps 제한기 포함).
    fn set_fps(&mut self, fps: u32) {
        if fps == self.fps {
            return;
        }
        self.fps = fps;
        self.shared.fps.store(fps, Ordering::SeqCst);
        lock(&self.shared.mail).pacer.set_fps(fps);
    }

    fn source_size(&self) -> (u32, u32) {
        match &self.source {
            Source::Wgc {
                capture, converter, ..
            } => converter
                .source_size()
                .unwrap_or_else(|| capture.as_ref().map_or((0, 0), WgcCapture::item_size)),
            Source::Pattern { size, .. } => *size,
        }
    }

    fn publish_encoder_stats(&self, sw: u32, sh: u32) {
        let mut st = lock(&self.shared.stats);
        if let Some(e) = &self.encoder {
            st.stats.encoder = e.info.name.clone();
            st.stats.hardware = e.info.hardware;
            st.stats.codec = Some(e.info.codec);
            st.stats.width = e.params.width;
            st.stats.height = e.params.height;
        }
        st.stats.source_width = sw;
        st.stats.source_height = sh;
        if let Source::Wgc { converter, .. } = &self.source {
            st.stats.gpu_convert = converter.is_gpu();
        }
    }

    fn run(&mut self) {
        loop {
            let timeout = self.wait_timeout();
            // 키프레임 대기처럼 짧은 시한은 조건 변수 시간 제한(시스템 타이머 단위, 약 15ms)이
            // 부정확하므로 고해상도 sleep으로 1ms씩 확인한다.
            let precise = timeout < Duration::from_millis(50);
            let (frame, closed, stop, keyreq, layer_keyreq, update, switch, hold, cap) = {
                let deadline = Instant::now() + timeout;
                let mut m = lock(&self.shared.mail);
                loop {
                    if m.stop
                        || m.closed.is_some()
                        || m.frame.is_some()
                        || m.keyframe
                        || m.layer_keyframe
                        || m.update.is_some()
                        || m.switch.is_some()
                        || m.cap.is_some()
                    {
                        break;
                    }
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    if precise {
                        drop(m);
                        std::thread::sleep(left.min(Duration::from_millis(1)));
                        m = lock(&self.shared.mail);
                    } else {
                        m = self
                            .shared
                            .cond
                            .wait_timeout(m, left)
                            .unwrap_or_else(|e| e.into_inner())
                            .0;
                    }
                }
                (
                    m.frame.take(),
                    m.closed.take(),
                    m.stop,
                    std::mem::take(&mut m.keyframe),
                    std::mem::take(&mut m.layer_keyframe),
                    m.update.take(),
                    m.switch.take(),
                    m.hold,
                    m.cap.take(),
                )
            };
            if stop {
                break;
            }
            if let Some((id, reply)) = switch {
                let r = self.switch_source(id);
                let _ = reply.send(r);
            }
            let lost = closed.is_some_and(|g| g == self.generation) || !self.check_alive();
            if (lost && !self.hold_source(hold)) || self.hold_expired(hold) {
                self.shared.set_error(&MediaError::SourceClosed);
                break;
            }
            // 바꾸거나 닫은 캡처의 프레임은 버린다(프레임 풀이 닫혀 텍스처를 쓸 수 없다).
            let frame = frame.and_then(|(g, f)| (g == self.generation).then_some(f));
            // 상한을 설정보다 먼저 적용한다(둘을 같이 바꾸면 인코더를 한 번만 바꾼다).
            if let Some(cap) = cap
                && let Err(e) = self.apply_cap(cap)
            {
                tracing::warn!("비트레이트 상한을 적용하지 못했습니다(이전 인코더 유지): {e}");
            }
            if let Some((s, reply)) = update {
                let r = self.apply_settings(s);
                let _ = reply.send(r);
            }
            if keyreq {
                self.force_key = true;
                self.key_requested_at = Some(Instant::now());
            }
            if layer_keyreq {
                self.key_requested_at.get_or_insert_with(Instant::now);
            }
            let res = match &self.source {
                Source::Wgc { converter, .. } => {
                    let kept = converter.source_size().is_some();
                    match frame {
                        Some(f) => self.on_wgc_frame(f),
                        None => {
                            let due = self
                                .key_requested_at
                                .is_some_and(|t| t.elapsed() >= KEYFRAME_WAIT);
                            if due && self.wants_keyframe() {
                                self.key_requested_at = None;
                                // 보관한 화면이 없으면(첫 프레임부터 아주 작음) 다음 프레임이 키프레임으로 나간다.
                                if kept {
                                    let now = qpc_now_100ns();
                                    self.encode_current(now, now)
                                } else {
                                    Ok(())
                                }
                            } else {
                                Ok(())
                            }
                        }
                    }
                }
                Source::Pattern { .. } => self.on_pattern_tick(),
            };
            if let Err(e) = res {
                self.shared.set_error(&e);
                if !self.recover() {
                    break;
                }
            }
        }
        if let Source::Wgc {
            capture: Some(capture),
            ..
        } = &mut self.source
        {
            capture.stop();
        }
        crate::trace::flush();
        tracing::info!("비디오 파이프라인 종료");
    }

    /// 0.5초마다 캡처 대상이 아직 있는지 확인한다(`Closed` 이벤트가 안 오는 경우 대비).
    fn check_alive(&mut self) -> bool {
        if matches!(self.source, Source::Wgc { capture: None, .. }) {
            return true; // 이미 닫혀 기다리는 중
        }
        if self.alive_checked.elapsed() < Duration::from_millis(500) {
            return true;
        }
        self.alive_checked = Instant::now();
        crate::source::source_alive(&self.source_id)
    }

    /// 대상이 닫혔을 때: 기다리기로 했으면(`hold`) 캡처만 닫고 `true`. 보관한 마지막 화면은 남겨 키프레임
    /// 요청에 다시 쓴다. 기다리지 않으면 `false`.
    fn hold_source(&mut self, hold: Option<Duration>) -> bool {
        let (
            Some(_),
            Source::Wgc {
                capture, lost_at, ..
            },
        ) = (hold, &mut self.source)
        else {
            return false;
        };
        if let Some(mut c) = capture.take() {
            c.stop();
            // 닫은 캡처에서 이미 받아 둔 프레임을 쓰지 않게 번호를 넘긴다.
            self.generation += 1;
            tracing::info!(source = %self.source_id, "캡처 대상이 닫혔습니다. 새 대상을 기다립니다");
        }
        lost_at.get_or_insert_with(Instant::now);
        self.shared.waiting.store(true, Ordering::SeqCst);
        true
    }

    /// 기다리는 시간이 지났는지(또는 기다리는 중에 `hold`가 꺼졌는지).
    fn hold_expired(&self, hold: Option<Duration>) -> bool {
        match (&self.source, hold) {
            (
                Source::Wgc {
                    lost_at: Some(t), ..
                },
                Some(max),
            ) => t.elapsed() > max,
            (
                Source::Wgc {
                    lost_at: Some(_), ..
                },
                None,
            ) => true,
            _ => false,
        }
    }

    /// 캡처 대상을 바꾼다(WGC끼리). 새 대상을 열고 시작한 뒤에 옛 캡처를 닫으므로, 열지 못하면 그대로다.
    fn switch_source(&mut self, id: SourceId) -> Result<(), MediaError> {
        let generation = self.generation + 1;
        let Source::Wgc {
            capture,
            d3d,
            lost_at,
            ..
        } = &mut self.source
        else {
            return Err(MediaError::Capture(
                "테스트 패턴 방송은 캡처 대상을 바꿀 수 없습니다".into(),
            ));
        };
        let new = start_wgc(d3d, &id, &self.shared, generation)?;
        new.begin()?;
        if let Some(mut old) = capture.take() {
            old.stop();
        }
        *capture = Some(new);
        *lost_at = None;
        self.shared.waiting.store(false, Ordering::SeqCst);
        self.generation = generation;
        tracing::info!(from = %self.source_id, to = %id, "캡처 대상 변경");
        self.source_id = id;
        self.alive_checked = Instant::now();
        self.force_key = true;
        // 주사율 원본이면 새 대상의 모니터 주사율로 다시 푼다. 바뀌면 다음 프레임에서 인코더를 그 fps로 다시 연다.
        let fps = self.settings.resolve_fps(refresh_rate(&self.source_id));
        if fps != self.fps {
            tracing::info!(from = self.fps, to = fps, "캡처 대상 변경으로 fps 변경");
            self.set_fps(fps);
            self.encoder = None;
        }
        Ok(())
    }

    /// 인코더 오류 뒤 한 번 다시 열어 본다.
    fn recover(&mut self) -> bool {
        self.encoder = None;
        let (sw, sh) = self.source_size();
        let (ow, oh) = self.settings.output_size(sw, sh);
        match open_best(self.settings.codec, &self.params(ow, oh)) {
            Ok(e) => {
                self.set_encoder(e);
                self.force_key = true;
                self.publish_encoder_stats(sw, sh);
                true
            }
            Err(e) => {
                self.shared.set_error(&e);
                false
            }
        }
    }

    /// 주 인코더나 simulcast 계층이 키프레임을 기다리는지.
    fn wants_keyframe(&self) -> bool {
        self.force_key
            || self
                .shared
                .layers
                .as_ref()
                .is_some_and(Layers::wants_keyframe)
    }

    fn wait_timeout(&self) -> Duration {
        match &self.source {
            Source::Wgc { .. } => match self.key_requested_at {
                Some(t) if self.wants_keyframe() => KEYFRAME_WAIT.saturating_sub(t.elapsed()),
                _ => Duration::from_millis(500),
            },
            // 테스트 패턴은 on_pattern_tick 안에서 정밀하게 잔다.
            Source::Pattern { .. } => Duration::ZERO,
        }
    }

    fn apply_settings(&mut self, new: VideoSettings) -> Result<(), MediaError> {
        let old = self.settings;
        if new == old {
            return Ok(());
        }
        let fps = new.resolve_fps(refresh_rate(&self.source_id));
        let (sw, sh) = match &self.source {
            Source::Pattern { .. } => pattern_source_size(&new),
            _ => self.source_size(),
        };
        let (ow, oh) = new.output_size(sw, sh);
        let kbps = capped_bitrate(&new, self.cap);
        let params = EncoderParams {
            width: ow,
            height: oh,
            fps,
            bitrate_kbps: kbps,
        };
        let current = self
            .encoder
            .as_ref()
            .filter(|e| e.info.codec == new.codec)
            .map(|e| e.params);
        // 풀어 쓴 값이 그대로면(예: 60Hz 모니터에서 60 → 원본) 인코더를 그대로 둔다.
        if current == Some(params) {
            self.settings = new;
            *lock(&self.shared.settings) = new;
            return Ok(());
        }
        let only_bitrate = current.is_some_and(|p| {
            EncoderParams {
                bitrate_kbps: p.bitrate_kbps,
                ..params
            } == p
        });
        if only_bitrate
            && let Some(enc) = &mut self.encoder
            && enc.set_bitrate(kbps)
        {
            self.settings = new;
            *lock(&self.shared.settings) = new;
            self.shared.encoder_kbps.store(kbps, Ordering::SeqCst);
            tracing::info!(
                kbps,
                setting = new.bitrate_kbps,
                "비트레이트 변경(인코더 유지)"
            );
            return Ok(());
        }
        let enc = open_best(new.codec, &params)?;
        tracing::info!(?new, fps, kbps, encoder = %enc.info.name, "설정 변경, 인코더 재생성");
        self.set_encoder(enc);
        self.settings = new;
        *lock(&self.shared.settings) = new;
        self.set_fps(fps);
        if let Source::Pattern {
            size, next_tick, ..
        } = &mut self.source
        {
            *size = (sw, sh);
            *next_tick = pattern_next_tick(qpc_now_100ns(), fps);
        }
        self.publish_encoder_stats(sw, sh);
        self.force_key = true;
        // 화면이 멈춰 있어도 새 설정의 키프레임이 바로 나가게 마지막 화면을 다시 인코딩한다.
        if let Source::Wgc { converter, .. } = &self.source
            && converter.source_size().is_some()
        {
            let now = qpc_now_100ns();
            self.encode_current(now, now)?;
        }
        Ok(())
    }

    fn on_wgc_frame(&mut self, f: CapturedFrame) -> Result<(), MediaError> {
        let Source::Wgc {
            capture, converter, ..
        } = &mut self.source
        else {
            return Ok(());
        };
        let tex = f.texture()?;
        if let Some(c) = capture {
            c.follow_size(f.width, f.height);
        }
        let kept = converter.ingest(&tex, f.width, f.height)?;
        let (qpc, arrival, w, h) = (f.qpc, f.arrival_qpc, f.width, f.height);
        drop(tex);
        drop(f); // 프레임 풀에 바로 돌려준다.
        if !kept {
            // 창이 막 뜨거나 최소화될 때 오는 아주 작은 프레임: 인코더를 그 크기로 다시 열지 않고 마지막 화면을 둔다.
            if !self.tiny_frames {
                self.tiny_frames = true;
                tracing::info!(w, h, source = %self.source_id, "아주 작은 프레임은 건너뜁니다(마지막 화면 유지)");
            }
            return Ok(());
        }
        if std::mem::take(&mut self.tiny_frames) {
            tracing::info!(w, h, "프레임 크기가 돌아왔습니다");
        }
        self.encode_current(qpc, arrival)
    }

    fn on_pattern_tick(&mut self) -> Result<(), MediaError> {
        let fps = self.fps;
        let Source::Pattern {
            next_tick,
            prev,
            index,
            white,
            ..
        } = &mut self.source
        else {
            return Ok(());
        };
        // 다음 틱까지 정밀하게 잔다(최대 10ms씩, 그 사이 메일 확인).
        let now = qpc_now_100ns();
        if now < *next_tick {
            let left = Duration::from_nanos((*next_tick - now) as u64 * 100);
            std::thread::sleep(left.min(Duration::from_millis(10)));
            return Ok(());
        }
        // 늦었으면 지난 틱은 건너뛰고 지금 이전의 마지막 틱을 쓴다.
        let mut t = *next_tick;
        loop {
            let n = pattern_next_tick(t, fps);
            if n > now {
                break;
            }
            t = n;
        }
        *white = pattern_is_white(*prev, t);
        *prev = Some(t);
        *index += 1;
        *next_tick = pattern_next_tick(t, fps);
        {
            let mut st = lock(&self.shared.stats);
            st.captured.push_back(Instant::now());
        }
        self.encode_current(t, now)
    }

    /// 지금 원본(보관 프레임 또는 패턴)을 인코딩한다.
    fn encode_current(&mut self, qpc: i64, arrival_qpc: i64) -> Result<(), MediaError> {
        let (sw, sh) = self.source_size();
        let (ow, oh) = self.settings.output_size(sw, sh);
        let reopen = self
            .encoder
            .as_ref()
            .is_none_or(|e| e.params.width != ow || e.params.height != oh);
        if reopen {
            self.encoder = None;
            let enc = open_best(self.settings.codec, &self.params(ow, oh))?;
            tracing::info!(sw, sh, ow, oh, encoder = %enc.info.name, "원본 크기 변경, 인코더 재생성");
            self.set_encoder(enc);
            self.force_key = true;
            self.publish_encoder_stats(sw, sh);
        }
        let enc = self.encoder.as_mut().expect("위에서 열었다");
        // simulcast 계층은 주 인코더와 같은 코덱·인코더로 연다(`frame_view`가 인코더를 빌리기 전에 꺼내 둔다).
        let main = self
            .shared
            .layers
            .is_some()
            .then(|| (enc.info.codec, enc.info.name.clone(), enc.info.hardware));
        let t0 = Instant::now();
        let gpu;
        let convert_ms;
        {
            let mut view = enc.frame_view()?;
            match &mut self.source {
                Source::Wgc { converter, .. } => {
                    converter.convert(&mut view)?;
                    gpu = converter.last_gpu;
                }
                Source::Pattern { white, index, .. } => {
                    render_pattern(&mut view, qpc, *white, *index);
                    gpu = false;
                }
            }
            convert_ms = t0.elapsed().as_secs_f32() * 1000.0;
            // 화면이 미리 보기를 보고 있으면 간격마다 한 장(시청자가 받는 것과 같은 프레임).
            if let Some(max) = self.shared.preview.take_turn(Instant::now()) {
                match self.preview.scale(&view, max) {
                    Ok((w, h, rgba)) => self.shared.preview.put(w, h, rgba),
                    Err(e) => tracing::debug!("송출 화면 미리 보기 실패: {e}"),
                }
            }
            if let (Some(layers), Some((codec, name, hardware))) = (&self.shared.layers, &main) {
                layers.submit(&view, qpc, arrival_qpc, *codec, (name, *hardware), self.fps);
            }
        }
        let key = self.force_key;
        let t1 = Instant::now();
        let packets = &mut self.packets;
        enc.encode(qpc, arrival_qpc, key, &mut |p| packets.push(p))?;
        let encode_ms = t1.elapsed().as_secs_f32() * 1000.0;
        if key {
            self.force_key = false;
            self.key_requested_at = None;
        }
        let codec = enc.info.codec;
        let (w, h) = (enc.params.width, enc.params.height);
        for p in self.packets.drain(..) {
            let now_qpc = qpc_now_100ns();
            let latency = qpc_ms(now_qpc - p.qpc);
            let pipeline = qpc_ms(now_qpc - arrival_qpc);
            {
                let mut st = lock(&self.shared.stats);
                let now = Instant::now();
                st.encoded.push_back((now, p.data.len()));
                st.stats.frames += 1;
                if p.keyframe {
                    st.stats.keyframes += 1;
                }
                st.stats.avg_encode_ms = ema(st.stats.avg_encode_ms, encode_ms);
                st.stats.avg_convert_ms = ema(st.stats.avg_convert_ms, convert_ms);
                st.stats.avg_latency_ms = ema(st.stats.avg_latency_ms, latency);
                st.stats.avg_pipeline_ms = ema(st.stats.avg_pipeline_ms, pipeline);
                st.stats.gpu_convert = gpu;
                st.prune(now);
            }
            (self.on_frame)(EncodedVideo {
                layer: Layer::High,
                codec,
                data: p.data,
                keyframe: p.keyframe,
                capture_qpc_100ns: p.qpc,
                capture: instant_from_qpc(p.qpc),
                width: w,
                height: h,
            });
        }
        Ok(())
    }
}

/// WGC 캡처를 연다(아직 [`WgcCapture::begin`] 전). `generation`은 이 캡처의 번호로, 닫힘 알림이
/// 바꾸기 전 캡처의 것인지 가리는 데 쓴다.
fn start_wgc(
    d3d: &D3d,
    source: &SourceId,
    shared: &Arc<Shared>,
    generation: u64,
) -> Result<WgcCapture, MediaError> {
    let s_frame = shared.clone();
    let s_closed = shared.clone();
    WgcCapture::start(
        d3d,
        source,
        move |f| on_captured(&s_frame, generation, f),
        move || {
            let mut m = lock(&s_closed.mail);
            m.closed = Some(generation);
            s_closed.cond.notify_all();
        },
    )
}

/// WGC 콜백: fps 제한을 적용하고 최신 프레임 칸에 넣는다.
fn on_captured(shared: &Shared, generation: u64, f: CapturedFrame) {
    let dropped = {
        let mut m = lock(&shared.mail);
        if m.stop || !m.pacer.accept(f.qpc) {
            return;
        }
        let dropped = m.frame.replace((generation, f)).is_some();
        shared.cond.notify_all();
        dropped
    };
    let mut st = lock(&shared.stats);
    st.captured.push_back(Instant::now());
    if dropped {
        st.stats.dropped_frames += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(pacer: &mut Pacer, source_hz: f64, secs: f64) -> usize {
        let period = QPC_PER_SEC as f64 / source_hz;
        let n = (secs * source_hz) as usize;
        (0..n)
            .filter(|&i| pacer.accept((i as f64 * period) as i64 + 1_000_000_000))
            .count()
    }

    #[test]
    fn bitrate_cap_takes_the_smaller() {
        let s = VideoSettings {
            bitrate_kbps: 6000,
            ..VideoSettings::DEFAULT
        };
        assert_eq!(capped_bitrate(&s, None), 6000);
        assert_eq!(capped_bitrate(&s, Some(2500)), 2500);
        assert_eq!(capped_bitrate(&s, Some(9000)), 6000);
    }

    #[test]
    fn pacer_caps_rate() {
        // 144Hz 원본 → 60fps
        let got = run(&mut Pacer::new(60), 144.0, 10.0);
        assert!((595..=601).contains(&got), "{got}");
        // 60Hz 원본 → 60fps: 모두 통과
        assert_eq!(run(&mut Pacer::new(60), 60.0, 10.0), 600);
        // 60Hz 원본 → 30fps: 절반
        let got = run(&mut Pacer::new(30), 60.0, 10.0);
        assert!((299..=301).contains(&got), "{got}");
        // 원본이 더 느리면 모두 통과
        assert_eq!(run(&mut Pacer::new(60), 24.0, 10.0), 240);
    }

    #[test]
    fn pacer_accepts_after_idle_without_burst() {
        let mut p = Pacer::new(60);
        assert!(p.accept(0));
        // 5초 동안 정지 후 144Hz로 다시 움직임
        let base = 5 * QPC_PER_SEC;
        let period = QPC_PER_SEC / 144;
        let accepted: Vec<i64> = (0..144)
            .map(|i| base + i * period)
            .filter(|&t| p.accept(t))
            .collect();
        assert_eq!(accepted[0], base);
        // 정지 직후에도 최소 간격(간격의 절반) 이상
        assert!(
            accepted
                .windows(2)
                .all(|w| w[1] - w[0] >= QPC_PER_SEC / 120)
        );
        assert!((58..=61).contains(&accepted.len()), "{}", accepted.len());
    }
}
