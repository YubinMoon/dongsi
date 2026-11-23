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

/// 오디오 RED(RFC 2198)의 중복 거리(D-087). 각 오디오 패킷에 직전 1개·2개 앞 프레임의 사본을 같이 싣는다.
///
/// 오디오에는 NACK도 재전송도 없어(str0m `format/codec_config.rs:132-136`, Chromium offer에도 오디오
/// `nack` 없음) 한 번 잃으면 PLC뿐이다. RED는 같은 프레임을 뒤따르는 패킷에 또 실어 수신쪽이 재전송 없이
/// 복구하게 한다. `str0m::Config::set_red_distances`(`config.rs:254-265`)에 그대로 넘기며, core가 str0m에
/// 의존하지 않도록 숫자만 둔다. 방송자→SFU와 SFU→시청자 두 구간 모두 쓴다. SFU(frame mode)는 방송자 세션에서
/// RED를 풀고 시청자 세션 Writer가 자기 송신 이력으로 다시 만들어, 두 구간이 서로 독립이기 때문이다.
///
/// - 복구: 연속 2개 손실까지 복구한다(실제 Edge 158에서 concealment 0). 연속 `n`개가 거리 수 `d`를 넘으면
///   `n − d`개가 남는다. 군집 손실이 심한 회선(평균 버스트 2, 10%)에서는 손실이 절반 남짓만 준다
///   (W4 조사, 2026-11-23).
/// - 대가: 오디오 payload가 (1 + 길이)배, 곧 3배다. 패킷 수는 그대로(초당 100개)라 헤더 비용은 늘지 않는다.
///   128 kbps Opus면 SFU 송신 패킷 평균 201.9 → 532.7 B, 약 +265 kbps. SFU→시청자는 시청자 수만큼 곱해진다.
/// - 비트레이트 예산은 `dongsi_net::rate::RateConfig::audio_share_kbps`가 이 길이로 센다.
pub const AUDIO_RED_DISTANCES: [u32; 2] = [1, 2];

/// simulcast 트랙(D-102). 같은 코덱으로 인코딩한 두 스트림이다.
///
/// - 상([`High`](Self::High)): 화질 설정 그대로(해상도 상자·fps·비트레이트, 네트워크 상한 적용). 주 인코더가 만든다.
/// - 하([`Low`](Self::Low)): FHD 상자([`LOW_BOX`](Self::LOW_BOX), 원본이 작으면 원본 크기)·방송 fps·
///   [`LOW_KBPS`](Self::LOW_KBPS) 고정. 인코더를 따로 둔다. 설정 비트레이트가 이 값 이하면 보내지 않는다
///   ([`low_wanted`](Self::low_wanted)): 그때는 상 하나만 나간다.
///
/// WebRTC에서는 RTP 스트림 ID(rid, `h`·`l`)로 구분한다. simulcast를 쓰지 않는 방송(소프트웨어 인코더)은 상 하나뿐이고
/// rid가 없다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Layer {
    High,
    Low,
}

impl Layer {
    /// 상, 하 순서.
    pub const ALL: [Layer; 2] = [Layer::High, Layer::Low];

    /// 하 트랙의 비트레이트(kbps, D-102: FHD 60fps 3Mbps).
    pub const LOW_KBPS: u32 = 3_000;

    /// 하 트랙의 출력 상자(FHD).
    pub const LOW_BOX: (u32, u32) = (1920, 1080);

    /// [`ALL`](Self::ALL)·[`LayerRates`] 안의 자리(상 0, 하 1).
    pub const fn index(self) -> usize {
        match self {
            Layer::High => 0,
            Layer::Low => 1,
        }
    }

    /// 설정 비트레이트가 `setting_kbps`일 때 하 트랙을 같이 보내는지: [`LOW_KBPS`](Self::LOW_KBPS)보다 클 때만(D-102).
    pub const fn low_wanted(setting_kbps: u32) -> bool {
        setting_kbps > Self::LOW_KBPS
    }

    /// 업로드 제한이 없을 때의 트랙 목표: 상은 설정 비트레이트, 하는 [`LOW_KBPS`](Self::LOW_KBPS)(보내지 않으면 0).
    pub const fn initial_rates(setting_kbps: u32) -> LayerRates {
        let low = if Self::low_wanted(setting_kbps) {
            Self::LOW_KBPS
        } else {
            0
        };
        [setting_kbps, low]
    }

    /// RTP 스트림 ID(rid).
    pub const fn rid(self) -> &'static str {
        match self {
            Layer::High => "h",
            Layer::Low => "l",
        }
    }

    pub fn from_rid(rid: &str) -> Option<Layer> {
        Layer::ALL.into_iter().find(|l| l.rid() == rid)
    }

    /// 화면·로그에 쓰는 이름.
    pub const fn label(self) -> &'static str {
        match self {
            Layer::High => "상",
            Layer::Low => "하",
        }
    }
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// 트랙별 목표 비트레이트(kbps, [`Layer::index`] 순서: 상·하). 0이면 그 트랙을 보내지 않는다.
pub type LayerRates = [u32; 2];

