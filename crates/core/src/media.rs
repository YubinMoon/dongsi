//! 미디어 공통 타입: 화질 설정, 오디오 RED 거리. 비디오 코덱은 H.264 하나라 따로 타입을 두지 않는다(D-103).
//!
//! 캡처·인코딩(`dongsi-media`), 송출(`dongsi-net`), 서버, 앱이 같은 값을 쓴다.

use serde::{Deserialize, Serialize};

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

/// 인코더에 넘기는 화질 설정.
///
/// `width`·`height`는 출력 크기의 상한(상자)이다. 원본이 더 작으면 원본 크기를 쓰고,
/// 더 크면 비율을 유지한 채 이 상자 안에 맞춘다. 상자는 [`MAX_WIDTH`](Self::MAX_WIDTH)×[`MAX_HEIGHT`](Self::MAX_HEIGHT)
/// (2560×1440)를 넘지 않는다. 0이거나 더 크면 그 축은 상한 값이다(D-097: 4K 없음. 화면 칩은 FHD 1920×1080, QHD 2560×1440).
///
/// `fps`는 [`FPS_30`](Self::FPS_30) 또는 [`FPS_60`](Self::FPS_60)이다(D-107). 다른 값(저장된 옛 상태 등)은
/// [`clamped`](Self::clamped)가 60으로 바꾼다. 화면 전환 등으로 캡처 대상이 바뀌어도 fps는 이 값 그대로다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
}

impl VideoSettings {
    /// 방송 fps는 이 둘뿐이다(D-107).
    pub const FPS_30: u32 = 30;
    pub const FPS_60: u32 = 60;
    pub const MIN_BITRATE_KBPS: u32 = 300;
    /// 최대 25 Mbps. 가장 높은 조합(QHD 60fps)에서도 화면 공유는 그 위로 주어도 화질 이득이 작다.
    pub const MAX_BITRATE_KBPS: u32 = 25_000;
    /// 출력 크기 상한(D-097). 4K는 보내지 않는다.
    pub const MAX_WIDTH: u32 = 2560;
    pub const MAX_HEIGHT: u32 = 1440;

    /// 앱이 정해 둔 기본값(FHD 60fps H.264 7Mbps). 개발용 시험 방송·예제·시험이 쓴다.
    /// 방송 화면은 자기 값을 직접 만든다(D-074).
    pub const DEFAULT: VideoSettings = VideoSettings {
        width: 1920,
        height: 1080,
        fps: 60,
        bitrate_kbps: 7_000,
    };

    /// 값을 허용 범위로 맞춘다. fps는 30이 아니면 60이다(D-107).
    pub fn clamped(mut self) -> Self {
        if self.fps != Self::FPS_30 {
            self.fps = Self::FPS_60;
        }
        self.bitrate_kbps = self
            .bitrate_kbps
            .clamp(Self::MIN_BITRATE_KBPS, Self::MAX_BITRATE_KBPS);
        self.width = cap_side(self.width, Self::MAX_WIDTH);
        self.height = cap_side(self.height, Self::MAX_HEIGHT);
        self
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
    fn output_size_keeps_aspect_and_even() {
        let s = VideoSettings::DEFAULT;
        assert_eq!(s.output_size(3840, 2160), (1920, 1080));
        assert_eq!(s.output_size(1280, 720), (1280, 720)); // 원본이 작으면 그대로
        assert_eq!(s.output_size(3440, 1440), (1920, 804)); // 울트라와이드
        assert_eq!(s.output_size(1848, 1156), (1726, 1080));
        let src = VideoSettings {
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
            width: 99_999,
            height: 99_999,
            fps: 999,
            bitrate_kbps: 1,
        }
        .clamped();
        assert_eq!(
            (s.width, s.height, s.fps, s.bitrate_kbps),
            (2560, 1440, 60, 300)
        );
        let s = VideoSettings {
            width: 0,
            height: 0,
            ..VideoSettings::DEFAULT
        }
        .clamped();
        assert_eq!((s.width, s.height), (2560, 1440));
    }

    #[test]
    fn fps_is_30_or_60() {
        let fps = |fps| {
            VideoSettings {
                fps,
                ..VideoSettings::DEFAULT
            }
            .clamped()
            .fps
        };
        assert_eq!(fps(30), 30);
        assert_eq!(fps(60), 60);
        // 다른 값(옛 "원본" 0, 144, 범위 밖)은 모두 60이다(D-107).
        for other in [0, 1, 29, 31, 59, 61, 120, 144, 240, 999] {
            assert_eq!(fps(other), 60, "fps {other}");
        }
    }
}
