//! 미디어 공통 타입: 비디오 코덱, 화질 설정.
//!
//! 캡처·인코딩(`dongsi-media`), 송출(`dongsi-net`), 서버, 앱이 같은 값을 쓴다.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// 스트림에 쓰는 비디오 코덱(D-075: H.264·AV1만).
///
/// 시청은 WebView2(Chromium)의 WebRTC로 하므로, 실제로 고를 수 있는 코덱은
/// "이 PC에 인코더가 있고" "WebView2가 디코딩할 수 있는" 것이다. 직렬화 값은 `h264`·`av1`이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    H264,
    Av1,
}

impl VideoCodec {
    pub const ALL: [VideoCodec; 2] = [VideoCodec::H264, VideoCodec::Av1];

    /// 화면에 보여 줄 이름.
    pub fn label(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::Av1 => "AV1",
        }
    }

    /// WebRTC `mimeType` (`RTCRtpCodec.mimeType`).
    pub fn mime_type(self) -> &'static str {
        match self {
            VideoCodec::H264 => "video/H264",
            VideoCodec::Av1 => "video/AV1",
        }
    }

    /// 직렬화에 쓰는 소문자 이름(`h264`, `av1`).
    pub fn as_str(self) -> &'static str {
        match self {
            VideoCodec::H264 => "h264",
            VideoCodec::Av1 => "av1",
        }
    }
}

impl fmt::Display for VideoCodec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl FromStr for VideoCodec {
    type Err = UnknownCodec;

