//! 비디오 파이프라인: 캡처(WGC 또는 테스트 패턴) → YUV 변환 → 인코딩.
//!
//! 스레드 구성
//! - WGC 콜백(시스템 스레드): 프레임 수 제한(fps)을 적용하고, 통과한 프레임을
//!   "최신 프레임 칸" 하나에 넣는다. 워커가 아직 이전 프레임을 처리 중이면 칸의
//!   이전 프레임은 버린다(dropped). 그래서 밀린 프레임이 쌓이지 않는다.
//! - 워커 스레드: 칸에서 프레임을 꺼내 보관 텍스처로 복사하고(캡처 프레임은 바로 반납),
//!   변환·인코딩한 뒤 `on_frame`을 부른다. 테스트 패턴은 워커가 직접 틱마다 그린다.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dongsi_core::{VideoCodec, VideoSettings};
use serde::Serialize;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};

use crate::capture::{CapturedFrame, WgcCapture};
use crate::clock::{QPC_PER_SEC, instant_from_qpc, qpc_ms, qpc_now_100ns};
use crate::convert::Converter;
use crate::d3d::create_d3d;
use crate::encoder::{EncodedPacket, EncoderParams, VideoEncoder, candidates_for, open_best};
use crate::yuv::{pattern_is_white, pattern_next_tick, pattern_source_size, render_pattern};
use crate::{MediaError, SourceId};

