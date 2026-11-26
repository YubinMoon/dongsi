//! 비디오 파이프라인: 캡처(WGC 또는 테스트 패턴) → GPU 변환(NV12) → 인코딩.
//!
//! 스레드 구성
//! - WGC 콜백(시스템 스레드): 프레임 시각 격자([`FrameGrid`])로 프레임을 고르고, 고른 프레임을 격자 시각과 함께
//!   "최신 프레임 칸"(메일함) 하나에 넣는다. 워커가 아직 이전 프레임을 처리 중이면 칸의 이전 프레임은 버린다.
//!   그래서 밀린 프레임이 쌓이지 않는다.
//! - 워커 스레드: 칸에서 (프레임, 격자 시각, 도착 시각)을 꺼내 보관 텍스처로 복사하고(캡처 프레임은 바로 반납),
//!   변환·인코딩한 뒤 `on_frame`을 부른다. 테스트 패턴은 워커가 직접 격자 칸마다 그린다.
//! - 비디오 트랙은 하나다(D-104): 인코더 크기 하나로만 변환·인코딩한다.
//!
//! ## 프레임 시각(D-111)
//!
//! 프레임 시각은 방송 fps(30·60) 간격의 격자 칸 시각이다([`FrameGrid`]). 캡처 프레임은 가장 가까운 칸에 들어가고 칸마다
//! 첫 프레임 하나만 고르며, 새 프레임이 없는 칸은 건너뛴다(반복 프레임은 만들지 않는다). 멈춘 화면 키프레임(설정·비트레이트
//! 변경 뒤 다시 인코딩 포함)과 테스트 패턴도 같은 격자에서 시각을 받는다([`FrameGrid::claim`]). 시각은 프레임마다 엄격히
//! 늘고, 그대로 인코더 pts → [`EncodedVideo::capture_qpc_100ns`] → 송출 시각(RTP·SR)이 된다. 대가로 내용과 시각이 칸 안쪽
//! (최대 간격의 절반)만큼 어긋난다.
//!
//! 시각이 거꾸로 가지 않게, 메일함에 새 프레임이 기다리거나 이번 바퀴에 꺼낸 프레임이 있으면 마지막 화면을 다시 인코딩하지
//! 않는다(그 프레임이 키프레임으로 나간다, `Worker::reencode_last`).
//!
//! ## 프레임을 버리는 곳과 세는 방식
//!
//! | 어디서 | 버리는 프레임 | 세는 곳 |
//! |---|---|---|
//! | WGC 콜백(`capture.rs`) | 콜백 한 번에 쌓여 있던 프레임 중 마지막 것을 뺀 나머지 | 세지 않음 |
//! | 격자([`FrameGrid::accept`]) | 그 칸이나 앞 칸에 이미 고른 프레임이 있는 프레임 | 세지 않음 |
//! | 메일함 | 워커가 꺼내기 전에 새 프레임에 덮인 프레임 | [`VideoStats::dropped_frames`] |
//! | 워커 | 바꾸거나 닫은 캡처의 프레임, 출력이 너무 작은 프레임, 다시 열지 못한 출력 크기의 프레임(D-109) | 세지 않음 |
//! | 송출 큐(`dongsi-net` `Publisher::send_video`) | 큐가 가득 찼을 때 온 인코딩 프레임. 키프레임을 다시 요청한다 | `PublishStats::dropped_frames` |
//!
//! [`VideoStats::capture_fps`]는 격자를 통과해 메일함에 넣은 프레임(테스트 패턴은 틱)을 센다.
//!
//! ## 프레임이 인코더에 가는 두 가지 길([`FramePath`], D-099·D-105)
//!
//! 변환은 늘 GPU(D3D11 Video Processor)다. CPU 변환(swscale)은 없다(D-105).
//!
//! 1. **zero-copy**: 인코더가 `h264_nvenc`이면, 워커가 프레임마다 hw 풀([`HwFramePool`], 인코더 크기 하나)에서 D3D11 NV12
//!    텍스처를 받아 거기에 바로 변환(Blt)하고 그 텍스처를 인코더에 넣는다. 읽어 오기·nvenc 안의 업로드가 없다.
//! 2. **읽어 오기**: 변환한 NV12를 시스템 메모리로 읽어 와 인코더(AMF, QSV, 또는 zero-copy를 쓰지 않는 경우)에 넣는다.
//!
//! zero-copy는 열기가 실패하면(하이브리드 GPU에서 NVENC 세션이 우리 장치에 안 열림 등) 그 파이프라인(방송) 동안 2번 길로
//! 고정한다(`DONGSI_MEDIA_NO_ZEROCOPY=1`이면 처음부터 2번). 테스트 패턴도 [`GpuPattern`]을 [`Converter::ingest`]에 넣어
//! 화면 캡처(WGC)와 같은 길을 탄다.
//!
//! GPU 변환을 쓸 수 없으면 파이프라인을 시작하지 않는다([`MediaError::NoGpuConvert`]). 장치 손실(`DXGI_ERROR_DEVICE_REMOVED`
//! 등)은 [`MediaError::DeviceLost`]로, 읽어 오기 변환 실패는 [`MediaError::GpuConvert`]로 알리고 방송을 멈춘다(재생성하지 않는다).
//!
//! ## 작은 프레임과 인코더 다시 열기(D-109)
//!
//! 출력 크기(원본에 화질 상자를 건 크기)가 160×64보다 작은 프레임은 보관하지 않고 버린다(마지막 화면 유지,
//! [`output_too_small`]). 출력 크기가 바뀌어 인코더를 다시 열지 못하면 옛 인코더를 그대로 두고 그 크기의 프레임만 버린다.
//! 방송은 멈추지 않고, 출력 크기가 다시 바뀌면 다시 열어 본다.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dongsi_core::VideoSettings;
use dongsi_core::priority::{ThreadRole, boost_current_thread};
use serde::Serialize;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};

use crate::capture::{CapturedFrame, WgcCapture};
use crate::clock::{QPC_PER_SEC, instant_from_qpc, qpc_ms, qpc_now_100ns};
use crate::convert::{Converter, output_too_small, region_size};
use crate::d3d::{D3d, create_d3d};
use crate::encoder::{
    EncodedPacket, EncoderParams, FrameTime, VideoEncoder, candidates, has_zero_copy_candidate,
    open_best,
};
use crate::ff::{HwDevice, HwFramePool, HwFramesRef};
use crate::gpu_pattern::{GpuPattern, GpuPatternKind};
use crate::preview::{PreviewFrame, PreviewScaler, PreviewSlot};
use crate::yuv::{pattern_is_white, pattern_source_size};
use crate::{MediaError, SourceId};

/// 인코딩된 비디오 프레임 하나.
#[derive(Debug, Clone)]
pub struct EncodedVideo {
    /// H.264 Annex B(4바이트 시작 코드). 키프레임마다 SPS/PPS가 IDR 앞에 들어 있다.
    /// AUD·filler NAL은 없다.
    pub data: Vec<u8>,
    pub keyframe: bool,
    /// 프레임 시각(QPC 100ns): 방송 fps 격자의 칸 시각(D-111, 모듈 문서 "프레임 시각"). 원본 캡처 시각(WGC
    /// `SystemRelativeTime`)과는 최대 간격의 절반만큼 다르다. 프레임마다 엄격히 는다.
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
    /// 지금 인코더의 ffmpeg 이름(예: `h264_nvenc`). 열기 전에는 비어 있다.
    pub encoder: String,
    /// 출력(인코딩) 크기
    pub width: u32,
    pub height: u32,
    /// 캡처 원본 크기
    pub source_width: u32,
    pub source_height: u32,
    /// 프레임 시각 격자를 통과한 캡처 프레임 수/초(테스트 패턴은 틱 수)
    pub capture_fps: f32,
    pub encode_fps: f32,
    /// 인코더 호출 시간(평균, ms)
    pub avg_encode_ms: f32,
    pub bitrate_kbps: f32,
    /// 메일함에서 새 프레임에 덮여 버린 캡처 프레임 수(누적, 워커가 바빴다). 다른 곳에서 버리는 프레임은 세지 않는다
    /// (모듈 문서 "프레임을 버리는 곳과 세는 방식").
    pub dropped_frames: u64,
    /// (추가) GPU 변환 시간(평균, ms): Blt 호출과, 읽어 오기면 NV12를 읽어 오기까지(테스트 패턴 그리기는 들지 않는다).
    pub avg_convert_ms: f32,
    /// (추가) 프레임 시각(`capture_qpc_100ns`, 격자 시각) → 인코딩 완료(on_frame 직전)(평균, ms).
    /// 격자 시각은 원본 캡처 시각과 최대 간격의 절반만큼 다르고, WGC 시각은 DWM 합성 기준이라 프레임이 실제로 도착한
    /// 시각보다 조금 앞설 수 있어서 이 값은 처리 지연과 다르게(때로 음수로) 나온다.
    pub avg_latency_ms: f32,
    /// (추가) 프레임 도착(WGC 콜백·패턴 틱) → 인코딩 완료(평균, ms). 순수 처리 지연.
    pub avg_pipeline_ms: f32,
    /// (추가) 인코딩한 프레임 수(누적)
    pub frames: u64,
    /// (추가) 그중 키프레임 수(누적)
    pub keyframes: u64,
}