    /// `h264`·`H.264`·`avc`·`video/H264`, `av1`·`video/AV1`을 받는다. 그 밖의 코덱은 오류다.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let lower = s.trim().to_ascii_lowercase();
        let normalized = lower.trim_start_matches("video/").replace(['.', '-'], "");
        match normalized.as_str() {
            "h264" | "avc" => Ok(VideoCodec::H264),
            "av1" => Ok(VideoCodec::Av1),
            _ => Err(UnknownCodec(s.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("알 수 없는 코덱입니다: {0}")]
pub struct UnknownCodec(pub String);

/// 인코더에 넘기는 화질 설정.
///
/// `width`·`height`는 출력 크기의 상한이다. 원본이 더 작으면 원본 크기를 쓰고,
/// 더 크면 비율을 유지한 채 이 상자 안에 맞춘다. 0이면 원본 크기 그대로다(D-074: FHD 1920×1080,
/// QHD 2560×1440, 원본 0×0).
///
/// `fps`가 0이면 주사율 "원본"이다([`SOURCE_FPS`](Self::SOURCE_FPS)). 파이프라인이 캡처 대상 모니터의
/// 주사율로 풀어 쓴다([`resolve_fps`](Self::resolve_fps)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoSettings {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
}

impl VideoSettings {
    pub const MIN_FPS: u32 = 1;
    pub const MAX_FPS: u32 = 240;
    pub const MIN_BITRATE_KBPS: u32 = 300;
    pub const MAX_BITRATE_KBPS: u32 = 100_000;
    /// 주사율 "원본"(캡처 대상 모니터의 주사율).
    pub const SOURCE_FPS: u32 = 0;
    /// 주사율을 알 수 없을 때(테스트 패턴 포함) "원본"으로 쓰는 값.
    pub const FALLBACK_FPS: u32 = 60;

    /// 앱이 정해 둔 기본값(FHD 60fps H.264 7Mbps). 개발용 시험 방송·예제·시험이 쓴다.
    /// 방송 화면은 자기 값을 직접 만든다(D-074).
    pub const DEFAULT: VideoSettings = VideoSettings {
        codec: VideoCodec::H264,
        width: 1920,
        height: 1080,
        fps: 60,
        bitrate_kbps: 7_000,
    };

    /// 값을 허용 범위로 맞춘다. fps 0(원본)은 그대로 둔다.
    pub fn clamped(mut self) -> Self {
        if self.fps != Self::SOURCE_FPS {
            self.fps = self.fps.clamp(Self::MIN_FPS, Self::MAX_FPS);
        }
        self.bitrate_kbps = self
            .bitrate_kbps
            .clamp(Self::MIN_BITRATE_KBPS, Self::MAX_BITRATE_KBPS);
        self.width = self.width.min(7680);
        self.height = self.height.min(4320);
        self
    }

    /// 실제로 쓸 fps. 0(원본)이면 캡처 대상의 주사율 `source_hz`(모르면 [`FALLBACK_FPS`](Self::FALLBACK_FPS)),
    /// 아니면 설정값이다. 결과는 1~240이다.
    pub fn resolve_fps(&self, source_hz: Option<u32>) -> u32 {
        let fps = match self.fps {
            Self::SOURCE_FPS => source_hz.filter(|&hz| hz > 0).unwrap_or(Self::FALLBACK_FPS),
            fps => fps,
        };
        fps.clamp(Self::MIN_FPS, Self::MAX_FPS)
    }

    /// 원본 크기(`src_w`×`src_h`)에서 실제 출력 크기를 계산한다.
    ///
    /// 비율을 유지하고, 인코더 호환을 위해 짝수로 맞춘다.
    pub fn output_size(&self, src_w: u32, src_h: u32) -> (u32, u32) {
        let src_w = src_w.max(2);
        let src_h = src_h.max(2);
        let (max_w, max_h) = match (self.width, self.height) {
            (0, 0) => (src_w, src_h),
            (0, h) => (u32::MAX, h),
            (w, 0) => (w, u32::MAX),
            (w, h) => (w, h),
        };
        let (w, h) = if src_w <= max_w && src_h <= max_h {
            (src_w, src_h)
        } else {
            let scale = f64::min(max_w as f64 / src_w as f64, max_h as f64 / src_h as f64);
            (
                (src_w as f64 * scale).round() as u32,
                (src_h as f64 * scale).round() as u32,
            )
        };
        (even(w), even(h))
    }
}

fn even(v: u32) -> u32 {
    (v & !1).max(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_names_roundtrip() {
        for c in VideoCodec::ALL {
            assert_eq!(c.as_str().parse::<VideoCodec>().unwrap(), c);
            assert_eq!(c.mime_type().parse::<VideoCodec>().unwrap(), c);
            let json = serde_json::to_string(&c).unwrap();
            assert_eq!(json, format!("\"{}\"", c.as_str()));
            assert_eq!(serde_json::from_str::<VideoCodec>(&json).unwrap(), c);
        }
        assert_eq!("H.264".parse::<VideoCodec>().unwrap(), VideoCodec::H264);
        assert!("mpeg2".parse::<VideoCodec>().is_err());
        // H.264·AV1 말고는 받지 않는다(D-075).
        for name in ["vp8", "VP9", "hevc", "h265"] {
            assert!(name.parse::<VideoCodec>().is_err(), "{name}");
            assert!(serde_json::from_str::<VideoCodec>(&format!("\"{name}\"")).is_err());
        }
    }

    #[test]
    fn output_size_keeps_aspect_and_even() {
        let s = VideoSettings::DEFAULT;
        assert_eq!(s.output_size(3840, 2160), (1920, 1080));
        assert_eq!(s.output_size(1280, 720), (1280, 720)); // 원본이 작으면 그대로
        assert_eq!(s.output_size(3440, 1440), (1920, 804)); // 울트라와이드
        assert_eq!(s.output_size(1848, 1156), (1726, 1080));
        let src = VideoSettings {
            codec: VideoCodec::Av1,
            width: 0,
            height: 0,
            ..VideoSettings::DEFAULT
        };
        assert_eq!(src.output_size(1921, 1081), (1920, 1080));
    }

    #[test]
    fn clamp_limits() {
        let s = VideoSettings {
            codec: VideoCodec::Av1,
            width: 99_999,
            height: 99_999,
            fps: 999,
            bitrate_kbps: 1,
        }
        .clamped();
        assert_eq!(
            (s.width, s.height, s.fps, s.bitrate_kbps),
            (7680, 4320, 240, 300)
        );
        // fps 0은 "원본"이라 지킨다.
        let s = VideoSettings {
            fps: 0,
            ..VideoSettings::DEFAULT
        }
        .clamped();
        assert_eq!(s.fps, VideoSettings::SOURCE_FPS);
    }

    #[test]
    fn resolve_fps_uses_the_source_refresh_rate_for_source_fps() {
        let source = VideoSettings {
            fps: 0,
            ..VideoSettings::DEFAULT
        };
        assert_eq!(source.resolve_fps(Some(144)), 144);
        assert_eq!(source.resolve_fps(Some(360)), 240); // 상한
        assert_eq!(source.resolve_fps(None), 60);
        assert_eq!(source.resolve_fps(Some(0)), 60);
        // 정한 값이면 주사율과 상관없다.
        assert_eq!(VideoSettings::DEFAULT.resolve_fps(Some(144)), 60);
        let thirty = VideoSettings {
            fps: 30,
            ..VideoSettings::DEFAULT
        };
        assert_eq!(thirty.resolve_fps(None), 30);
    }
}
