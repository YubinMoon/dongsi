//! WASAPI 루프백 캡처(장치 루프백, 프로세스 루프백 INCLUDE/EXCLUDE).

use std::collections::VecDeque;

use wasapi::{
    AudioCaptureClient, AudioClient, DeviceEnumerator, Direction, SampleType, StreamMode,
    WaveFormat,
};

use super::timeline::{CHANNELS, SAMPLE_RATE};
use crate::MediaError;

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

pub(crate) struct LoopbackCapture {
    client: AudioClient,
    capture: AudioCaptureClient,
    pub target: Target,
    bytes: VecDeque<u8>,
    samples: Vec<f32>,
}

impl LoopbackCapture {
    /// 48kHz 스테레오 float32로 루프백 캡처를 연다(형식 자동 변환 사용).
    pub fn open(target: Target) -> Result<Self, MediaError> {
        let format = WaveFormat::new(
            32,
            32,
            SampleType::Float,
            SAMPLE_RATE as usize,
            CHANNELS,
            None,
        );
        let (mut client, target) = match target {
            Target::Device(_) => {
                let e = DeviceEnumerator::new().map_err(|e| audio_err("장치 열거", e))?;
                let d = e
                    .get_default_device(Direction::Render)
                    .map_err(|e| audio_err("기본 출력 장치", e))?;
                let id = d.get_id().map_err(|e| audio_err("장치 ID", e))?;
                let c = d
                    .get_iaudioclient()
                    .map_err(|e| audio_err("IAudioClient", e))?;
                (c, Target::Device(id))
            }
            Target::Exclude(pid) => (
                AudioClient::new_application_loopback_client(pid, false)
                    .map_err(|e| audio_err("프로세스 루프백(EXCLUDE)", e))?,
                Target::Exclude(pid),
            ),
            Target::Include(pid) => (
                AudioClient::new_application_loopback_client(pid, true)
                    .map_err(|e| audio_err("프로세스 루프백(INCLUDE)", e))?,
                Target::Include(pid),
            ),
        };
        let mode = StreamMode::PollingShared {
            autoconvert: true,
            buffer_duration_hns: 2_000_000, // 200ms 버퍼(폴링이 늦어도 넘치지 않게)
        };
        client
            .initialize_client(&format, Direction::Capture, mode, None)
            .map_err(|e| audio_err("초기화", e))?;
        let capture = client
            .get_audiocaptureclient()
            .map_err(|e| audio_err("IAudioCaptureClient", e))?;
        client.start_stream().map_err(|e| audio_err("시작", e))?;
        Ok(LoopbackCapture {
            client,
            capture,
            target,
            bytes: VecDeque::new(),
            samples: Vec::new(),
        })
    }

    /// 쌓인 버퍼를 모두 읽는다. `sink(첫 샘플의 QPC, 스테레오 샘플)`. 읽은 샘플 수(채널당).
    pub fn read(&mut self, mut sink: impl FnMut(Option<i64>, &[f32])) -> Result<usize, MediaError> {
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
