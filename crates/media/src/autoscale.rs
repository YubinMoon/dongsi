//! 인코더가 패킷마다 알려 주는 QP 읽기(D-099).
//!
//! ffmpeg 인코더는 `AV_PKT_DATA_QUALITY_STATS` side data에 프레임 평균 품질을 싣는다(`ff_encode_add_stats_side_data`,
//! 앞 4바이트 LE = `quality`, [4] = pict_type). `quality`는 QP × `FF_QP2LAMBDA`(118)이다. 인코더마다 QP의
//! 기준(0 또는 1 시작)과 범위가 달라서, 판정기([`dongsi_core::qp_scale`])가 쓰는 코덱 고유 범위로 바꾼다.
//! 이 모듈은 ffmpeg를 부르지 않는 순수 함수만 둔다.

use dongsi_core::VideoCodec;
use dongsi_core::qp_scale::QpScaleConfig;

/// 판정 설정(D-099 초기안, 임계값은 libwebrtc 값). 인코더마다 같은 값이다.
///
/// 전체 시험 전용 훅: `DONGSI_MEDIA_QP_FAST_UP=1`이면 올림 대기(`up_hold`·`up_gap`)를 2초로 줄인다(올림을 1분 안에 보려고).
/// 내림 판정은 그대로다.
pub(crate) fn config(codec: VideoCodec) -> QpScaleConfig {
    let c = QpScaleConfig::new(codec);
    #[cfg(feature = "full-tests")]
    let c = fast_up(c);
    c
}

#[cfg(feature = "full-tests")]
fn fast_up(mut c: QpScaleConfig) -> QpScaleConfig {
    if std::env::var("DONGSI_MEDIA_QP_FAST_UP").is_ok_and(|v| v == "1") {
        c.up_hold = std::time::Duration::from_secs(2);
        c.up_gap = std::time::Duration::from_secs(2);
    }
    c
}

/// ffmpeg `FF_QP2LAMBDA`.
const QP2LAMBDA: f32 = 118.0;

/// `AV_PICTURE_TYPE_I`.
const PICT_TYPE_I: u8 = 1;

/// 패킷 하나에서 읽은 QP.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PacketQp {
    /// 판정기가 쓰는 코덱 고유 범위(H.264 0~51, AV1 qindex 0~255)의 프레임 평균 QP.
    pub qp: f32,
    /// pict_type이 I(인트라)다. IDR이 아닌 I 프레임도 표본에서 빼려고 따로 둔다.
    pub intra: bool,
}

/// 이 인코더가 패킷마다 QP를 알려 주는가. 아니면 해상도를 자동으로 낮추지 않는다(크기 그대로).
///
/// 소스 근거(ffmpeg 8.1.3): nvenc.c 2808~2809, qsvenc.c 2687~2694(H.264만), libsvtav1.c 743.
/// AMF·libopenh264·av1_qsv는 side data를 내지 않는다.
pub fn reports_qp(encoder: &str) -> bool {
    matches!(
        encoder,
        "h264_nvenc" | "av1_nvenc" | "h264_qsv" | "libsvtav1"
    )
}

/// `quality`(QP × 118)를 코덱 고유 범위의 QP로 바꾼다. QP를 알려 주지 않는 인코더면 `None`.
///
/// - `h264_nvenc`·`av1_nvenc`: nvenc는 `(frameAvgQP - 1) * 118`을 싣는다. QP = quality/118 + 1.
///   `av1_nvenc`의 frameAvgQP 단위(0~255 qindex로 본다)는 **미검증**이다(nvEncodeAPI.h가 소스 묶음에 없다).
/// - `h264_qsv`: `enc_info->QP * 118`. QP = quality/118.
/// - `libsvtav1`: quality/118이 0~63 `qp`다. qindex ≈ 4배(0~255).
pub(crate) fn qp_from_quality(encoder: &str, quality: i32) -> Option<f32> {
    let q = quality as f32 / QP2LAMBDA;
    let qp = match encoder {
        "h264_nvenc" | "av1_nvenc" => q + 1.0,
        "h264_qsv" => q,
        "libsvtav1" => q * 4.0,
        _ => return None,
    };
    (qp.is_finite() && qp >= 0.0).then_some(qp)
}

/// `AV_PKT_DATA_QUALITY_STATS` 바이트에서 (quality, pict_type)을 읽는다. 5바이트보다 짧으면 `None`.
pub(crate) fn parse_quality_stats(data: &[u8]) -> Option<(i32, u8)> {
    let q: [u8; 4] = data.get(..4)?.try_into().ok()?;
    Some((i32::from_le_bytes(q), *data.get(4)?))
}

/// side data → [`PacketQp`].
pub(crate) fn packet_qp(encoder: &str, side_data: &[u8]) -> Option<PacketQp> {
    let (quality, pict) = parse_quality_stats(side_data)?;
    Some(PacketQp {
        qp: qp_from_quality(encoder, quality)?,
        intra: pict == PICT_TYPE_I,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side(quality: i32, pict: u8) -> Vec<u8> {
        let mut v = quality.to_le_bytes().to_vec();
        v.extend([pict, 0, 0, 0]);
        v
    }

    #[test]
    fn nvenc_adds_one() {
        // nvenc는 (QP-1)*118을 싣는다.
        assert_eq!(qp_from_quality("h264_nvenc", 29 * 118), Some(30.0));
        assert_eq!(qp_from_quality("av1_nvenc", 199 * 118), Some(200.0));
        // frameAvgQP가 0이면 (0-1)*118 = -118: 0으로.
        assert_eq!(qp_from_quality("h264_nvenc", -118), Some(0.0));
    }

    #[test]
    fn qsv_is_plain() {
        assert_eq!(qp_from_quality("h264_qsv", 31 * 118), Some(31.0));
        assert_eq!(qp_from_quality("h264_qsv", -5), None);
    }

    #[test]
    fn svtav1_is_scaled_to_qindex() {
        assert_eq!(qp_from_quality("libsvtav1", 40 * 118), Some(160.0));
        assert_eq!(qp_from_quality("libsvtav1", 63 * 118), Some(252.0));
    }

    #[test]
    fn others_report_nothing() {
        for name in ["h264_amf", "av1_amf", "libopenh264", "av1_qsv", "nope"] {
            assert_eq!(qp_from_quality(name, 30 * 118), None, "{name}");
            assert!(!reports_qp(name), "{name}");
        }
        for name in ["h264_nvenc", "av1_nvenc", "h264_qsv", "libsvtav1"] {
            assert!(reports_qp(name), "{name}");
        }
    }

    #[test]
    fn parses_side_data() {
        assert_eq!(parse_quality_stats(&side(3540, 1)), Some((3540, 1)));
        assert_eq!(parse_quality_stats(&side(-118, 3)), Some((-118, 3)));
        assert_eq!(parse_quality_stats(&[1, 2, 3, 4]), None);
        assert_eq!(parse_quality_stats(&[]), None);
    }

    #[test]
    fn packet_qp_combines() {
        let p = packet_qp("h264_nvenc", &side(24 * 118, 1)).unwrap();
        assert_eq!(
            p,
            PacketQp {
                qp: 25.0,
                intra: true
            }
        );
        let p = packet_qp("h264_nvenc", &side(24 * 118, 2)).unwrap();
        assert!(!p.intra);
        assert_eq!(packet_qp("libopenh264", &side(24 * 118, 2)), None);
        assert_eq!(packet_qp("h264_nvenc", &[]), None);
    }
}
