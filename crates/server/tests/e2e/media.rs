//! 시험용 미디어: 페이로드에 표시를 넣어 시청 쪽에서 어느 방송의 어느 프레임인지 알아본다.
//!
//! 비디오는 H.264만이다(D-103). SFU는 디코딩하지 않으므로 화면 내용은 없고, 패킷화·키프레임 판단이 실제와
//! 같도록 형식만 맞춘다.
//!
//! - Annex B. 키프레임은 SPS(7)·PPS(8)·IDR(5), 아닌 것은 non-IDR 슬라이스(1) 하나다. str0m은 IDR이 있으면
//!   키프레임으로 본다. 슬라이스 내용(표시 + 채움)에는 0 바이트가 없어서 시작 코드(`00 00 01`)로 잘못 나뉘지 않는다
//!   (실제 인코더는 emulation prevention으로 막는 것).

use std::time::{Duration, Instant};

use dongsi_core::proto::PublishInfo;
use dongsi_net::publish::Publisher;

const MAGIC: &[u8; 3] = b"DSK";
/// 표시 길이: `DSK` + tick(16진 8자) + 방송자 표시(16진 8자).
const MARKER_LEN: usize = 3 + 16;
/// 비디오 프레임 페이로드 길이(H.264는 RTP 패킷 3개쯤으로 나뉜다).
const VIDEO_PAYLOAD: usize = 3000;

/// 페이로드: `DSK` + tick(16진 8자) + 방송자 표시(16진 8자) + 채움(0xAA). 0 바이트가 없다.
pub fn payload(index: u32, tag: u32, len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len.max(MARKER_LEN));
    v.extend_from_slice(MAGIC);
    v.extend_from_slice(format!("{index:08x}{tag:08x}").as_bytes());
    v.resize(len.max(MARKER_LEN), 0xAA);
    v
}

/// (tick, 방송자 표시)
pub fn find_marker(data: &[u8]) -> Option<(u32, u32)> {
    let pos = data.windows(3).position(|w| w == MAGIC)?;
    let hex = std::str::from_utf8(data.get(pos + 3..pos + MARKER_LEN)?).ok()?;
    Some((
        u32::from_str_radix(&hex[..8], 16).ok()?,
        u32::from_str_radix(&hex[8..], 16).ok()?,
    ))
}

/// H.264 SPS(High, 레벨 3.1)·PPS. 디코딩하지 않으므로 형식만 맞춘 값이다.
const H264_SPS: &[u8] = &[0x67, 0x64, 0x00, 0x1f, 0xac, 0xd9, 0x40];
const H264_PPS: &[u8] = &[0x68, 0xeb, 0xe3, 0xcb];

fn annexb(out: &mut Vec<u8>, nal: &[u8]) {
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(nal);
}

/// 시험용 키프레임(SPS·PPS·IDR).
pub fn video_frame(index: u32, tag: u32) -> Vec<u8> {
    let body = payload(index, tag, VIDEO_PAYLOAD);
    let mut v = Vec::with_capacity(VIDEO_PAYLOAD + 32);
    annexb(&mut v, H264_SPS);
    annexb(&mut v, H264_PPS);
    let mut idr = vec![0x65];
    idr.extend_from_slice(&body);
    annexb(&mut v, &idr);
    v
}

/// 시험용 비디오 프레임 가운데 키프레임이 아닌 것(non-IDR 슬라이스).
pub fn delta_frame(index: u32, tag: u32) -> Vec<u8> {
    let body = payload(index, tag, VIDEO_PAYLOAD);
    let mut v = Vec::with_capacity(VIDEO_PAYLOAD + 8);
    let mut slice = vec![0x41];
    slice.extend_from_slice(&body);
    annexb(&mut v, &slice);
    v
}

pub fn info() -> PublishInfo {
    PublishInfo {
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
/// 한 번에 어떤 트랙을 보낼지.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tracks {
    Both,
    AudioOnly,
    VideoOnly,
}

pub struct Pump {
    tag: u32,
    tick: u32,
    origin: Option<Instant>,
    /// false면 비디오를 키프레임이 아닌 프레임으로만 보낸다.
    keyframes: bool,
}

impl Pump {
    pub fn new(tag: u32) -> Self {
        Self {
            tag,
            tick: 0,
            origin: None,
            keyframes: true,
        }
    }

    /// 처음 시작한 때(tick 0의 시각). 아직 안 보냈으면 `None`.
    pub fn origin(&self) -> Option<Instant> {
        self.origin
    }

    /// 보내는 비디오가 키프레임인지(기본 true). false면 키프레임 없이 델타 프레임만 보낸다.
    pub fn set_keyframes(&mut self, on: bool) {
        self.keyframes = on;
    }

    /// `ticks`번(20ms씩) 보낸다. 보낸 비디오 프레임 수를 돌려준다.
    /// `on_tick`은 이번 호출에서 몇 번째 tick인지(0부터)를 받는다.
    pub async fn run(
        &mut self,
        publisher: &Publisher,
        ticks: u32,
        on_tick: impl FnMut(u32),
    ) -> usize {
        self.run_tracks(publisher, ticks, Tracks::Both, on_tick)
            .await
    }

    /// `run`과 같고, `tracks`로 정한 트랙만 보낸다(tick은 그대로 흐른다).
    pub async fn run_tracks(
        &mut self,
        publisher: &Publisher,
        ticks: u32,
        tracks: Tracks,
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
            if tracks != Tracks::VideoOnly {
                publisher.send_audio(payload(tick, self.tag, 80), at, u64::from(tick) * 960);
            }
            if tracks != Tracks::AudioOnly && tick.is_multiple_of(2) {
                let frame = if self.keyframes {
                    video_frame(tick, self.tag)
                } else {
                    delta_frame(tick, self.tag)
                };
                publisher.send_video(frame, at);
                videos += 1;
            }
            self.tick += 1;
        }
        videos
    }

    /// `ticks`번 보낸다(`on_tick` 없는 [`run`](Self::run)).
    pub async fn h264(&mut self, publisher: &Publisher, ticks: u32) -> usize {
        self.run(publisher, ticks, |_| {}).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_roundtrip_and_no_start_code_emulation() {
        for (i, t) in [(0, 0), (256, 1), (0x0001_0000, 0x0100), (u32::MAX, 7)] {
            let p = payload(i, t, 80);
            assert_eq!(find_marker(&p), Some((i, t)));
            assert!(!p.contains(&0), "페이로드에 0 바이트가 없다");
        }
        // H.264 키프레임은 NAL 세 개(SPS, PPS, IDR), 델타는 non-IDR 하나로 나뉜다.
        let key = video_frame(256, 1);
        let starts = key.windows(3).filter(|w| *w == [0, 0, 1]).count();
        assert_eq!(starts, 3);
        assert_eq!(find_marker(&key), Some((256, 1)));
        let delta = delta_frame(5, 1);
        assert_eq!(delta.windows(3).filter(|w| *w == [0, 0, 1]).count(), 1);
        assert_eq!(delta[4] & 0x1f, 1);
    }
}