/// 프레임이 인코더에 가는 길(모듈 설명 참고). [`VideoPipeline::frame_path`]로 본다. 변환은 둘 다 GPU다(D-105).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FramePath {
    /// GPU 변환 + NV12를 시스템 메모리로 읽어 와 인코더에 넣는다(`DONGSI_MEDIA_NO_ZEROCOPY=1`, 인코더가 NVENC가 아님, 또는 zero-copy에서 내려옴).
    GpuReadback,
    /// GPU 변환 결과(D3D11 NV12 텍스처)를 NVENC에 바로 넣는다.
    ZeroCopy,
}

impl FramePath {
    fn of(zero_copy: bool) -> Self {
        if zero_copy {
            FramePath::ZeroCopy
        } else {
            FramePath::GpuReadback
        }
    }
}

impl std::fmt::Display for FramePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            FramePath::GpuReadback => "gpu-readback",
            FramePath::ZeroCopy => "zero-copy",
        })
    }
}

/// 프레임 시각 격자(D-111). 고른 프레임마다 방송 fps 간격 칸의 시각을 준다. 비디오 프레임 시각의 출처는 이것 하나다:
/// 화면 캡처 프레임([`accept`](Self::accept))도, 멈춘 화면 키프레임·테스트 패턴([`claim`](Self::claim))도 여기서 받는다.
/// 캡처 스레드가 메일함 잠금 안에서 쓰고, 워커도 같은 잠금 안에서 쓴다.
///
/// - 칸 `k`의 시각은 `t_k = base + floor(k·10^7/fps)`다(QPC 100ns). 60fps는 간격이 정수가 아니라(166666.67) 누적 오차가
///   없게 칸마다 다시 계산한다.
/// - 칸은 `t_k`를 가운데에 둔 간격 하나 너비다: 시각 `t`는 칸 `round((t − base)·fps/10^7)`에 든다.
/// - `base`는 격자의 첫 프레임(또는 `claim`) 시각이다. 정초 격자(테스트 패턴)는 그 시각이 든 초의 정초(QPC가 1초의 배수)로
///   잡아, 매 초 첫 칸이 정확히 정초가 된다(흰 프레임이 테스트 톤의 삐 소리와 같은 시각에 오게, [`pattern_is_white`]).
/// - 내보내는 시각은 늘 엄격히 는다(인코더 pts·RTP 단조). fps를 바꾸면 격자를 새로 잡되, 새 격자에서도 마지막으로 낸 시각보다
///   뒤인 칸만 쓴다.
#[derive(Debug, Clone)]
pub(crate) struct FrameGrid {
    fps: i64,
    /// 칸을 정초에 맞춘다(테스트 패턴).
    whole_seconds: bool,
    /// 지금 격자의 칸 0 시각. `None`이면 다음 `accept`·`claim`이 정한다.
    base: Option<i64>,
    /// 지금 격자에서 마지막으로 쓴 칸.
    last_cell: Option<i64>,
    /// 마지막으로 낸 시각. fps를 바꿔 격자를 새로 잡아도 남는다.
    last_time: Option<i64>,
}

impl FrameGrid {
    /// `whole_seconds`면 칸을 정초에 맞춘다(테스트 패턴).
    pub fn new(fps: u32, whole_seconds: bool) -> Self {
        FrameGrid {
            fps: fps.max(1) as i64,
            whole_seconds,
            base: None,
            last_cell: None,
            last_time: None,
        }
    }

    /// fps를 바꾸고 격자를 새로 잡는다(다음 `accept`·`claim`이 새 기준이 된다). 같은 fps면 그대로다.
    pub fn set_fps(&mut self, fps: u32) {
        let fps = fps.max(1) as i64;
        if fps != self.fps {
            self.fps = fps;
            self.base = None;
            self.last_cell = None;
        }
    }

    /// 화면 캡처 프레임(캡처 시각 `qpc`)을 가장 가까운 칸에 넣는다. 그 칸 또는 그 앞 칸에 이미 고른 프레임이 있으면 버리고
    /// `None`(칸마다 첫 프레임 하나), 아니면 그 칸의 시각. 새 프레임이 없던 칸은 건너뛴다(반복 프레임을 만들지 않는다).
    pub fn accept(&mut self, qpc: i64) -> Option<i64> {
        let base = self.base_for(qpc);
        let cell = self.cell_of(base, qpc);
        (cell >= self.first_free(base)).then(|| self.take(base, cell))
    }

    /// 지금(`now`) 프레임 하나를 낼 칸의 시각: `now`가 든 칸. 그 칸을 이미 썼으면 쓰지 않은 다음 칸이다(멈춘 화면 키프레임,
    /// 테스트 패턴).
    pub fn claim(&mut self, now: i64) -> i64 {
        let base = self.base_for(now);
        let cell = self.cell_of(base, now).max(self.first_free(base));
        self.take(base, cell)
    }

    /// 다음에 쓸 수 있는 칸의 시각(테스트 패턴이 이때까지 잔다). 격자를 아직(또는 fps를 바꾼 뒤 새로) 잡지 않았으면 `None`
    /// (바로 낸다).
    pub fn next_due(&self) -> Option<i64> {
        let base = self.base?;
        let first = self.first_free(base);
        (first != i64::MIN).then(|| self.time_of(base, first))
    }

    fn base_for(&mut self, t: i64) -> i64 {
        let whole_seconds = self.whole_seconds;
        *self.base.get_or_insert_with(|| {
            if whole_seconds {
                t.div_euclid(QPC_PER_SEC) * QPC_PER_SEC
            } else {
                t
            }
        })
    }

    fn time_of(&self, base: i64, cell: i64) -> i64 {
        base + (cell * QPC_PER_SEC).div_euclid(self.fps)
    }

    /// `t`가 든 칸: `round((t − base)·fps / 10^7)`.
    fn cell_of(&self, base: i64, t: i64) -> i64 {
        ((t - base) * self.fps * 2 + QPC_PER_SEC).div_euclid(2 * QPC_PER_SEC)
    }

    /// 지금 격자에서 쓸 수 있는 첫 칸: 마지막 칸 다음. 격자를 새로 잡은 뒤면 마지막으로 낸 시각보다 뒤인 첫 칸. 낸 적이 없으면
    /// 어느 칸이든 쓴다(`i64::MIN`).
    fn first_free(&self, base: i64) -> i64 {
        match (self.last_cell, self.last_time) {
            (Some(cell), _) => cell + 1,
            // time_of(base, k) > last ⟺ floor(k·10^7/fps) ≥ last − base + 1 ⟺ k ≥ ceil((last − base + 1)·fps / 10^7)
            (None, Some(last)) => -(-(last - base + 1) * self.fps).div_euclid(QPC_PER_SEC),
            (None, None) => i64::MIN,
        }
    }

    fn take(&mut self, base: i64, cell: i64) -> i64 {
        let t = self.time_of(base, cell);
        self.last_cell = Some(cell);
        self.last_time = Some(t);
        t
    }
}

/// 메일함의 캡처 프레임 하나.
struct MailFrame {
    /// 캡처 번호(`Worker::generation`). 바꾸거나 닫은 캡처의 프레임은 워커가 쓰지 않고 버린다(그 프레임 풀은 이미 닫혔다).
    generation: u64,
    frame: CapturedFrame,
    /// 격자가 준 프레임 시각([`FrameGrid::accept`]).
    at: i64,
}

