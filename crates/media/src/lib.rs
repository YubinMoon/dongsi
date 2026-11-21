//! dongsi 미디어: 화면·창 캡처, 시스템 오디오 루프백, WebRTC용 인코딩(Windows 11).
//!
//! - 비디오: Windows.Graphics.Capture → D3D11 Video Processor(BGRA→NV12, 크기 조절) →
//!   libavcodec 인코더(하드웨어 우선). [`VideoPipeline`]
//! - 오디오: WASAPI 루프백(Discord 프로세스 트리 제외 가능) → Opus 20ms. [`AudioPipeline`]
//! - 시각: 모든 캡처 시각은 QPC 100ns 단위다. [`clock`]
//!
//! 송출(WebRTC)은 이 크레이트 밖에서 한다. 출력 형식은 [`EncodedVideo::data`],
//! [`EncodedAudio`]를 본다.

mod audio;
pub mod bitstream;
mod capture;
pub mod clock;
mod com;
mod convert;
mod d3d;
mod encoder;
mod error;
mod ff;
mod preview;
#[doc(hidden)]
pub mod probe;
mod process;
mod source;
mod thumbnail;
#[doc(hidden)]
pub mod trace;
mod video;
mod yuv;

pub use audio::{AudioControl, AudioPipeline, AudioSource, AudioStats, EncodedAudio};
pub use encoder::{EncoderInfo, available_encoders};
pub use error::MediaError;
pub use preview::{PREVIEW_INTERVAL, PreviewFrame};
pub use process::{DISCORD_EXE_NAMES, find_discord_root_pids};
pub use source::{
    SourceId, SourceInfo, SourceKind, TEST_PATTERN_SIZE, exclude_window_from_capture, list_sources,
};
pub use thumbnail::{Thumbnail, capture_thumbnails};
pub use video::{EncodedVideo, VideoPipeline, VideoStats};

/// 측정용 도구(예제·벤치마크에서 쓴다). API 안정성을 보장하지 않는다.
#[doc(hidden)]
pub mod bench {
    use std::time::Instant;

    use crate::MediaError;
    use crate::convert::{Converter, create_test_texture};
    use crate::d3d::create_d3d;
    use crate::yuv::{Chroma, YuvView};

    /// 변환 측정 결과(ms).
    #[derive(Debug, Clone, Copy)]
    pub struct ConvertBench {
        pub gpu: bool,
        pub avg_ms: f32,
        pub max_ms: f32,
    }

    /// `src`×`src` BGRA 텍스처를 `out` 크기 NV12로 `n`번 변환해 시간을 잰다.
    /// `cpu`면 CPU 경로(스테이징 + swscale)를 쓴다.
    pub fn convert(
        src: (u32, u32),
        out: (u32, u32),
        n: usize,
        cpu: bool,
    ) -> Result<ConvertBench, MediaError> {
        if cpu {
            // SAFETY: 측정용 예제에서만 부르며, 이 프로세스의 다른 스레드가 환경을 읽지 않는 시점에 쓴다.
            unsafe { std::env::set_var("DONGSI_MEDIA_CPU_CONVERT", "1") };
        }
        let d3d = create_d3d()?;
        let mut conv = Converter::new(d3d.clone());
        if cpu {
            // SAFETY: 위와 같다.
            unsafe { std::env::remove_var("DONGSI_MEDIA_CPU_CONVERT") };
        }
        let tex = create_test_texture(&d3d, src.0, src.1)?;
        let (w, h) = (out.0 as usize, out.1 as usize);
        let mut y = vec![0u8; w * h];
        let mut uv = vec![0u8; w * h / 2];
        let mut total = 0f32;
        let mut max = 0f32;
        for i in 0..n + 3 {
            let t = Instant::now();
            conv.ingest(&tex, src.0, src.1)?;
            let mut view = YuvView {
                width: w,
                height: h,
                y: &mut y,
                y_stride: w,
                chroma: Chroma::Nv12 {
                    uv: &mut uv,
                    stride: w,
                },
            };
            conv.convert(&mut view)?;
            let ms = t.elapsed().as_secs_f32() * 1000.0;
            if i >= 3 {
                total += ms;
                max = max.max(ms);
            }
        }
        Ok(ConvertBench {
            gpu: conv.is_gpu(),
            avg_ms: total / n.max(1) as f32,
            max_ms: max,
        })
    }
}
