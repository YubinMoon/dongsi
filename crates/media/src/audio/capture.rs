//! WASAPI 루프백 캡처(장치 루프백, 프로세스 루프백 INCLUDE/EXCLUDE).
//!
//! 이벤트 방식(`AUDCLNT_STREAMFLAGS_EVENTCALLBACK`)으로 연다(D-080). 엔진이 버퍼를 채우면 바로 깨어나 읽으므로
//! 폴링 간격만큼 늦지 않는다. 이벤트 방식으로 열지 못하면 예전처럼 폴링으로 연다.
//!
//! 장치 루프백은 재생 중인 소리가 없으면 데이터를 주지 않는다. 그러면 파이프라인이 무음을 늦게(`SILENCE_LAG_QPC`) 채워서
//! 소리가 멈추고 시작할 때마다 송출 시각이 계단처럼 뛴다(실측: 렌더 스트림이 있으면 캡처 시각→송출 33ms, 없으면 89ms).
//! 수신 쪽 지터 버퍼(NetEq)는 이 계단을 도착 흔들림으로 보고 목표 지연을 올린다. 그래서 장치 루프백일 때는 같은 장치에
//! 무음 렌더 스트림([`SilentRender`])을 함께 열어 루프백이 늘 흐르게 한다(Chromium의 루프백 입력과 같은 방식).
//! 프로세스 루프백은 무음이어도 데이터가 계속 와서(실측) 필요 없다.

use std::collections::VecDeque;
use std::time::Duration;

use wasapi::{
    AudioCaptureClient, AudioClient, AudioRenderClient, BufferFlags, Device, DeviceEnumerator,
    Direction, Handle, SampleType, StreamMode, WaveFormat,
};

use super::timeline::{CHANNELS, SAMPLE_RATE};
use crate::MediaError;

/// 캡처 버퍼 크기(100ns). 공유 모드에서는 용량일 뿐이라 제때 비우면 지연이 늘지 않는다. 고리가 늦어도 넘치지 않게 넉넉히 둔다.
const CAPTURE_BUFFER_HNS: i64 = 2_000_000; // 200ms
/// 무음 렌더 스트림의 버퍼 크기(100ns). 캡처 고리가 채우므로 고리가 이만큼 멈춰도 비지 않는다.
const SILENT_RENDER_BUFFER_HNS: i64 = 1_000_000; // 100ms

/// 무엇을 캡처할지.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// 기본 출력 장치 전체 루프백. 값은 장치 ID(기본 장치 변경 감지용).
    Device(String),
    /// 이 프로세스 트리만 빼고 시스템 전체(프로세스 루프백 EXCLUDE).
    Exclude(u32),
    /// 이 프로세스 트리만(프로세스 루프백 INCLUDE).
    Include(u32),
}

fn audio_err(context: &str, e: impl std::fmt::Display) -> MediaError {
    MediaError::Audio(format!("{context}: {e}"))
}

/// 기본 출력(렌더) 장치 ID.
pub(crate) fn default_render_device_id() -> Result<String, MediaError> {
    let e = DeviceEnumerator::new().map_err(|e| audio_err("장치 열거", e))?;
    let d = e
        .get_default_device(Direction::Render)
        .map_err(|e| audio_err("기본 출력 장치", e))?;
    d.get_id().map_err(|e| audio_err("장치 ID", e))
}

/// 48kHz 스테레오 float32(캡처·무음 렌더 공통, 형식 자동 변환을 쓴다).
fn stream_format() -> WaveFormat {
    WaveFormat::new(
        32,
        32,
        SampleType::Float,
        SAMPLE_RATE as usize,
        CHANNELS,
        None,
    )
}

pub(crate) struct LoopbackCapture {
    client: AudioClient,
    capture: AudioCaptureClient,
    /// 이벤트 방식이면 엔진이 캡처 버퍼를 채울 때 신호가 오는 핸들. 폴링으로 열었으면 `None`.
    event: Option<Handle>,
    /// 장치 루프백을 늘 흐르게 하는 무음 렌더 스트림(장치 루프백일 때만, 열지 못했으면 `None`).
    keepalive: Option<SilentRender>,
    pub target: Target,
    bytes: VecDeque<u8>,
    samples: Vec<f32>,
}