struct Mail {
    frame: Option<MailFrame>,
    /// 프레임 시각 격자(D-111). 캡처 콜백과 워커가 이 잠금 안에서 쓴다.
    grid: FrameGrid,
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
    /// (전체 시험 전용) GPU 테스트 패턴의 종류를 바꾼다.
    #[cfg(feature = "full-tests")]
    pattern_kind: Option<GpuPatternKind>,
    /// (전체 시험 전용) GPU 테스트 패턴의 원본 크기를 바꾼다.
    #[cfg(feature = "full-tests")]
    pattern_size: Option<(u32, u32)>,
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
    /// 지금 인코더에 건 비트레이트(kbps).
    encoder_kbps: AtomicU32,
    /// 대상이 닫혀 새 대상을 기다리는 중(`hold`).
    waiting: AtomicBool,
    error: Mutex<Option<String>>,
    stats: Mutex<StatsState>,
    started: Instant,
    /// 송출 화면 미리 보기 칸(화면이 요청할 때만 워커가 채운다).
    preview: PreviewSlot,
    /// 지금 프레임이 zero-copy 길로 가는지([`FramePath`]).
    zero_copy_path: AtomicBool,
    /// zero-copy를 쓰려다 내려간 이유(내려갔을 때만).
    zero_copy_fallback: Mutex<Option<String>>,
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
    /// 하드웨어 인코더가 없거나([`MediaError::NoEncoder`]) GPU 변환을 쓸 수 없으면([`MediaError::NoGpuConvert`]) 시작하지 않는다.
    ///
    /// fps는 `settings.fps`(30 또는 60, D-107) 그대로 프레임 시각 격자(D-111)와 인코더에 쓴다. 캡처 대상의 주사율은 보지 않는다.
    pub fn start(
        source: SourceId,
        settings: VideoSettings,
        on_frame: Box<dyn FnMut(EncodedVideo) + Send>,
    ) -> Result<Self, MediaError> {
        let settings = settings.clamped();
        if candidates().is_empty() {
            return Err(MediaError::NoEncoder);
        }
        // 워커가 끝날 때 RoUninitialize가 MTA를 내리지 않게(다음 방송의 WGC 팩터리가 죽은 MTA를 쓰지 않게).
        crate::com::keep_mta();
        let shared = Arc::new(Shared {
            mail: Mutex::new(Mail {
                frame: None,
                grid: FrameGrid::new(settings.fps, matches!(source, SourceId::TestPattern)),
                closed: None,
                stop: false,
                keyframe: false,
                update: None,
                switch: None,
                hold: None,
                cap: None,
                #[cfg(feature = "full-tests")]
                pattern_kind: None,
                #[cfg(feature = "full-tests")]
                pattern_size: None,
            }),
            cond: Condvar::new(),
            settings: Mutex::new(settings),
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
            zero_copy_path: AtomicBool::new(false),
            zero_copy_fallback: Mutex::new(None),
        });
        let (init_tx, init_rx) = mpsc::sync_channel(1);
        let worker_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("dongsi-video".into())
            .spawn(move || {
                let _boost = boost_current_thread(ThreadRole::Video);
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
    /// 20ms 뒤 마지막 화면을 다시 인코딩해 키프레임을 낸다(시각은 지금이 든 격자 칸, D-111).
    pub fn request_keyframe(&self) {
        let mut m = lock(&self.shared.mail);
        m.keyframe = true;
        self.shared.cond.notify_all();
    }

    /// 캡처를 멈추지 않고 해상도·fps·비트레이트를 바꾼다.
    ///
    /// 워커가 새 설정을 적용(필요하면 인코더를 다시 만듦)할 때까지 기다리고, 그 결과를
    /// 돌려준다. 실패하면 이전 설정이 유지된다. 바뀐 뒤 첫 프레임은 키프레임이다.
    /// 비트레이트만 바뀌고 인코더가 지원하면(nvenc) 인코더를 그대로 두고 바꾼다
    /// (이때는 키프레임을 강제하지 않는다). 인코더에 가는 값(출력 크기·fps·비트레이트)이 그대로면 인코더도 그대로다.
    pub fn update(&self, settings: VideoSettings) -> Result<(), MediaError> {
        let settings = settings.clamped();
        if !self.is_running() {
            return Err(MediaError::Thread(
                "비디오 파이프라인이 멈춰 있습니다".into(),
            ));
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
    ///
    /// `Some(0)`은 무시한다(지금 상한을 그대로 둔다): 0이 상한으로 새어 들어와도 인코더가 0kbps로 열리지 않게 하는 방어다.
    pub fn set_bitrate_cap(&self, kbps: Option<u32>) {
        if kbps == Some(0) {
            tracing::debug!("비트레이트 상한 0은 무시합니다(지금 상한 유지)");
            return;
        }
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
    /// 바뀐 뒤 첫 프레임은 키프레임이고, 크기가 다르면 인코더를 새 크기로 다시 연다. fps는 설정 그대로다
    /// (새 대상의 주사율을 보지 않는다, D-107). 테스트 패턴 방송에서는 쓸 수 없다.
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

    /// 지금 적용된 설정. `fps`가 프레임 수 제한·인코더가 쓰는 값이다(30 또는 60).
    pub fn settings(&self) -> VideoSettings {
        *lock(&self.shared.settings)
    }

    pub fn stats(&self) -> VideoStats {
        self.shared.stats()
    }

    /// (전체 시험 전용) GPU 테스트 패턴의 종류를 `noise`(프레임마다 전체가 바뀌는 무작위 그림, 압축하기 어렵다)로 바꾸거나
    /// 막대 그림으로 되돌린다. GPU 테스트 패턴이 아니면 아무 일도 없다.
    #[cfg(feature = "full-tests")]
    pub fn set_test_pattern_noise(&self, noise: bool) {
        let mut m = lock(&self.shared.mail);
        m.pattern_kind = Some(if noise {
            GpuPatternKind::Noise
        } else {
            GpuPatternKind::Bars
        });
        self.shared.cond.notify_all();
    }

    /// (전체 시험 전용) GPU 테스트 패턴의 원본 크기를 `width`×`height`로 바꾼다(화면 캡처에서 창 크기가 바뀐 것처럼).
    /// 다음 틱부터 그 크기로 보관하고, 출력 크기가 바뀌면 인코더를 다시 연다(D-109의 작은 프레임·다시 열기 실패 규칙을 탄다).
    /// `width`는 640 이상이어야 한다([`GpuPattern::new`]). 테스트 패턴이 아니면 아무 일도 없다.
    #[cfg(feature = "full-tests")]
    pub fn set_test_pattern_size(&self, width: u32, height: u32) {
        let mut m = lock(&self.shared.mail);
        m.pattern_size = Some((width, height));
        self.shared.cond.notify_all();
    }

    /// 프레임이 지금 인코더에 가는 길(시험·측정용). 인코더를 (다시) 열 때와 프레임마다 갱신한다.
    pub fn frame_path(&self) -> FramePath {
        FramePath::of(self.shared.zero_copy_path.load(Ordering::SeqCst))
    }

    /// zero-copy(NVENC에 텍스처를 바로 넣기)를 쓰려다 내려갔다면 그 이유. 내려가면 이 파이프라인(방송) 동안 GPU 변환 + 읽어 오기
    /// 길로 고정된다. 내려간 적이 없으면 `None`(`DONGSI_MEDIA_NO_ZEROCOPY=1`·인코더가 NVENC가 아닌 경우도 `None`).
    pub fn zero_copy_fallback(&self) -> Option<String> {
        lock(&self.shared.zero_copy_fallback).clone()
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

/// GPU 테스트 패턴: 화면 캡처(WGC)와 같은 길(`Converter::ingest` → 보관본 → 변환)을 타는 BGRA 텍스처 소스.
struct GpuTest {
    d3d: D3d,
    converter: Converter,
    pattern: GpuPattern,
    kind: GpuPatternKind,
}

/// GPU 테스트 패턴을 만든다. GPU 변환을 쓸 수 없거나 패턴을 만들지 못하면 오류다(D-105: CPU로 그리는 패턴은 없다).
fn make_gpu_test(size: (u32, u32)) -> Result<GpuTest, MediaError> {
    let d3d = create_d3d()?;
    let converter = Converter::new(d3d.clone())?;
    let pattern = GpuPattern::new(&d3d, size.0, size.1, GpuPatternKind::Bars)?;
    Ok(GpuTest {
        d3d,
        converter,
        pattern,
        kind: GpuPatternKind::Bars,
    })
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
        /// 지난 틱의 시각(흰 프레임 판정, [`pattern_is_white`]). 틱은 격자 칸마다다([`FrameGrid::next_due`]).
        prev: Option<i64>,
        index: u64,
        white: bool,
        /// WGC와 같은 길을 타는 GPU 패턴.
        gpu: GpuTest,
    },
}

impl Source {
    /// 장치와 변환기(WGC 또는 GPU 테스트 패턴).
    fn gfx(&self) -> (&D3d, &Converter) {
        match self {
            Source::Wgc { d3d, converter, .. } => (d3d, converter),
            Source::Pattern { gpu: g, .. } => (&g.d3d, &g.converter),
        }
    }

    fn gfx_mut(&mut self) -> (&D3d, &mut Converter) {
        match self {
            Source::Wgc { d3d, converter, .. } => (d3d, converter),
            Source::Pattern { gpu: g, .. } => (&g.d3d, &mut g.converter),
        }
    }
}

/// NVENC zero-copy(D3D11 NV12 텍스처를 인코더에 바로 넣기) 상태. 파이프라인(방송)마다 하나.
///
/// - hw 장치([`HwDevice`])는 처음 필요할 때 우리 D3D11 장치로 만들고, 풀([`HwFramePool`])은 인코더 크기의 것 하나다.
///   크기가 바뀌면 풀을 새로 만든다. 풀을 다시 만들면 세대가 달라져 그 풀을 쓰던 인코더가 다시 열린다(옛 풀의 프레임을 쥔
///   인코더는 refcount로 계속 쓸 수 있다).
/// - 열기가 실패하면 `fallback`에 이유를 남기고 이 파이프라인 동안 쓰지 않는다.
struct ZeroCopy {
    /// `DONGSI_MEDIA_NO_ZEROCOPY=1`(시험·비교용).
    env_off: bool,
    device: Option<HwDevice>,
    pool: Option<HwFramePool>,
    /// 내려간 이유. 있으면 이 파이프라인은 읽어 오기 경로(GPU 변환 + 읽어 오기)로 고정이다.
    fallback: Option<String>,
}

impl ZeroCopy {
    fn new() -> Self {
        ZeroCopy {
            env_off: std::env::var("DONGSI_MEDIA_NO_ZEROCOPY").is_ok_and(|v| v == "1"),
            device: None,
            pool: None,
            fallback: None,
        }
    }

    /// 환경 변수와 내려가기 상태만 본 허용 여부(인코더가 zero-copy를 받는지는 호출자가 본다).
    fn allowed(&self) -> bool {
        !self.env_off && self.fallback.is_none()
    }

    /// `w`×`h` 풀의 frames ctx 참조. 풀이 없거나 크기가 다르면 (장치가 없으면 장치와 함께) 새로 만든다.
    fn frames_ref(&mut self, d3d: &D3d, w: u32, h: u32) -> Result<HwFramesRef, MediaError> {
        if self.pool_ref(w, h).is_none() {
            if self.device.is_none() {
                self.device = Some(HwDevice::new(&d3d.device, &d3d.context)?);
            }
            let dev = self.device.as_ref().expect("방금 만들었다");
            self.pool = Some(HwFramePool::new(dev, w, h)?);
        }
        let pool = self.pool.as_ref().expect("방금 만들었다");
        HwFramesRef::of_pool(pool)
            .ok_or_else(|| MediaError::Graphics("hw frames ctx 참조를 만들지 못했습니다".into()))
    }

    /// 크기가 `w`×`h`인 풀(있으면).
    fn pool_ref(&self, w: u32, h: u32) -> Option<&HwFramePool> {
        self.pool
            .as_ref()
            .filter(|p| p.width() == w && p.height() == h)
    }

    fn pool_mut(&mut self, w: u32, h: u32) -> Option<&mut HwFramePool> {
        self.pool
            .as_mut()
            .filter(|p| p.width() == w && p.height() == h)
    }
}

/// 키프레임 요청 뒤 새 프레임을 기다리는 시간. 넘으면 마지막 화면을 다시 인코딩한다.
const KEYFRAME_WAIT: Duration = Duration::from_millis(20);

struct Worker {
    shared: Arc<Shared>,
    source: Source,
    /// 지금 설정. `fps`(30 또는 60)를 프레임 시각 격자와 인코더에 그대로 쓴다.
    settings: VideoSettings,
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
    /// 출력이 너무 작은 프레임([`output_too_small`])을 건너뛰는 중(기록을 한 번만 남기려고).
    tiny_frames: bool,
    /// 출력 크기가 바뀌어 인코더를 다시 열다 실패한 출력 크기(D-109). 이 크기의 프레임은 옛 인코더를 두고 버리고,
    /// 출력 크기가 이 값에서 바뀌면 지운다(그 뒤 이 크기가 다시 오면 다시 열어 본다).
    failed_output: Option<(u32, u32)>,
    /// 다시 열기 실패 경고를 남긴 출력 크기(크기마다 한 번).
    warned_outputs: HashSet<(u32, u32)>,
    /// NVENC zero-copy 상태.
    zc: ZeroCopy,
    /// 마지막 `encode_current`가 hw 입력 경로 안에서 멈췄다(오류 뒤 읽어 오기 경로로 내릴지 정하는 데 쓴다).
    in_hw: bool,
    /// 마지막으로 GPU 장치가 사라졌는지 확인한 시각.
    device_checked: Instant,
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
                prev: None,
                index: 0,
                white: false,
                gpu: make_gpu_test(pattern_source_size(&settings))?,
            },
            _ => {
                let d3d = create_d3d()?;
                let converter = Converter::new(d3d.clone())?;
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
            failed_output: None,
            warned_outputs: HashSet::new(),
            zc: ZeroCopy::new(),
            in_hw: false,
            device_checked: Instant::now(),
        };
        // 인코더를 미리 열어 본다(인코더·해상도 문제를 start()에서 알리기 위해).
        // 최소화된 창은 아주 작은 크기를 알려 주므로 하한을 둔다. 첫 프레임 크기가 다르면
        // 그때 다시 연다.
        let (sw, sh) = w.source_size();
        let (ow, oh) = settings.output_size(sw.max(320), sh.max(180));
        if let Some(sink) = crate::trace::video() {
            sink.mark(
                "pipeline",
                &format!(
                    "source={} size={}x{} fps={} kbps={}",
                    w.source_id,
                    settings.width,
                    settings.height,
                    settings.fps,
                    settings.bitrate_kbps
                ),
            );
        }
        let params = w.params(ow, oh);
        let enc = w.open_encoder(&params)?;
        w.set_encoder(enc);
        w.publish_encoder_stats(sw, sh);
        // 인코더가 준비된 뒤에 프레임을 받기 시작한다(그 전 프레임이 버려진 것으로 세지 않게).
        if let Source::Wgc {
            capture: Some(capture),
            ..
        } = &w.source
        {
            capture.begin()?;
        }
        tracing::info!(%source, ?settings, "비디오 파이프라인 시작");
        Ok(w)
    }

    fn params(&self, w: u32, h: u32) -> EncoderParams {
        EncoderParams {
            width: w,
            height: h,
            fps: self.settings.fps,
            bitrate_kbps: capped_bitrate(&self.settings, self.cap),
        }
    }

    fn set_encoder(&mut self, enc: VideoEncoder) {
        self.shared
            .encoder_kbps
            .store(enc.params.bitrate_kbps, Ordering::SeqCst);
        self.encoder = Some(enc);
        self.publish_path();
    }

    /// 원본 크기에서 인코더의 출력 크기.
    fn output_size(&self, sw: u32, sh: u32) -> (u32, u32) {
        self.settings.output_size(sw, sh)
    }

    /// 프레임이 인코더에 가는 길을 공개한다([`VideoPipeline::frame_path`]).
    fn publish_path(&self) {
        let hw = self.encoder.as_ref().is_some_and(VideoEncoder::is_hw_input);
        self.shared.zero_copy_path.store(hw, Ordering::SeqCst);
    }

    /// zero-copy를 이 파이프라인(방송) 동안 쓰지 않기로 한다. 경고는 처음 한 번만 남긴다. 풀·장치는 놓는다(프레임을 쥔
    /// 인코더는 refcount로 계속 쓸 수 있다). 인코더는 다음 프레임에서 시스템 메모리 입력(읽어 오기)으로 다시 열린다.
    fn zc_fall_back(&mut self, reason: String) {
        if self.zc.fallback.is_none() {
            tracing::warn!(
                %reason,
                "NVENC zero-copy를 쓸 수 없어 이 방송은 GPU 변환 + 읽어 오기 경로로 보냅니다"
            );
            *lock(&self.shared.zero_copy_fallback) = Some(reason.clone());
            self.zc.fallback = Some(reason);
        }
        self.zc.pool = None;
        self.zc.device = None;
    }

    /// 인코더가 hw 입력(zero-copy)으로 열릴 때 쓸 frames ctx. 허용되지 않거나 NVENC 후보가 없으면 `None`(시스템 메모리
    /// 입력으로 연다). 풀·장치를 만들지 못하면 내려간다.
    fn hw_frames_for(&mut self, w: u32, h: u32) -> Option<HwFramesRef> {
        if !self.zc.allowed() || !has_zero_copy_candidate() {
            return None;
        }
        let (d3d, _) = self.source.gfx();
        match self.zc.frames_ref(d3d, w, h) {
            Ok(f) => Some(f),
            Err(e) => {
                self.zc_fall_back(format!("hw 풀을 만들지 못했습니다: {e}"));
                None
            }
        }
    }

    /// 인코더를 연다(zero-copy가 되면 D3D11 입력으로). hw 입력 열기가 실패해 시스템 메모리 입력으로 열었다면 이 파이프라인은
    /// 읽어 오기 경로(GPU 변환 + 읽어 오기)로 고정한다.
    fn open_encoder(&mut self, params: &EncoderParams) -> Result<VideoEncoder, MediaError> {
        let hw = self.hw_frames_for(params.width, params.height);
        let opened = open_best(params, hw.as_ref())?;
        drop(hw);
        if let Some(reason) = opened.hw_error {
            self.zc_fall_back(format!(
                "{} 인코더를 D3D11 텍스처 입력으로 열지 못했습니다: {reason}",
                opened.enc.info.name
            ));
        } else if opened.enc.is_hw_input() {
            tracing::info!(
                encoder = %opened.enc.info.name,
                width = params.width,
                height = params.height,
                "NVENC zero-copy 입력으로 열었습니다(D3D11 NV12 텍스처)"
            );
        }
        Ok(opened.enc)
    }

    /// 다음 프레임에서 인코더를 (다시) 열어야 하는지: 인코더가 없거나, 출력 크기가 다르거나, 입력 방식(hw/시스템 메모리)이 달라졌거나,
    /// hw 입력인데 풀이 다시 만들어져 세대가 달라졌다(**인코더 하나는 평생 frames ctx 하나**).
    fn encoder_stale(&self, ow: u32, oh: u32) -> bool {
        let Some(enc) = &self.encoder else {
            return true;
        };
        if enc.params.width != ow || enc.params.height != oh {
            return true;
        }
        let want_hw = self.zc.allowed() && enc.zero_copy_capable();
        if want_hw != enc.is_hw_input() {
            return true;
        }
        want_hw && self.zc.pool_ref(ow, oh).map(HwFramePool::generation) != enc.hw_generation()
    }

    /// 비트레이트 상한을 바꾼다(`VideoPipeline::set_bitrate_cap`). 실패하면 이전 인코더와 상한이 그대로다.
    /// `frame_waiting`: 이번 바퀴에 인코딩할 새 프레임이 있다([`reencode_last`](Self::reencode_last)를 하지 않는다).
    fn apply_cap(&mut self, cap: Option<u32>, frame_waiting: bool) -> Result<(), MediaError> {
        if cap == self.cap {
            return Ok(());
        }
        self.retune(cap, frame_waiting)
    }

    /// 인코더의 비트레이트를 `min(설정, cap)`으로 맞추고 성공하면 `self.cap = cap`으로 적는다(이미 같으면 아무 일도 없다).
    fn retune(&mut self, cap: Option<u32>, frame_waiting: bool) -> Result<(), MediaError> {
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
        // nvenc는 hw 입력이어도 인코더를 그대로 두고 비트레이트만 바꾼다(D-081).
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
        let new = self.open_encoder(&params)?;
        tracing::info!(from, to = kbps, encoder = %new.info.name, "네트워크에 맞춰 비트레이트 변경(인코더 재생성)");
        self.set_encoder(new);
        self.cap = cap;
        let (sw, sh) = self.source_size();
        self.publish_encoder_stats(sw, sh);
        self.force_key = true;
        // 화면이 멈춰 있어도 새 인코더의 키프레임이 바로 나가게 마지막 화면을 다시 인코딩한다.
        if !frame_waiting {
            self.reencode_last()?;
        }
        Ok(())
    }

    /// 새 설정을 적는다. fps가 바뀌면 프레임 시각 격자를 새 간격으로 다시 잡는다(D-111).
    fn commit_settings(&mut self, new: VideoSettings) {
        if new.fps != self.settings.fps {
            lock(&self.shared.mail).grid.set_fps(new.fps);
        }
        self.settings = new;
        *lock(&self.shared.settings) = new;
    }

    /// 원본 크기: 보관한 프레임의 크기. 아직 없으면 캡처 대상(테스트 패턴은 패턴)의 크기.
    fn source_size(&self) -> (u32, u32) {
        match &self.source {
            Source::Wgc {
                capture, converter, ..
            } => converter
                .source_size()
                .unwrap_or_else(|| capture.as_ref().map_or((0, 0), WgcCapture::item_size)),
            Source::Pattern { gpu: g, .. } => g
                .converter
                .source_size()
                .unwrap_or_else(|| g.pattern.size()),
        }
    }

    fn publish_encoder_stats(&self, sw: u32, sh: u32) {
        let mut st = lock(&self.shared.stats);
        if let Some(e) = &self.encoder {
            st.stats.encoder = e.info.name.clone();
            st.stats.width = e.params.width;
            st.stats.height = e.params.height;
        }
        st.stats.source_width = sw;
        st.stats.source_height = sh;
    }

    fn run(&mut self) {
        loop {
            let timeout = self.wait_timeout();
            // 키프레임 대기처럼 짧은 시한은 조건 변수 시간 제한(시스템 타이머 단위, 약 15ms)이
            // 부정확하므로 고해상도 sleep으로 1ms씩 확인한다.
            let precise = timeout < Duration::from_millis(50);
            let (frame, closed, stop, keyreq, update, switch, hold, cap) = {
                let deadline = Instant::now() + timeout;
                let mut m = lock(&self.shared.mail);
                loop {
                    if m.stop
                        || m.closed.is_some()
                        || m.frame.is_some()
                        || m.keyframe
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
            // GPU 장치가 사라졌으면(프레임이 더 오지 않아 오류조차 나지 않을 수 있다) 멈추고 알린다.
            if let Some(e) = self.check_device() {
                self.shared.set_error(&e);
                break;
            }
            // 바꾸거나 닫은 캡처의 프레임은 버린다(프레임 풀이 닫혀 텍스처를 쓸 수 없다).
            let frame = frame.filter(|f| f.generation == self.generation);
            // 꺼낸 새 프레임이 있으면 상한·설정을 바꾼 뒤 마지막 화면을 다시 인코딩하지 않는다: 그 프레임이 키프레임으로
            // 나가고, 다시 인코딩하면 그 프레임보다 뒤 칸을 써서 시각이 거꾸로 간다(D-111).
            let frame_waiting = frame.is_some();
            // 상한을 설정보다 먼저 적용한다(둘을 같이 바꾸면 인코더를 한 번만 바꾼다).
            if let Some(cap) = cap
                && let Err(e) = self.apply_cap(cap, frame_waiting)
            {
                tracing::warn!("비트레이트 상한을 적용하지 못했습니다(이전 인코더 유지): {e}");
            }
            if let Some((s, reply)) = update {
                let r = self.apply_settings(s, frame_waiting);
                let _ = reply.send(r);
            }
            #[cfg(feature = "full-tests")]
            {
                let (kind, size) = {
                    let mut m = lock(&self.shared.mail);
                    (m.pattern_kind.take(), m.pattern_size.take())
                };
                if let Some(kind) = kind {
                    self.set_pattern_kind(kind);
                }
                if let Some(size) = size {
                    self.set_pattern_size(size);
                }
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
                            self.reencode_last()
                        } else {
                            Ok(())
                        }
                    }
                },
                Source::Pattern { .. } => self.on_pattern_tick(),
            };
            if let Err(e) = res {
                // 장치가 사라졌으면 캡처·변환·인코더 세션이 모두 그 장치에 묶여 있어 인코더만 다시 열어서는 소용이 없다.
                // 장치·컨버터·캡처·풀·인코더를 통째로 다시 만드는 대신(NVENC 세션 정리가 사라진 장치에서 멈출 위험이 있고 이
                // PC에서 재현할 수 없다) 방송을 멈추고 알린다.
                if let Some(lost) = self.device_lost() {
                    tracing::error!(cause = %e, "그래픽 장치 손실로 방송을 멈춥니다");
                    self.shared.set_error(&lost);
                    break;
                }
                self.shared.set_error(&e);
                // GPU 변환(읽어 오기)이 실패하면 다른 변환 경로가 없다(D-105): 장치 손실처럼 방송을 멈춘다(recover로 되풀이하지 않는다).
                if matches!(e, MediaError::GpuConvert(_)) {
                    break;
                }
                // hw 입력(zero-copy) 경로에서 난 오류면 이 방송은 읽어 오기 경로(GPU 변환 + 읽어 오기)로 내려가 다시 열어 본다.
                if self.in_hw {
                    self.zc_fall_back(format!("hw 입력 경로 오류: {e}"));
                }
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

    /// (전체 시험 전용) GPU 테스트 패턴의 종류를 바꾼다. 그림만 바뀌고 크기·인코더는 그대로다.
    #[cfg(feature = "full-tests")]
    fn set_pattern_kind(&mut self, kind: GpuPatternKind) {
        if let Source::Pattern { gpu: g, .. } = &mut self.source
            && g.kind != kind
        {
            let (w, h) = g.pattern.size();
            match GpuPattern::new(&g.d3d, w, h, kind) {
                Ok(p) => {
                    g.pattern = p;
                    g.kind = kind;
                }
                Err(e) => tracing::warn!(?kind, "GPU 테스트 패턴 종류를 바꾸지 못했습니다: {e}"),
            }
        }
    }

    /// (전체 시험 전용) GPU 테스트 패턴의 원본 크기를 바꾼다. 인코더는 그대로이고, 다음 틱의 보관·인코딩이 새 크기를 본다.
    #[cfg(feature = "full-tests")]
    fn set_pattern_size(&mut self, (w, h): (u32, u32)) {
        if let Source::Pattern { gpu: g, .. } = &mut self.source
            && g.pattern.size() != (w, h)
        {
            match GpuPattern::new(&g.d3d, w, h, g.kind) {
                Ok(p) => g.pattern = p,
                Err(e) => tracing::warn!(w, h, "GPU 테스트 패턴 크기를 바꾸지 못했습니다: {e}"),
            }
        }
    }

    /// 지금 GPU 장치가 사라졌는지(`GetDeviceRemovedReason`). 사라졌으면 `MediaError::DeviceLost`.
    fn device_lost(&self) -> Option<MediaError> {
        self.source.gfx().0.lost_error()
    }

    /// 0.5초마다 [`device_lost`](Self::device_lost)를 확인한다.
    fn check_device(&mut self) -> Option<MediaError> {
        if self.device_checked.elapsed() < Duration::from_millis(500) {
            return None;
        }
        self.device_checked = Instant::now();
        self.device_lost()
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
        Ok(())
    }

    /// 인코더 오류 뒤 한 번 다시 열어 본다.
    fn recover(&mut self) -> bool {
        self.encoder = None;
        let (sw, sh) = self.source_size();
        let (ow, oh) = self.output_size(sw, sh);
        let params = self.params(ow, oh);
        match self.open_encoder(&params) {
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

    /// 새 설정을 적용한다(`VideoPipeline::update`). `frame_waiting`은 [`apply_cap`](Self::apply_cap)과 같다.
    fn apply_settings(
        &mut self,
        new: VideoSettings,
        frame_waiting: bool,
    ) -> Result<(), MediaError> {
        let old = self.settings;
        if new == old {
            return Ok(());
        }
        let (sw, sh) = match &self.source {
            Source::Pattern { .. } => pattern_source_size(&new),
            _ => self.source_size(),
        };
        let (ow, oh) = new.output_size(sw, sh);
        let kbps = capped_bitrate(&new, self.cap);
        let params = EncoderParams {
            width: ow,
            height: oh,
            fps: new.fps,
            bitrate_kbps: kbps,
        };
        let current = self.encoder.as_ref().map(|e| e.params);
        // 인코더에 가는 값이 그대로면(예: 원본이 두 상자보다 작음) 인코더를 그대로 둔다.
        if current == Some(params) {
            self.commit_settings(new);
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
            self.commit_settings(new);
            self.shared.encoder_kbps.store(kbps, Ordering::SeqCst);
            tracing::info!(
                kbps,
                setting = new.bitrate_kbps,
                "비트레이트 변경(인코더 유지)"
            );
            return Ok(());
        }
        // 테스트 패턴은 새 크기의 GPU 패턴을 인코더보다 먼저 만든다(만들지 못하면 설정을 바꾸지 않는다).
        let new_pattern = match &self.source {
            Source::Pattern { gpu: g, .. } if g.pattern.size() != (sw, sh) => {
                Some(GpuPattern::new(&g.d3d, sw, sh, g.kind)?)
            }
            _ => None,
        };
        let enc = self.open_encoder(&params)?;
        tracing::info!(?new, kbps, encoder = %enc.info.name, zero_copy = enc.is_hw_input(), "설정 변경, 인코더 재생성");
        self.set_encoder(enc);
        // fps가 바뀌면 격자도 새로 잡는다. 테스트 패턴의 다음 틱은 새 격자의 칸이다.
        self.commit_settings(new);
        if let (Source::Pattern { gpu: g, .. }, Some(p)) = (&mut self.source, new_pattern) {
            g.pattern = p;
        }
        self.publish_encoder_stats(sw, sh);
        self.force_key = true;
        // 화면이 멈춰 있어도 새 설정의 키프레임이 바로 나가게 마지막 화면을 다시 인코딩한다.
        if !frame_waiting {
            self.reencode_last()?;
        }
        Ok(())
    }

    /// 캡처 텍스처의 왼쪽 위 `w`×`h` 영역을 보관본으로 넣는다. 작은 프레임 판정(D-109)은 여기 한 곳에서만 한다: 출력 크기
    /// (원본에 화질 상자를 건 크기)가 [`output_too_small`]이면 넣지 않고 `false`라, 인코더를 그 크기로 다시 열지 않고 마지막
    /// 화면(보관본)을 둔다. 그래서 멈춘 화면 키프레임도 마지막으로 보관한 정상 화면을 쓴다. 건너뛰기 시작과 끝을 한 번씩만 적는다.
    fn ingest(&mut self, tex: &ID3D11Texture2D, w: u32, h: u32) -> Result<bool, MediaError> {
        let (rw, rh) = region_size(tex, w, h);
        let (ow, oh) = self.output_size(rw, rh);
        if output_too_small((ow, oh)) {
            // 창이 막 뜨거나 최소화될 때 오는 아주 작은 프레임, 상자에 맞추면 아주 납작해지는 창 등.
            if !self.tiny_frames {
                self.tiny_frames = true;
                tracing::info!(w = rw, h = rh, ow, oh, source = %self.source_id, "출력이 너무 작은 프레임은 건너뜁니다(마지막 화면 유지)");
            }
            return Ok(false);
        }
        if std::mem::take(&mut self.tiny_frames) {
            tracing::info!(w = rw, h = rh, "프레임 크기가 돌아왔습니다");
        }
        self.source.gfx_mut().1.ingest(tex, rw, rh)?;
        Ok(true)
    }

    /// 메일함에서 꺼낸 캡처 프레임을 보관하고 격자 시각으로 인코딩한다.
    fn on_wgc_frame(&mut self, mail: MailFrame) -> Result<(), MediaError> {
        let Source::Wgc { capture, .. } = &mut self.source else {
            return Ok(());
        };
        let MailFrame { frame: f, at, .. } = mail;
        let tex = f.texture()?;
        if let Some(c) = capture {
            c.follow_size(f.width, f.height);
        }
        let time = FrameTime {
            at,
            captured: f.qpc,
            arrival: f.arrival_qpc,
        };
        let kept = self.ingest(&tex, f.width, f.height)?;
        drop(tex);
        drop(f); // 프레임 풀에 바로 돌려준다.
        if !kept {
            return Ok(());
        }
        self.encode_current(time)
    }

    /// 보관한 마지막 화면을 다시 인코딩한다(멈춘 화면 키프레임, 비트레이트·설정을 바꾼 뒤). 시각은 격자에서 받는다
    /// ([`FrameGrid::claim`], D-111).
    ///
    /// 메일함에 새 프레임이 기다리면 하지 않는다: 그 프레임이 곧 키프레임으로 나가고, 그 프레임의 칸이 지금 칸보다 앞이라
    /// 다시 인코딩하면 시각이 거꾸로 간다. 이때는 키프레임 요청 시각도 둔다(그 프레임을 버리게 되면 다음 바퀴에 다시 본다).
    /// 보관한 화면이 없거나(첫 프레임부터 출력이 너무 작음, 다음 프레임이 키프레임으로 나간다) 테스트 패턴이면 하지 않는다.
    /// 호출자는 이번 바퀴에 꺼낸 프레임이 없을 때만 부른다.
    fn reencode_last(&mut self) -> Result<(), MediaError> {
        let kept = matches!(&self.source, Source::Wgc { converter, .. } if converter.source_size().is_some());
        let now = qpc_now_100ns();
        let at = {
            let mut m = lock(&self.shared.mail);
            if m.frame.is_some() {
                return Ok(());
            }
            self.key_requested_at = None;
            if !kept {
                return Ok(());
            }
            m.grid.claim(now)
        };
        self.encode_current(FrameTime {
            at,
            captured: now,
            arrival: now,
        })
    }

    /// 테스트 패턴: 격자의 다음 칸까지 자고, 그 칸의 시각으로 한 장 그려 인코딩한다(D-111, [`FrameGrid::claim`]).
    fn on_pattern_tick(&mut self) -> Result<(), MediaError> {
        let Source::Pattern {
            prev,
            index,
            white,
            gpu: g,
        } = &mut self.source
        else {
            return Ok(());
        };
        // 다음 칸까지 정밀하게 잔다(최대 10ms씩, 그 사이 메일 확인). 늦었으면 지난 칸은 건너뛴다(`claim`은 지금이 든 칸을 준다).
        let now = qpc_now_100ns();
        let t = {
            let mut m = lock(&self.shared.mail);
            match m.grid.next_due() {
                Some(due) if now < due => {
                    drop(m);
                    let left = Duration::from_nanos((due - now) as u64 * 100);
                    std::thread::sleep(left.min(Duration::from_millis(10)));
                    return Ok(());
                }
                _ => m.grid.claim(now),
            }
        };
        *white = pattern_is_white(*prev, t);
        *prev = Some(t);
        *index += 1;
        // WGC와 같은 길: 텍스처를 보관본으로 GPU 안에서 복사한다(`encode_current`가 변환한다).
        let (w, h) = g.pattern.size();
        let tex = if *white {
            g.pattern.white_frame()?
        } else {
            g.pattern.frame(*index)?
        }
        .clone();
        {
            let mut st = lock(&self.shared.stats);
            st.captured.push_back(Instant::now());
        }
        if !self.ingest(&tex, w, h)? {
            return Ok(());
        }
        self.encode_current(FrameTime {
            at: t,
            captured: t,
            arrival: now,
        })
    }

    /// 지금 원본(보관 프레임)을 인코딩한다.
    ///
    /// 인코더가 hw 입력(zero-copy)이면 풀에서 D3D11 NV12 텍스처를 받아 `convert_to_texture`로 변환(Blt)하고 그 텍스처를
    /// 인코더에 넣는다. 아니면 시스템 메모리 NV12 프레임에 변환(VP로 인코더 크기로 줄여 읽어 옴)해 넣는다.
    ///
    /// 출력 크기가 바뀌어 인코더를 다시 열지 못하면(D-109) 옛 인코더를 그대로 두고 이 프레임을 버린다(`Ok`). 같은 출력 크기로는
    /// 다시 열어 보지 않고, 출력 크기가 바뀌면 다시 열어 본다. 경고는 크기마다 한 번이다. 멈춘 화면 키프레임도 같은 규칙을 탄다.
    ///
    /// `time.at`(격자 시각)은 앞서 인코딩한 프레임보다 커야 한다(D-111). 격자가 낸 순서대로 부르면 지켜진다.
    fn encode_current(&mut self, time: FrameTime) -> Result<(), MediaError> {
        self.in_hw = false;
        let (sw, sh) = self.source_size();
        let (ow, oh) = self.output_size(sw, sh);
        if self.failed_output.is_some_and(|f| f != (ow, oh)) {
            self.failed_output = None;
        }
        if self.encoder_stale(ow, oh) {
            let resized = self
                .encoder
                .as_ref()
                .is_some_and(|e| (e.params.width, e.params.height) != (ow, oh));
            let params = self.params(ow, oh);
            if resized {
                // 출력 크기가 바뀌었다: 새 인코더를 연 뒤에만 옛 인코더를 바꾼다(열지 못하면 옛 인코더를 둔다).
                if self.failed_output == Some((ow, oh)) {
                    return Ok(());
                }
                match self.open_encoder(&params) {
                    Ok(enc) => {
                        tracing::info!(sw, sh, ow, oh, encoder = %enc.info.name, zero_copy = enc.is_hw_input(), "출력 크기 변경, 인코더 재생성");
                        self.set_encoder(enc);
                    }
                    Err(e) => {
                        self.failed_output = Some((ow, oh));
                        if self.warned_outputs.insert((ow, oh)) {
                            tracing::warn!(
                                sw,
                                sh,
                                ow,
                                oh,
                                "출력 크기가 바뀌어 인코더를 다시 열지 못했습니다. 옛 인코더를 두고 이 크기의 프레임은 버립니다: {e}"
                            );
                        }
                        return Ok(());
                    }
                }
            } else {
                // 인코더가 없거나 입력 방식(hw/시스템 메모리)·풀 세대가 바뀌었다: 지금처럼 옛 인코더를 버리고 연다.
                self.encoder = None;
                let enc = self.open_encoder(&params)?;
                tracing::info!(sw, sh, ow, oh, encoder = %enc.info.name, zero_copy = enc.is_hw_input(), "입력 방식 변경, 인코더 재생성");
                self.set_encoder(enc);
            }
            self.force_key = true;
            self.publish_encoder_stats(sw, sh);
        }
        let enc = self.encoder.as_mut().expect("위에서 열었다");
        let hw_input = enc.is_hw_input();
        let t0 = Instant::now();
        let convert_ms;
        let mut preview_failed: Option<String> = None;
        // 프레임마다 풀에서 새 버퍼(텍스처)를 받는다. 이전 프레임은 인코더가 아직 쥐고 있을 수 있어 다시 쓰지 않는다
        // (NVENC가 매핑한 동안 텍스처에 쓰면 안 된다).
        let mut frame = if hw_input {
            self.in_hw = true;
            let missing = |what: &str| MediaError::Encoder {
                encoder: enc.info.name.clone(),
                message: what.into(),
            };
            // 인코더가 hw 입력이면 그 크기의 풀이 있다(`encoder_stale`이 풀 세대를 본다).
            let frame = self
                .zc
                .pool_mut(enc.params.width, enc.params.height)
                .ok_or_else(|| missing("hw 프레임 풀이 없습니다"))?
                .get()?;
            let (_, conv) = self.source.gfx_mut();
            let (tex, slice) = frame
                .d3d11_texture()
                .ok_or_else(|| missing("hw 프레임에서 D3D11 텍스처를 얻지 못했습니다"))?;
            conv.convert_to_texture(&tex, slice)?;
            drop(tex);
            convert_ms = t0.elapsed().as_secs_f32() * 1000.0;
            // 화면이 미리 보기를 보고 있으면 간격마다 한 장. 보관한 원본을 GPU에서 작게 줄인 텍스처만 읽어 온다.
            if let Some((max_w, max_h)) = self.shared.preview.take_turn(Instant::now()) {
                // 요청은 가로·세로 상자다. 비율을 지켜 상자에 맞춘 크기의 긴 변으로 줄인다(`preview_rgba`는 긴 변 하나를 받는다).
                let max = conv.source_size().map_or(max_w.max(max_h), |(sw, sh)| {
                    let (w, h) = crate::preview::fit(sw, sh, max_w, max_h);
                    w.max(h)
                });
                match conv.preview_rgba(max) {
                    Ok(Some((w, h, rgba))) => self.shared.preview.put(w, h, rgba),
                    Ok(None) => {}
                    Err(e) => preview_failed = Some(e.to_string()),
                }
            }
            frame
        } else {
            // 읽어 오는 길: VP로 인코더 크기로 줄이고 NV12만 읽어 온다. 실패하면 `GpuConvert`로 방송을 멈춘다(D-105).
            let mut frame = enc.new_frame()?;
            {
                let mut view = frame.view_mut().ok_or_else(|| MediaError::Encoder {
                    encoder: enc.info.name.clone(),
                    message: "입력 프레임을 쓸 수 없습니다".into(),
                })?;
                self.source.gfx_mut().1.convert(&mut view)?;
                // 화면이 미리 보기를 보고 있으면 간격마다 한 장(시청자가 받는 것과 같은 프레임).
                if let Some(max) = self.shared.preview.take_turn(Instant::now()) {
                    match self.preview.scale(&view, max) {
                        Ok((w, h, rgba)) => self.shared.preview.put(w, h, rgba),
                        Err(e) => tracing::debug!("송출 화면 미리 보기 실패: {e}"),
                    }
                }
            }
            convert_ms = t0.elapsed().as_secs_f32() * 1000.0;
            frame
        };
        let key = self.force_key;
        let t1 = Instant::now();
        {
            let packets = &mut self.packets;
            enc.encode(&mut frame, time, key, &mut |p| packets.push(p))?;
        }
        drop(frame);
        let encode_ms = t1.elapsed().as_secs_f32() * 1000.0;
        if key {
            self.force_key = false;
            self.key_requested_at = None;
        }
        let (w, h) = (enc.params.width, enc.params.height);
        self.shared.zero_copy_path.store(hw_input, Ordering::SeqCst);
        for p in self.packets.drain(..) {
            let now_qpc = qpc_now_100ns();
            let latency = qpc_ms(now_qpc - p.qpc);
            let pipeline = qpc_ms(now_qpc - time.arrival);
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
                st.prune(now);
            }
            (self.on_frame)(EncodedVideo {
                data: p.data,
                keyframe: p.keyframe,
                capture_qpc_100ns: p.qpc,
                capture: instant_from_qpc(p.qpc),
                width: w,
                height: h,
            });
        }
        self.in_hw = false;
        if let Some(reason) = preview_failed {
            // 이 장치가 BGRA 비디오 출력을 지원하지 않는 등(hw 경로에는 시스템 메모리 YUV 프레임이 없어 swscale 미리 보기로 갈 수
            // 없다). 다음 프레임부터 읽어 오기 경로로 내려가 그 YUV 프레임으로 미리 보기를 만든다.
            self.zc_fall_back(format!("GPU 미리 보기를 만들지 못했습니다: {reason}"));
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

/// WGC 콜백: 프레임 시각 격자로 고르고(D-111), 고른 프레임을 격자 시각과 함께 최신 프레임 칸에 넣는다. 격자가 거른 프레임은
/// 세지 않고, 칸에 남아 있다 덮인 프레임은 `dropped_frames`로 센다(모듈 문서 "프레임을 버리는 곳과 세는 방식").
fn on_captured(shared: &Shared, generation: u64, frame: CapturedFrame) {
    let dropped = {
        let mut m = lock(&shared.mail);
        if m.stop {
            return;
        }
        let Some(at) = m.grid.accept(frame.qpc) else {
            return;
        };
        let dropped = m
            .frame
            .replace(MailFrame {
                generation,
                frame,
                at,
            })
            .is_some();
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

    const I60: f64 = QPC_PER_SEC as f64 / 60.0;

    /// 시각이 모두 격자(`base + floor(k·10^7/fps)`) 위에 있고 칸 번호가 엄격히 느는지 보고, 칸 번호를 돌려준다.
    fn cells_on_grid(times: &[i64], base: i64, fps: i64) -> Vec<i64> {
        let cells: Vec<i64> = times
            .iter()
            .map(|&t| {
                let k = ((t - base) * fps * 2 + QPC_PER_SEC).div_euclid(2 * QPC_PER_SEC);
                assert_eq!(
                    base + (k * QPC_PER_SEC).div_euclid(fps),
                    t,
                    "격자 밖의 시각 {t}"
                );
                k
            })
            .collect();
        assert!(
            cells.windows(2).all(|w| w[1] > w[0]),
            "칸이 늘지 않음: {cells:?}"
        );
        cells
    }

    /// 칸 간격(몇 칸 건너 골랐나)들.
    fn cell_gaps(cells: &[i64]) -> Vec<i64> {
        cells.windows(2).map(|w| w[1] - w[0]).collect()
    }

    /// 원본 프레임 시각 `qpcs`를 차례로 넣고 고른 시각.
    fn feed(grid: &mut FrameGrid, qpcs: impl IntoIterator<Item = i64>) -> Vec<i64> {
        qpcs.into_iter().filter_map(|q| grid.accept(q)).collect()
    }

    /// `hz` 고정 주사율 원본 `n`장(첫 프레임 `start`).
    fn steady(start: i64, hz: f64, n: usize) -> impl Iterator<Item = i64> {
        (0..n).map(move |i| start + (i as f64 * QPC_PER_SEC as f64 / hz) as i64)
    }

    /// 간격이 `lo`~`hi` fps 사이에서 무작위로 바뀌는 원본(VRR·가변 fps 게임). 시드가 고정이라 늘 같다.
    fn variable(start: i64, lo: f64, hi: f64, secs: f64, mut seed: u64) -> Vec<i64> {
        let mut out = Vec::new();
        let mut t = start;
        while t < start + (secs * QPC_PER_SEC as f64) as i64 {
            out.push(t);
            // xorshift64
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let u = (seed >> 11) as f64 / (1u64 << 53) as f64;
            let fps = lo + (hi - lo) * u;
            t += (QPC_PER_SEC as f64 / fps) as i64;
        }
        out
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

    /// 60Hz 원본: 60fps는 모두, 30fps는 칸마다 한 장. 고른 시각은 모두 격자 위이고 칸을 건너뛰지 않는다.
    #[test]
    fn grid_steady_60hz_source() {
        let start = 1_000_000_123;
        let mut g = FrameGrid::new(60, false);
        let got = feed(&mut g, steady(start, 60.0, 600));
        assert_eq!(got.len(), 600);
        assert_eq!(got[0], start, "격자 기준은 첫 프레임의 시각");
        assert!(
            cell_gaps(&cells_on_grid(&got, start, 60))
                .iter()
                .all(|&d| d == 1)
        );

        let mut g = FrameGrid::new(30, false);
        let got = feed(&mut g, steady(start, 60.0, 600));
        assert!((299..=301).contains(&got.len()), "{}", got.len());
        assert!(
            cell_gaps(&cells_on_grid(&got, start, 30))
                .iter()
                .all(|&d| d == 1)
        );

        // 59.94Hz 원본은 칸보다 조금 느리다: 약 8.3초(위상이 칸 반 개만큼 밀릴 때)에 한 번 한 칸을 건너뛴다.
        let mut g = FrameGrid::new(60, false);
        let got = feed(&mut g, steady(start, 60_000.0 / 1001.0, 600));
        let gaps = cell_gaps(&cells_on_grid(&got, start, 60));
        assert_eq!(got.len(), 600);
        assert_eq!(gaps.iter().filter(|&&d| d == 2).count(), 1, "{gaps:?}");
        assert!(gaps.iter().all(|&d| d == 1 || d == 2));
    }

    /// 144Hz 원본 → 60fps: 칸마다 한 장(약 600장/10초), 간격은 늘 한 칸이다. 옛 fps 제한기(`Pacer`)는 원본 캡처 시각을 그대로
    /// 써서 13.9·20.8ms 간격이 섞였다.
    #[test]
    fn grid_144hz_source_gives_one_frame_per_cell() {
        let start = 7 * QPC_PER_SEC + 4_321;
        let mut g = FrameGrid::new(60, false);
        let got = feed(&mut g, steady(start, 144.0, 1440));
        assert!((599..=601).contains(&got.len()), "{}", got.len());
        let gaps = cell_gaps(&cells_on_grid(&got, start, 60));
        assert!(gaps.iter().all(|&d| d == 1), "{gaps:?}");
        // 시각 간격은 1/60초(100ns 내림 때문에 166666 또는 166667)뿐이다.
        assert!(
            got.windows(2)
                .all(|w| (w[1] - w[0] - I60 as i64).abs() <= 1)
        );
    }

    /// 80~110fps 가변 원본(무작위, 시드 고정): 간격이 한 칸보다 짧아 칸마다 한 장이고, 시각은 모두 격자 위다.
    #[test]
    fn grid_variable_80_to_110fps_source() {
        for (seed, fps) in [(0x9E37_79B9_7F4A_7C15, 60), (0x2545_F491_4F6C_DD1D, 30)] {
            let start = 3 * QPC_PER_SEC + 77;
            let src = variable(start, 80.0, 110.0, 10.0, seed);
            let mut g = FrameGrid::new(fps, false);
            let got = feed(&mut g, src.iter().copied());
            let want = 10 * fps as usize;
            assert!(
                (want - 2..=want + 1).contains(&got.len()),
                "fps {fps}: {}",
                got.len()
            );
            let gaps = cell_gaps(&cells_on_grid(&got, start, fps as i64));
            assert!(gaps.iter().all(|&d| d == 1), "fps {fps}: {gaps:?}");
            // 고른 시각과 원본 캡처 시각의 차이는 칸 반 개 안쪽이다(D-111의 대가).
            let half = QPC_PER_SEC / fps as i64 / 2 + 1;
            for &t in &got {
                let nearest = src.iter().map(|&q| (q - t).abs()).min().unwrap();
                assert!(nearest <= half, "fps {fps}: {t} 근처 원본 없음({nearest})");
            }
        }
    }

    /// 원본이 더 느리면(24Hz → 60fps) 모두 고르고, 새 프레임이 없는 칸은 건너뛴다(반복 프레임을 만들지 않는다).
    #[test]
    fn grid_slower_source_skips_empty_cells() {
        let start = 1_000_000_000;
        let mut g = FrameGrid::new(60, false);
        let got = feed(&mut g, steady(start, 24.0, 240));
        assert_eq!(got.len(), 240);
        let gaps = cell_gaps(&cells_on_grid(&got, start, 60));
        assert!(gaps.iter().all(|&d| d == 2 || d == 3), "{gaps:?}");
    }

    /// 화면이 멈췄다가 다시 움직이면 곧바로 받고, 몰아 받지 않는다(칸마다 한 장).
    #[test]
    fn grid_accepts_after_idle_without_burst() {
        let mut g = FrameGrid::new(60, false);
        assert_eq!(g.accept(0), Some(0));
        let base = 5 * QPC_PER_SEC + 12_345;
        let got = feed(&mut g, steady(base, 144.0, 144));
        // 정지 뒤 첫 프레임은 바로 받는다(그 프레임이 든 칸의 시각, 칸 반 개 안쪽).
        assert!((got[0] - base).abs() <= I60 as i64 / 2 + 1, "{}", got[0]);
        let mut all = vec![0];
        all.extend(&got);
        let cells = cells_on_grid(&all, 0, 60);
        assert!(cell_gaps(&cells[1..]).iter().all(|&d| d == 1));
        assert!((59..=61).contains(&got.len()), "{}", got.len());
    }

    /// fps를 바꾸면 격자를 새로 잡는다. 시각은 바꾼 뒤에도 엄격히 늘고, 새 격자의 간격을 따른다.
    #[test]
    fn grid_fps_change_keeps_times_increasing() {
        let start = 2 * QPC_PER_SEC;
        let src: Vec<i64> = steady(start, 144.0, 144 * 6).collect();
        let mut g = FrameGrid::new(60, false);
        let mut segments: Vec<(i64, i64, Vec<i64>)> = Vec::new();
        for (i, chunk) in src.chunks(288).enumerate() {
            let fps = if i == 1 { 30 } else { 60 };
            g.set_fps(fps);
            let got = feed(&mut g, chunk.iter().copied());
            segments.push((i64::from(fps), g.base.unwrap(), got));
        }
        let all: Vec<i64> = segments.iter().flat_map(|(_, _, s)| s.clone()).collect();
        assert!(all.windows(2).all(|w| w[1] > w[0]), "{all:?}");
        for (fps, base, seg) in &segments {
            // 각 구간은 그 구간에서 새로 잡은 격자 위에 있고 칸을 건너뛰지 않는다.
            let gaps = cell_gaps(&cells_on_grid(seg, *base, *fps));
            assert!(gaps.iter().all(|&d| d == 1), "fps {fps}: {gaps:?}");
            assert!(
                (seg.len() as i64 - 2 * fps).abs() <= 2,
                "fps {fps}: {}",
                seg.len()
            );
        }

        // 다시 인코딩이 칸을 앞서 잡아(지금보다 뒤 시각) 둔 상태에서 fps를 바꿔도 거꾸로 가지 않는다.
        let mut g = FrameGrid::new(60, false);
        let a = g.accept(start).unwrap();
        let b = g.claim(start + 1); // 칸 0은 썼다 → 칸 1(지금보다 뒤)
        assert!(b > start + 1);
        g.set_fps(30);
        assert_eq!(g.next_due(), None, "새 격자는 아직 없다");
        let c = g.accept(start + 2);
        assert!(c.is_none_or(|c| c > b), "{c:?}");
        let d = g.claim(start + 3);
        assert!(a < b && b < d && c.is_none_or(|c| c < d));
    }

    /// `claim`(멈춘 화면 키프레임·테스트 패턴)은 캡처 프레임과 같은 격자에서 칸을 받고, 시각은 늘 엄격히 는다.
    #[test]
    fn grid_claim_is_monotonic_and_shares_cells_with_frames() {
        let base = 10 * QPC_PER_SEC;
        let i = I60 as i64;
        let mut g = FrameGrid::new(60, false);
        assert_eq!(g.accept(base), Some(base));
        // 같은 순간에 다시 내면 다음 칸이다.
        let c1 = g.claim(base);
        let c2 = g.claim(base);
        assert_eq!(cells_on_grid(&[base, c1, c2], base, 60), vec![0, 1, 2]);
        // 다시 인코딩이 쓴 칸(1, 2)에 든 캡처 프레임은 버리고, 그다음 칸 프레임은 받는다.
        assert_eq!(g.accept(base + i * 12 / 10), None);
        assert_eq!(g.accept(base + i * 22 / 10), None);
        let f3 = g.accept(base + i * 3 + 10).unwrap();
        assert_eq!(cells_on_grid(&[f3], base, 60), vec![3]);
        // 마지막 칸보다 이른 시각으로 불러도 시각은 는다.
        let c4 = g.claim(base);
        assert!(c4 > f3);
        // 한참 뒤(화면이 멈춘 뒤 키프레임 요청)면 지금이 든 칸이다(사이 칸은 건너뜀).
        let later = base + 5 * QPC_PER_SEC + 3;
        let c5 = g.claim(later);
        assert_eq!(c5, base + 5 * QPC_PER_SEC);
        assert_eq!(g.next_due(), Some(base + 5 * QPC_PER_SEC + 166_666));
        // 섞어 내도 모두 격자 위에서 엄격히 는다.
        cells_on_grid(&[base, c1, c2, f3, c4, c5], base, 60);
    }

    /// 정초 격자(테스트 패턴): 매 초 첫 칸이 정확히 정초다. 30·60fps 모두 1초에 fps칸이고, 늦게 깨면 지난 칸을 건너뛴다.
    #[test]
    fn whole_second_grid_puts_a_cell_on_every_second() {
        let mut g = FrameGrid::new(60, true);
        assert_eq!(g.next_due(), None);
        // 정초 바로 뒤에 시작해도 칸은 정초에 맞춘다.
        let first = g.claim(5 * QPC_PER_SEC + 3);
        assert_eq!(first, 5 * QPC_PER_SEC);
        let mut ticks = vec![first];
        for _ in 0..60 {
            let due = g.next_due().unwrap();
            ticks.push(g.claim(due));
        }
        assert_eq!(ticks[1], 5 * QPC_PER_SEC + 166_666);
        assert_eq!(ticks[60], 6 * QPC_PER_SEC);
        cells_on_grid(&ticks, 5 * QPC_PER_SEC, 60);
        // 늦게 깨면(칸 반 개 넘게) 지난 칸은 건너뛴다.
        let due = g.next_due().unwrap();
        let late = g.claim(due + (I60 * 0.7) as i64);
        assert_eq!(late, 6 * QPC_PER_SEC + (2 * QPC_PER_SEC) / 60);
        // 30fps로 바꾸면 새 격자도 정초에 맞고, 1초에 30칸이다.
        g.set_fps(30);
        let t = g.claim(late + 10);
        assert!(t > late);
        let mut n = 0;
        let mut t = t;
        while t < 7 * QPC_PER_SEC {
            t = g.claim(g.next_due().unwrap());
            n += 1;
        }
        assert_eq!(t, 7 * QPC_PER_SEC, "30fps 격자도 정초에 칸이 있다");
        let mut n2 = 0;
        while t < 8 * QPC_PER_SEC {
            t = g.claim(g.next_due().unwrap());
            n2 += 1;
        }
        assert!(n > 0);
        assert_eq!(n2, 30);
        assert_eq!(t, 8 * QPC_PER_SEC);
    }
}
