//! 시험용 미디어: 페이로드에 표시를 넣어 시청 쪽에서 어느 방송의 어느 프레임인지 알아본다.

use std::time::{Duration, Instant};

use dongsi_core::VideoCodec;
use dongsi_core::proto::PublishInfo;
use dongsi_net::publish::Publisher;

const MAGIC: &[u8; 3] = b"DSK";

/// 페이로드: `DSK` + tick(4바이트) + 방송자 표시(4바이트) + 채움.
pub fn payload(index: u32, tag: u32, len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len + 11);
    v.extend_from_slice(MAGIC);
    v.extend_from_slice(&index.to_le_bytes());
    v.extend_from_slice(&tag.to_le_bytes());
    v.resize(len, 0xAA);
    v
}

/// (tick, 방송자 표시)
pub fn find_marker(data: &[u8]) -> Option<(u32, u32)> {
    let pos = data.windows(3).position(|w| w == MAGIC)?;
    let b = data.get(pos + 3..pos + 11)?;
    Some((
        u32::from_le_bytes(b[..4].try_into().ok()?),
        u32::from_le_bytes(b[4..].try_into().ok()?),
    ))
}

/// 시험용 비디오 프레임. H.264는 Annex B(SPS, PPS, IDR). 모든 프레임이 키프레임이다.
pub fn video_frame(codec: VideoCodec, index: u32, tag: u32) -> Vec<u8> {
    match codec {
        VideoCodec::H264 => {
            let mut v = vec![0, 0, 0, 1, 0x67, 0x64, 0x00, 0x1f, 0xac, 0xd9, 0x40];
            v.extend_from_slice(&[0, 0, 0, 1, 0x68, 0xeb, 0xe3, 0xcb]);
            v.extend_from_slice(&[0, 0, 0, 1, 0x65]);
            v.extend_from_slice(&payload(index, tag, 3000));
            v
        }
        // VP8: 첫 바이트의 0번 비트가 0이면 키프레임
        _ => {
            let mut v = vec![0x10, 0x02, 0x00, 0x9d, 0x01, 0x2a];
            v.extend_from_slice(&payload(index, tag, 3000));
            v
        }
    }
}

pub fn info(codec: VideoCodec) -> PublishInfo {
    PublishInfo {
        codec,
        width: 1920,
        height: 1080,
        fps: 30,
        has_audio: true,
    }
}

/// 방송자 쪽 미디어 펌프: 20ms마다 오디오, 40ms마다 비디오(같은 캡처 시각).
///
/// tick은 처음 시작한 때부터 실제 시간과 같이 흐른다(20ms당 1). 시험이 쉬었다가 이어서 부르면 쉰
/// 시간만큼 tick을 건너뛴다. 캡처 시각과 RTP 시각이 같이 흘러야 SR(A/V 동기)이 어긋나지 않는다.
pub struct Pump {
    tag: u32,
    tick: u32,
    origin: Option<Instant>,
}

impl Pump {
    pub fn new(tag: u32) -> Self {
        Self {
            tag,
            tick: 0,
            origin: None,
        }
    }

    /// `ticks`번(20ms씩) 보낸다. 보낸 비디오 프레임 수를 돌려준다.
    /// `codec_at`·`on_tick`은 이번 호출에서 몇 번째 tick인지(0부터)를 받는다.
    pub async fn run(
        &mut self,
        publisher: &Publisher,
        ticks: u32,
        codec_at: impl Fn(u32) -> VideoCodec,
        mut on_tick: impl FnMut(u32),
    ) -> usize {
        let origin = *self.origin.get_or_insert_with(Instant::now);
        let elapsed = (origin.elapsed().as_millis() / 20) as u32;
        if elapsed > self.tick {
            self.tick = elapsed + 1;
        }
        let mut videos = 0;
        for i in 0..ticks {
            let tick = self.tick;
            let at = origin + Duration::from_millis(20 * u64::from(tick));
            let now = Instant::now();
            if at > now {
                tokio::time::sleep(at - now).await;
            }
            on_tick(i);
            publisher.send_audio(payload(tick, self.tag, 80), at, u64::from(tick) * 960);
            if tick.is_multiple_of(2) {
                let codec = codec_at(i);
                publisher.send_video(codec, video_frame(codec, tick, self.tag), at);
                videos += 1;
            }
            self.tick += 1;
        }
        videos
    }

    /// VP8로 `ticks`번 보낸다.
    pub async fn vp8(&mut self, publisher: &Publisher, ticks: u32) -> usize {
        self.run(publisher, ticks, |_| VideoCodec::Vp8, |_| {})
            .await
    }
}