impl LoopbackCapture {
    /// 48kHz 스테레오 float32로 루프백 캡처를 연다. 이벤트 방식을 먼저 시도하고, 안 되면 폴링으로 연다.
    pub fn open(target: Target) -> Result<Self, MediaError> {
        match Self::open_with(&target, true) {
            Ok(c) => Ok(c),
            Err(e) => {
                tracing::warn!("이벤트 방식 루프백을 열지 못해 폴링으로 엽니다: {e}");
                Self::open_with(&target, false)
            }
        }
    }

    fn open_with(target: &Target, events: bool) -> Result<Self, MediaError> {
        let format = stream_format();
        let (mut client, target, device) = match target {
            Target::Device(_) => {
                let e = DeviceEnumerator::new().map_err(|e| audio_err("장치 열거", e))?;
                let d = e
                    .get_default_device(Direction::Render)
                    .map_err(|e| audio_err("기본 출력 장치", e))?;
                let id = d.get_id().map_err(|e| audio_err("장치 ID", e))?;
                let c = d
                    .get_iaudioclient()
                    .map_err(|e| audio_err("IAudioClient", e))?;
                (c, Target::Device(id), Some(d))
            }
            Target::Exclude(pid) => (
                AudioClient::new_application_loopback_client(*pid, false)
                    .map_err(|e| audio_err("프로세스 루프백(EXCLUDE)", e))?,
                Target::Exclude(*pid),
                None,
            ),
            Target::Include(pid) => (
                AudioClient::new_application_loopback_client(*pid, true)
                    .map_err(|e| audio_err("프로세스 루프백(INCLUDE)", e))?,
                Target::Include(*pid),
                None,
            ),
        };
        let mode = if events {
            StreamMode::EventsShared {
                autoconvert: true,
                buffer_duration_hns: CAPTURE_BUFFER_HNS,
            }
        } else {
            StreamMode::PollingShared {
                autoconvert: true,
                buffer_duration_hns: CAPTURE_BUFFER_HNS,
            }
        };
        client
            .initialize_client(&format, Direction::Capture, mode, None)
            .map_err(|e| audio_err("초기화", e))?;
        let event = if events {
            Some(
                client
                    .set_get_eventhandle()
                    .map_err(|e| audio_err("이벤트 핸들", e))?,
            )
        } else {
            None
        };
        let capture = client
            .get_audiocaptureclient()
            .map_err(|e| audio_err("IAudioCaptureClient", e))?;
        // 무음 렌더는 캡처보다 먼저 켜서 첫 버퍼부터 흐르게 한다. 못 열어도 캡처는 한다(예전처럼 무음 구간은 늦게 채움).
        let keepalive = device.and_then(|d| match SilentRender::open(&d, &format) {
            Ok(k) => Some(k),
            Err(e) => {
                tracing::warn!("장치 루프백용 무음 렌더 스트림을 열지 못했습니다: {e}");
                None
            }
        });
        client.start_stream().map_err(|e| audio_err("시작", e))?;
        tracing::info!(
            ?target,
            events,
            keepalive = keepalive.is_some(),
            "루프백 캡처 열림"
        );
        Ok(LoopbackCapture {
            client,
            capture,
            event,
            keepalive,
            target,
            bytes: VecDeque::new(),
            samples: Vec::new(),
        })
    }

    /// 이벤트 방식으로 열렸는가.
    pub fn has_event(&self) -> bool {
        self.event.is_some()
    }

    /// 엔진이 캡처 버퍼를 채울 때까지 최대 `timeout`만큼 기다린다. 이벤트 방식이 아니면 `timeout`만큼 잔다.
    pub fn wait(&self, timeout: Duration) {
        match &self.event {
            // 시간 초과도 정상이다(소리가 없거나 고리를 돌릴 때가 됨).
            Some(ev) => {
                let _ = ev.wait_for_event(timeout.as_millis().max(1) as u32);
            }
            None => std::thread::sleep(timeout),
        }
    }