/// 인코더에 넘기는 화질 설정.
///
/// `width`·`height`는 출력 크기의 상한(상자)이다. 원본이 더 작으면 원본 크기를 쓰고,
/// 더 크면 비율을 유지한 채 이 상자 안에 맞춘다. 상자는 [`MAX_WIDTH`](Self::MAX_WIDTH)×[`MAX_HEIGHT`](Self::MAX_HEIGHT)
/// (2560×1440)를 넘지 않는다. 0이거나 더 크면 그 축은 상한 값이다(D-097: 4K 없음. 화면 칩은 FHD 1920×1080, QHD 2560×1440).
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
    /// 출력 크기 상한(D-097). 4K는 보내지 않는다.
    pub const MAX_WIDTH: u32 = 2560;
    pub const MAX_HEIGHT: u32 = 1440;
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
        self.width = cap_side(self.width, Self::MAX_WIDTH);
        self.height = cap_side(self.height, Self::MAX_HEIGHT);
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
    /// 비율을 유지하고, 인코더 호환을 위해 짝수로 맞춘다. 상자는 2560×1440을 넘지 않는다(D-097).
    pub fn output_size(&self, src_w: u32, src_h: u32) -> (u32, u32) {
        let src_w = src_w.max(2);
        let src_h = src_h.max(2);
        let max_w = cap_side(self.width, Self::MAX_WIDTH);
        let max_h = cap_side(self.height, Self::MAX_HEIGHT);
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

    /// 하 트랙(D-102)의 설정: 상자는 [`Layer::LOW_BOX`](FHD), 비트레이트는 [`Layer::LOW_KBPS`], 코덱·fps는 이 설정 그대로.
    pub fn low_layer(&self) -> VideoSettings {
        VideoSettings {
            width: Layer::LOW_BOX.0,
            height: Layer::LOW_BOX.1,
            bitrate_kbps: Layer::LOW_KBPS,
            ..*self
        }
    }
}

fn even(v: u32) -> u32 {
    (v & !1).max(2)
}

/// 상자 한 축: 0(제한 없음)이나 상한보다 큰 값은 상한으로.
fn cap_side(v: u32, max: u32) -> u32 {
    if v == 0 { max } else { v.min(max) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_rids_roundtrip() {
        for (i, l) in Layer::ALL.into_iter().enumerate() {
            assert_eq!(l.index(), i);
            assert_eq!(Layer::from_rid(l.rid()), Some(l));
        }
        assert_eq!(Layer::from_rid("m"), None, "중 계층은 없다(D-102)");
        assert_eq!(serde_json::to_string(&Layer::Low).unwrap(), "\"low\"");
    }

    #[test]
    fn low_track_only_above_3mbps() {
        // 설정 비트레이트가 3Mbps 이하면 단일, 넘으면 하 3Mbps를 같이(D-102).
        assert!(!Layer::low_wanted(2_500));
        assert!(!Layer::low_wanted(3_000));
        assert!(Layer::low_wanted(3_001));
        assert_eq!(Layer::initial_rates(3_000), [3_000, 0]);
        assert_eq!(Layer::initial_rates(8_000), [8_000, 3_000]);
        // 하는 FHD 상자·3Mbps, fps·코덱은 그대로.
        let qhd30 = VideoSettings {
            codec: VideoCodec::Av1,
            width: 2560,
            height: 1440,
            fps: 30,
            bitrate_kbps: 12_000,
        };
        let low = qhd30.low_layer();
        assert_eq!(
            (low.codec, low.width, low.height, low.fps, low.bitrate_kbps),
            (VideoCodec::Av1, 1920, 1080, 30, 3_000)
        );
        assert_eq!(low.output_size(3840, 2160), (1920, 1080));
        assert_eq!(low.output_size(1280, 720), (1280, 720)); // 원본이 작으면 원본
        assert_eq!(low.output_size(3440, 1440), (1920, 804));
    }

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
        // 4K는 없다(D-097): 상자 0×0이나 더 큰 상자도 2560×1440 안으로.
        assert_eq!(src.output_size(3840, 2160), (2560, 1440));
        assert_eq!(src.output_size(3440, 1440), (2560, 1072));
        let big = VideoSettings {
            width: 3840,
            height: 2160,
            ..VideoSettings::DEFAULT
        };
        assert_eq!(big.output_size(3840, 2160), (2560, 1440));
        assert_eq!(big.output_size(1920, 1080), (1920, 1080));
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
            (2560, 1440, 240, 300)
        );
        let s = VideoSettings {
            width: 0,
            height: 0,
            ..VideoSettings::DEFAULT
        }
        .clamped();
        assert_eq!((s.width, s.height), (2560, 1440));
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