/// 인코딩된 비디오 프레임 하나.
#[derive(Debug, Clone)]
pub struct EncodedVideo {
    pub codec: VideoCodec,
    /// - H.264/H.265: Annex B(4바이트 시작 코드). 키프레임마다 SPS/PPS(H.265는 VPS도)가
    ///   IDR 앞에 들어 있다. AUD·filler NAL은 없다.
    /// - VP8/VP9: 프레임 하나(IVF 프레임 페이로드와 같은 형식).
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
    /// (추가) 캡처 시각 → 인코딩 완료(on_frame 직전) 지연(평균, ms)
    pub avg_latency_ms: f32,
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
    frame: Option<CapturedFrame>,
    pacer: Pacer,
    closed: bool,
    stop: bool,
    keyframe: bool,
    update: Option<(VideoSettings, SyncSender<Result<(), MediaError>>)>,
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
    running: AtomicBool,
    error: Mutex<Option<String>>,
    stats: Mutex<StatsState>,
    started: Instant,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn set_error(&self, e: &MediaError) {
        tracing::error!("비디오 파이프라인 오류: {e}");
        *lock(&self.error) = Some(e.to_string());
    }
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
    pub fn start(
        source: SourceId,
        settings: VideoSettings,
        on_frame: Box<dyn FnMut(EncodedVideo) + Send>,
    ) -> Result<Self, MediaError> {
        let settings = settings.clamped();
        if candidates_for(settings.codec).is_empty() {
            return Err(MediaError::NoEncoder(settings.codec));
        }
        let shared = Arc::new(Shared {
            mail: Mutex::new(Mail {
                frame: None,
                pacer: Pacer::new(settings.fps),
                closed: false,
                stop: false,
                keyframe: false,
                update: None,
            }),
            cond: Condvar::new(),
            settings: Mutex::new(settings),
            running: AtomicBool::new(true),
            error: Mutex::new(None),
            stats: Mutex::new(StatsState {
                stats: VideoStats::default(),
                captured: VecDeque::new(),
                encoded: VecDeque::new(),
            }),
            started: Instant::now(),
        });
        let (init_tx, init_rx) = mpsc::sync_channel(1);
        let worker_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("dongsi-video".into())
            .spawn(move || {
                // SAFETY: 이 스레드에서 WinRT(MTA)를 쓰고 끝날 때 해제한다.
                let com = unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_ok();
                match Worker::new(source, settings, worker_shared.clone(), on_frame) {
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
    /// 약 30ms 뒤 마지막 화면을 다시 인코딩해 키프레임을 낸다.
    pub fn request_keyframe(&self) {
        let mut m = lock(&self.shared.mail);
        m.keyframe = true;
        self.shared.cond.notify_all();
    }

    /// 캡처를 멈추지 않고 코덱·해상도·fps·비트레이트를 바꾼다.
    ///
    /// 워커가 새 설정을 적용(필요하면 인코더를 다시 만듦)할 때까지 기다리고, 그 결과를
    /// 돌려준다. 실패하면 이전 설정이 유지된다. 바뀐 뒤 첫 프레임은 키프레임이다.
    /// 비트레이트만 바뀌고 인코더가 지원하면(nvenc, libx264) 인코더를 그대로 두고 바꾼다
    /// (이때는 키프레임을 강제하지 않는다).
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

    /// 지금 적용된 설정.
    pub fn settings(&self) -> VideoSettings {
        *lock(&self.shared.settings)
    }

    pub fn stats(&self) -> VideoStats {
        let now = Instant::now();
        let mut st = lock(&self.shared.stats);
        st.prune(now);
        let span = (now - self.shared.started).as_secs_f32().clamp(0.001, 1.0);
        let mut s = st.stats.clone();
        s.capture_fps = st.captured.len() as f32 / span;
        s.encode_fps = st.encoded.len() as f32 / span;
        s.bitrate_kbps =
            st.encoded.iter().map(|(_, b)| *b).sum::<usize>() as f32 * 8.0 / 1000.0 / span;
        s
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
        capture: WgcCapture,
        converter: Converter,
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
const KEYFRAME_WAIT: Duration = Duration::from_millis(30);

struct Worker {
    shared: Arc<Shared>,
    source: Source,
    settings: VideoSettings,
    encoder: Option<VideoEncoder>,
    on_frame: Box<dyn FnMut(EncodedVideo) + Send>,
    force_key: bool,
    key_requested_at: Option<Instant>,
    packets: Vec<EncodedPacket>,
}

impl Worker {
    fn new(
        source: SourceId,
        settings: VideoSettings,
        shared: Arc<Shared>,
        on_frame: Box<dyn FnMut(EncodedVideo) + Send>,
    ) -> Result<Self, MediaError> {
        let src = match &source {
            SourceId::TestPattern => Source::Pattern {
                next_tick: pattern_next_tick(qpc_now_100ns(), settings.fps),
                prev: None,
                index: 0,
                white: false,
                size: pattern_source_size(&settings),
            },
            _ => {
                let d3d = create_d3d()?;
                let converter = Converter::new(d3d.clone());
                let s_frame = shared.clone();
                let s_closed = shared.clone();
                let capture = WgcCapture::start(
                    &d3d,
                    &source,
                    move |f| on_captured(&s_frame, f),
                    move || {
                        let mut m = lock(&s_closed.mail);
                        m.closed = true;
                        s_closed.cond.notify_all();
                    },
                )?;
                Source::Wgc { capture, converter }
            }
        };
        let mut w = Worker {
            shared,
            source: src,
            settings,
            encoder: None,
            on_frame,
            force_key: true,
            key_requested_at: None,
            packets: Vec::new(),
        };
        // 인코더를 미리 열어 본다(코덱·해상도 문제를 start()에서 알리기 위해).
        let (sw, sh) = w.source_size();
        let (ow, oh) = settings.output_size(sw, sh);
        w.encoder = Some(open_best(settings.codec, &w.params(ow, oh))?);
        w.publish_encoder_stats(sw, sh);
        tracing::info!(%source, ?settings, "비디오 파이프라인 시작");
        Ok(w)
    }

    fn params(&self, w: u32, h: u32) -> EncoderParams {
        EncoderParams {
            width: w,
            height: h,
            fps: self.settings.fps,
            bitrate_kbps: self.settings.bitrate_kbps,
        }
    }

    fn source_size(&self) -> (u32, u32) {
        match &self.source {
            Source::Wgc { capture, converter } => converter
                .source_size()
                .unwrap_or_else(|| capture.item_size()),
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
            let (frame, closed, stop, keyreq, update) = {
                let mut m = lock(&self.shared.mail);
                let deadline = Instant::now() + timeout;
                loop {
                    if m.stop || m.closed || m.frame.is_some() || m.keyframe || m.update.is_some() {
                        break;
                    }
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    m = self
                        .shared
                        .cond
                        .wait_timeout(m, left)
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                }
                (
                    m.frame.take(),
                    m.closed,
                    m.stop,
                    std::mem::take(&mut m.keyframe),
                    m.update.take(),
                )
            };
            if stop {
                break;
            }
            if closed {
                self.shared.set_error(&MediaError::SourceClosed);
                break;
            }
            if let Some((s, reply)) = update {
                let r = self.apply_settings(s);
                let _ = reply.send(r);
            }
            if keyreq {
                self.force_key = true;
                self.key_requested_at = Some(Instant::now());
            }
            let res = match &self.source {
                Source::Wgc { .. } => match frame {
                    Some(f) => self.on_wgc_frame(f),
                    None => {
                        let due = self
                            .key_requested_at
                            .is_some_and(|t| t.elapsed() >= KEYFRAME_WAIT);
                        if due && self.force_key {
                            self.key_requested_at = None;
                            self.encode_current(qpc_now_100ns())
                        } else {
                            Ok(())
                        }
                    }
                },
                Source::Pattern { .. } => self.on_pattern_tick(),
            };
            if let Err(e) = res {
                self.shared.set_error(&e);
                if !self.recover() {
                    break;
                }
            }
        }
        if let Source::Wgc { capture, .. } = &mut self.source {
            capture.stop();
        }
        tracing::info!("비디오 파이프라인 종료");
    }

    /// 인코더 오류 뒤 한 번 다시 열어 본다.
    fn recover(&mut self) -> bool {
        self.encoder = None;
        let (sw, sh) = self.source_size();
        let (ow, oh) = self.settings.output_size(sw, sh);
        match open_best(self.settings.codec, &self.params(ow, oh)) {
            Ok(e) => {
                self.encoder = Some(e);
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

    fn wait_timeout(&self) -> Duration {
        match &self.source {
            Source::Wgc { .. } => match self.key_requested_at {
                Some(t) if self.force_key => KEYFRAME_WAIT.saturating_sub(t.elapsed()),
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
        let (sw, sh) = match &self.source {
            Source::Pattern { .. } => pattern_source_size(&new),
            _ => self.source_size(),
        };
        let (ow, oh) = new.output_size(sw, sh);
        let only_bitrate = VideoSettings {
            bitrate_kbps: old.bitrate_kbps,
            ..new
        } == old
            && self
                .encoder
                .as_ref()
                .is_some_and(|e| e.params.width == ow && e.params.height == oh);
        if only_bitrate
            && let Some(enc) = &mut self.encoder
            && enc.set_bitrate(new.bitrate_kbps)
        {
            self.settings = new;
            *lock(&self.shared.settings) = new;
            tracing::info!(kbps = new.bitrate_kbps, "비트레이트 변경(인코더 유지)");
            return Ok(());
        }
        let params = EncoderParams {
            width: ow,
            height: oh,
            fps: new.fps,
            bitrate_kbps: new.bitrate_kbps,
        };
        let enc = open_best(new.codec, &params)?;
        tracing::info!(?new, encoder = %enc.info.name, "설정 변경, 인코더 재생성");
        self.encoder = Some(enc);
        self.settings = new;
        *lock(&self.shared.settings) = new;
        if new.fps != old.fps {
            lock(&self.shared.mail).pacer.set_fps(new.fps);
        }
        if let Source::Pattern {
            size, next_tick, ..
        } = &mut self.source
        {
            *size = (sw, sh);
            *next_tick = pattern_next_tick(qpc_now_100ns(), new.fps);
        }
        self.publish_encoder_stats(sw, sh);
        self.force_key = true;
        // 화면이 멈춰 있어도 새 설정의 키프레임이 바로 나가게 마지막 화면을 다시 인코딩한다.
        if let Source::Wgc { converter, .. } = &self.source
            && converter.source_size().is_some()
        {
            self.encode_current(qpc_now_100ns())?;
        }
        Ok(())
    }

    fn on_wgc_frame(&mut self, f: CapturedFrame) -> Result<(), MediaError> {
        let Source::Wgc { capture, converter } = &mut self.source else {
            return Ok(());
        };
        let tex = f.texture()?;
        capture.follow_size(f.width, f.height);
        converter.ingest(&tex, f.width, f.height)?;
        let qpc = f.qpc;
        drop(tex);
        drop(f); // 프레임 풀에 바로 돌려준다.
        self.encode_current(qpc)
    }

    fn on_pattern_tick(&mut self) -> Result<(), MediaError> {
        let fps = self.settings.fps;
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
        self.encode_current(t)
    }

    /// 지금 원본(보관 프레임 또는 패턴)을 인코딩한다.
    fn encode_current(&mut self, qpc: i64) -> Result<(), MediaError> {
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
            self.encoder = Some(enc);
            self.force_key = true;
            self.publish_encoder_stats(sw, sh);
        }
        let enc = self.encoder.as_mut().expect("위에서 열었다");
        let t0 = Instant::now();
        let gpu;
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
        }
        let convert_ms = t0.elapsed().as_secs_f32() * 1000.0;
        let key = self.force_key;
        let t1 = Instant::now();
        let packets = &mut self.packets;
        enc.encode(qpc, key, &mut |p| packets.push(p))?;
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
                st.stats.gpu_convert = gpu;
                st.prune(now);
            }
            (self.on_frame)(EncodedVideo {
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

/// WGC 콜백: fps 제한을 적용하고 최신 프레임 칸에 넣는다.
fn on_captured(shared: &Shared, f: CapturedFrame) {
    let dropped = {
        let mut m = lock(&shared.mail);
        if m.stop || !m.pacer.accept(f.qpc) {
            return;
        }
        let dropped = m.frame.replace(f).is_some();
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