    /// 쌓인 버퍼를 모두 읽는다. `sink(첫 샘플의 QPC, 스테레오 샘플)`. 읽은 샘플 수(채널당).
    pub fn read(&mut self, mut sink: impl FnMut(Option<i64>, &[f32])) -> Result<usize, MediaError> {
        if let Some(k) = &mut self.keepalive
            && let Err(e) = k.fill()
        {
            // 장치가 빠졌으면 캡처도 곧 실패해서 다시 열린다. 무음 렌더만 실패했으면 예전처럼 이어 간다.
            tracing::warn!("무음 렌더 스트림 오류, 끕니다: {e}");
            self.keepalive = None;
        }
        let mut total = 0;
        loop {
            let n = self
                .capture
                .get_next_packet_size()
                .map_err(|e| audio_err("다음 패킷 크기", e))?
                .unwrap_or(0);
            if n == 0 {
                return Ok(total);
            }
            self.bytes.clear();
            let info = self
                .capture
                .read_from_device_to_deque(&mut self.bytes)
                .map_err(|e| audio_err("읽기", e))?;
            let raw = self.bytes.make_contiguous();
            self.samples.clear();
            self.samples.extend(
                raw.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            );
            let qpc = (!info.flags.timestamp_error && info.timestamp != 0)
                .then_some(info.timestamp as i64);
            total += self.samples.len() / CHANNELS;
            sink(qpc, &self.samples);
        }
    }
}

impl Drop for LoopbackCapture {
    fn drop(&mut self) {
        let _ = self.client.stop_stream();
    }
}

/// 장치 루프백을 늘 흐르게 하는 무음 렌더 스트림. 0만 쓰고 `AUDCLNT_BUFFERFLAGS_SILENT`를 달아서 들리지 않는다.
/// 볼륨 믹서에는 이 앱의 세션으로 보인다.
struct SilentRender {
    client: AudioClient,
    render: AudioRenderClient,
    block_align: usize,
    zeros: Vec<u8>,
}

impl SilentRender {
    fn open(device: &Device, format: &WaveFormat) -> Result<Self, MediaError> {
        let mut client = device
            .get_iaudioclient()
            .map_err(|e| audio_err("무음 렌더 IAudioClient", e))?;
        client
            .initialize_client(
                format,
                Direction::Render,
                StreamMode::PollingShared {
                    autoconvert: true,
                    buffer_duration_hns: SILENT_RENDER_BUFFER_HNS,
                },
                None,
            )
            .map_err(|e| audio_err("무음 렌더 초기화", e))?;
        let render = client
            .get_audiorenderclient()
            .map_err(|e| audio_err("IAudioRenderClient", e))?;
        let mut s = SilentRender {
            client,
            render,
            block_align: format.get_blockalign() as usize,
            zeros: Vec::new(),
        };
        s.fill()?;
        s.client
            .start_stream()
            .map_err(|e| audio_err("무음 렌더 시작", e))?;
        Ok(s)
    }

    /// 렌더 버퍼의 빈자리를 무음으로 채운다.
    fn fill(&mut self) -> Result<(), MediaError> {
        let frames = self
            .client
            .get_available_space_in_frames()
            .map_err(|e| audio_err("무음 렌더 빈자리", e))? as usize;
        if frames == 0 {
            return Ok(());
        }
        let n = frames * self.block_align;
        if self.zeros.len() < n {
            self.zeros.resize(n, 0);
        }
        let silent = BufferFlags {
            data_discontinuity: false,
            silent: true,
            timestamp_error: false,
        };
        self.render
            .write_to_device(&self.zeros[..n], Some(silent))
            .map_err(|e| audio_err("무음 렌더 쓰기", e))
    }
}

impl Drop for SilentRender {
    fn drop(&mut self) {
        let _ = self.client.stop_stream();
    }
}
