//! 미디어 공통 타입: 비디오 코덱, 화질 설정, 화질 프리셋.
//!
//! 캡처·인코딩(`dongsi-media`), 송출(`dongsi-net`), 서버, 앱이 같은 값을 쓴다.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// 스트림에 쓰는 비디오 코덱.
///
/// 시청은 WebView2(Chromium)의 WebRTC로 하므로, 실제로 고를 수 있는 코덱은
/// "이 PC에 인코더가 있고" "WebView2가 디코딩할 수 있는" 것이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    H264,
    H265,
    Vp8,
    Vp9,
    Av1,
}

impl VideoCodec {
    pub const ALL: [VideoCodec; 5] = [
        VideoCodec::H264,
        VideoCodec::H265,
        VideoCodec::Vp8,
        VideoCodec::Vp9,
        VideoCodec::Av1,
    ];

    /// 화면에 보여 줄 이름.
    pub fn label(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::H265 => "H.265",
            VideoCodec::Vp8 => "VP8",
            VideoCodec::Vp9 => "VP9",
            VideoCodec::Av1 => "AV1",
        }
    }

    /// WebRTC `mimeType` (`RTCRtpCodec.mimeType`).
    pub fn mime_type(self) -> &'static str {
        match self {
            VideoCodec::H264 => "video/H264",
            VideoCodec::H265 => "video/H265",
            VideoCodec::Vp8 => "video/VP8",
            VideoCodec::Vp9 => "video/VP9",
            VideoCodec::Av1 => "video/AV1",
        }
    }

    /// 직렬화에 쓰는 소문자 이름(`h264`, `av1` 등).
    pub fn as_str(self) -> &'static str {
        match self {
            VideoCodec::H264 => "h264",
            VideoCodec::H265 => "h265",
            VideoCodec::Vp8 => "vp8",
            VideoCodec::Vp9 => "vp9",
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

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let lower = s.trim().to_ascii_lowercase();
        let normalized = lower.trim_start_matches("video/").replace(['.', '-'], "");
        match normalized.as_str() {
            "h264" | "avc" => Ok(VideoCodec::H264),
            "h265" | "hevc" => Ok(VideoCodec::H265),
            "vp8" => Ok(VideoCodec::Vp8),
            "vp9" => Ok(VideoCodec::Vp9),
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
/// 더 크면 비율을 유지한 채 이 상자 안에 맞춘다. 0이면 원본 크기 그대로다.
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

    /// 값을 허용 범위로 맞춘다.
    pub fn clamped(mut self) -> Self {
        self.fps = self.fps.clamp(Self::MIN_FPS, Self::MAX_FPS);
        self.bitrate_kbps = self
            .bitrate_kbps
            .clamp(Self::MIN_BITRATE_KBPS, Self::MAX_BITRATE_KBPS);
        self.width = self.width.min(7680);
        self.height = self.height.min(4320);
        self
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

/// 방송을 시작할 때 고르는 화질 프리셋.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QualityPreset {
    P720p30,
    P1080p30,
    P1080p60,
    P1440p60,
    P2160p30,
    P2160p60,
    /// 원본 해상도, 60fps.
    Source,
}

impl QualityPreset {
    pub const ALL: [QualityPreset; 7] = [
        QualityPreset::P720p30,
        QualityPreset::P1080p30,
        QualityPreset::P1080p60,
        QualityPreset::P1440p60,
        QualityPreset::P2160p30,
        QualityPreset::P2160p60,
        QualityPreset::Source,
    ];

    pub fn label(self) -> &'static str {
        match self {
            QualityPreset::P720p30 => "720p 30fps",
            QualityPreset::P1080p30 => "1080p 30fps",
            QualityPreset::P1080p60 => "1080p 60fps",
            QualityPreset::P1440p60 => "1440p 60fps",
            QualityPreset::P2160p30 => "4K 30fps",
            QualityPreset::P2160p60 => "4K 60fps",
            QualityPreset::Source => "원본 60fps",
        }
    }

    /// 프리셋의 (최대 너비, 최대 높이, fps).
    pub fn dimensions(self) -> (u32, u32, u32) {
        match self {
            QualityPreset::P720p30 => (1280, 720, 30),
            QualityPreset::P1080p30 => (1920, 1080, 30),
            QualityPreset::P1080p60 => (1920, 1080, 60),
            QualityPreset::P1440p60 => (2560, 1440, 60),
            QualityPreset::P2160p30 => (3840, 2160, 30),
            QualityPreset::P2160p60 => (3840, 2160, 60),
            QualityPreset::Source => (0, 0, 60),
        }
    }

    /// 코덱별 기본 비트레이트(kbps). 화면 공유는 정지 화면이 많아 일반 영상보다 낮게 잡는다.
    pub fn default_bitrate_kbps(self, codec: VideoCodec) -> u32 {
        let base = match self {
            QualityPreset::P720p30 => 2_500,
            QualityPreset::P1080p30 => 4_500,
            QualityPreset::P1080p60 => 7_000,
            QualityPreset::P1440p60 => 12_000,
            QualityPreset::P2160p30 => 16_000,
            QualityPreset::P2160p60 => 25_000,
            QualityPreset::Source => 12_000,
        };
        // 압축 효율이 좋은 코덱은 같은 화질에 비트레이트를 덜 쓴다.
        match codec {
            VideoCodec::H264 | VideoCodec::Vp8 => base,
            VideoCodec::H265 | VideoCodec::Vp9 => base * 4 / 5,
            VideoCodec::Av1 => base * 7 / 10,
        }
    }

    pub fn settings(self, codec: VideoCodec) -> VideoSettings {
        let (width, height, fps) = self.dimensions();
        VideoSettings {
            codec,
            width,
            height,
            fps,
            bitrate_kbps: self.default_bitrate_kbps(codec),
        }
    }
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
        }
        assert_eq!("HEVC".parse::<VideoCodec>().unwrap(), VideoCodec::H265);
        assert_eq!("H.264".parse::<VideoCodec>().unwrap(), VideoCodec::H264);
        assert!("mpeg2".parse::<VideoCodec>().is_err());
    }

    #[test]
    fn output_size_keeps_aspect_and_even() {
        let s = QualityPreset::P1080p60.settings(VideoCodec::H264);
        assert_eq!(s.output_size(3840, 2160), (1920, 1080));
        assert_eq!(s.output_size(1280, 720), (1280, 720)); // 원본이 작으면 그대로
        assert_eq!(s.output_size(3440, 1440), (1920, 804)); // 울트라와이드
        assert_eq!(s.output_size(1848, 1156), (1726, 1080));
        let src = QualityPreset::Source.settings(VideoCodec::Av1);
        assert_eq!(src.output_size(1921, 1081), (1920, 1080));
    }

    #[test]
    fn clamp_limits() {
        let s = VideoSettings {
            codec: VideoCodec::Vp9,
            width: 99_999,
            height: 99_999,
            fps: 0,
            bitrate_kbps: 1,
        }
        .clamped();
        assert_eq!(
            (s.width, s.height, s.fps, s.bitrate_kbps),
            (7680, 4320, 1, 300)
        );
    }
}
