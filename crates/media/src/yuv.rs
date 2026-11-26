//! 4:2:0 YUV(BT.601 limited range) 프레임 버퍼 보기와 테스트 패턴의 크기·흰 프레임 규칙.
//!
//! 테스트 패턴의 틱(프레임 시각)은 비디오 파이프라인의 정초 격자(`video.rs`의 `FrameGrid`, D-111)가 정한다.
//!
//! 테스트 패턴 그림은 GPU(`gpu_pattern.rs`)가 그린다. CPU로 YUV에 그리는 패턴은 없다(D-105).

use dongsi_core::VideoSettings;

use crate::clock::QPC_PER_SEC;
use crate::source::TEST_PATTERN_SIZE;

/// 색상 하나(Y, U, V). BT.601 limited range(16~235).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Yuv(pub u8, pub u8, pub u8);

#[cfg(test)]
impl Yuv {
    pub const BLACK: Yuv = Yuv(16, 128, 128);
    pub const WHITE: Yuv = Yuv(235, 128, 128);
}

/// 크로마 평면 배치.
pub(crate) enum Chroma<'a> {
    /// U·V가 번갈아 들어 있는 평면 하나(NV12).
    Nv12 { uv: &'a mut [u8], stride: usize },
    /// U, V 평면이 따로 있음(YUV420P/I420).
    I420 {
        u: &'a mut [u8],
        u_stride: usize,
        v: &'a mut [u8],
        v_stride: usize,
    },
}

/// 인코더 입력 프레임(4:2:0)에 쓰기 위한 보기. 너비·높이는 짝수다.
pub(crate) struct YuvView<'a> {
    pub width: usize,
    pub height: usize,
    pub y: &'a mut [u8],
    pub y_stride: usize,
    pub chroma: Chroma<'a>,
}

impl YuvView<'_> {
    /// 사각형을 칠한다. 화면 밖은 잘라 내고, 크로마는 2픽셀 단위로 맞춘다.
    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, c: Yuv) {
        let x1 = (x + w).min(self.width);
        let y1 = (y + h).min(self.height);
        if x >= x1 || y >= y1 {
            return;
        }
        for row in y..y1 {
            let start = row * self.y_stride;
            self.y[start + x..start + x1].fill(c.0);
        }
        let (cx0, cx1) = (x / 2, x1.div_ceil(2));
        let (cy0, cy1) = (y / 2, y1.div_ceil(2));
        match &mut self.chroma {
            Chroma::Nv12 { uv, stride } => {
                for row in cy0..cy1 {
                    let line = &mut uv[row * *stride..];
                    for cx in cx0..cx1 {
                        line[cx * 2] = c.1;
                        line[cx * 2 + 1] = c.2;
                    }
                }
            }
            Chroma::I420 {
                u,
                u_stride,
                v,
                v_stride,
            } => {
                for row in cy0..cy1 {
                    u[row * *u_stride + cx0..row * *u_stride + cx1].fill(c.1);
                    v[row * *v_stride + cx0..row * *v_stride + cx1].fill(c.2);
                }
            }
        }
    }

    pub fn fill(&mut self, c: Yuv) {
        let (w, h) = (self.width, self.height);
        self.fill_rect(0, 0, w, h, c);
    }

    /// NV12 원본(행 간격 `pitch`, UV 평면은 `uv` 시작)을 이 프레임으로 복사한다.
    pub fn copy_from_nv12(&mut self, y_src: &[u8], uv_src: &[u8], pitch: usize) {
        let (w, h) = (self.width, self.height);
        for row in 0..h {
            self.y[row * self.y_stride..row * self.y_stride + w]
                .copy_from_slice(&y_src[row * pitch..row * pitch + w]);
        }
        match &mut self.chroma {
            Chroma::Nv12 { uv, stride } => {
                for row in 0..h / 2 {
                    uv[row * *stride..row * *stride + w]
                        .copy_from_slice(&uv_src[row * pitch..row * pitch + w]);
                }
            }
            Chroma::I420 {
                u,
                u_stride,
                v,
                v_stride,
            } => {
                for row in 0..h / 2 {
                    let src = &uv_src[row * pitch..row * pitch + w];
                    let ud = &mut u[row * *u_stride..row * *u_stride + w / 2];
                    let vd = &mut v[row * *v_stride..row * *v_stride + w / 2];
                    for (i, pair) in src.as_chunks::<2>().0.iter().enumerate() {
                        ud[i] = pair[0];
                        vd[i] = pair[1];
                    }
                }
            }
        }
    }
}

/// 테스트 패턴(GPU 패턴)의 원본 크기.
///
/// 화질 설정의 상자가 0이면 1280×720이고, 상자가 있으면 그 안에 들어가는 가장 큰 16:9 크기다(확대 포함).
/// 그래서 2560×1440 설정이면 QHD 패턴이 나온다(설정 상자는 2560×1440을 넘지 않는다, D-097).
pub(crate) fn pattern_source_size(s: &VideoSettings) -> (u32, u32) {
    let (bw, bh) = TEST_PATTERN_SIZE;
    let scale = match (s.width, s.height) {
        (0, 0) => return (bw, bh),
        (0, h) => h as f64 / bh as f64,
        (w, 0) => w as f64 / bw as f64,
        (w, h) => f64::min(w as f64 / bw as f64, h as f64 / bh as f64),
    };
    let w = ((bw as f64 * scale).round() as u32 & !1).max(16);
    let h = ((bh as f64 * scale).round() as u32 & !1).max(16);
    (w, h)
}

/// 이 프레임이 흰 프레임인지: 정초(QPC % 10^7 == 0)를 지난 뒤 첫 프레임이면 흰색. 틱은 정초 격자라 매 초 첫 틱이 정확히
/// 정초다.
///
/// 이전 프레임이 없으면 정확히 정초일 때만 흰색이다.
pub(crate) fn pattern_is_white(prev: Option<i64>, t: i64) -> bool {
    match prev {
        Some(p) => t.div_euclid(QPC_PER_SEC) > p.div_euclid(QPC_PER_SEC),
        None => t.rem_euclid(QPC_PER_SEC) == 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dongsi_core::VideoSettings;

    struct Buf {
        w: usize,
        h: usize,
        y: Vec<u8>,
        c: Vec<u8>,
        c2: Vec<u8>,
    }

    impl Buf {
        fn new(w: usize, h: usize) -> Self {
            Buf {
                w,
                h,
                y: vec![0; w * h],
                c: vec![0; w * h / 2],
                c2: vec![0; w * h / 4],
            }
        }
        fn nv12(&mut self) -> YuvView<'_> {
            YuvView {
                width: self.w,
                height: self.h,
                y: &mut self.y,
                y_stride: self.w,
                chroma: Chroma::Nv12 {
                    uv: &mut self.c,
                    stride: self.w,
                },
            }
        }
        fn i420(&mut self) -> YuvView<'_> {
            YuvView {
                width: self.w,
                height: self.h,
                y: &mut self.y,
                y_stride: self.w,
                chroma: Chroma::I420 {
                    u: &mut self.c[..self.w * self.h / 4],
                    u_stride: self.w / 2,
                    v: &mut self.c2,
                    v_stride: self.w / 2,
                },
            }
        }
    }

    #[test]
    fn white_rule() {
        assert!(pattern_is_white(None, 3 * QPC_PER_SEC));
        assert!(!pattern_is_white(None, 3 * QPC_PER_SEC + 1));
        assert!(pattern_is_white(Some(QPC_PER_SEC - 5), QPC_PER_SEC + 100));
        assert!(!pattern_is_white(Some(QPC_PER_SEC), QPC_PER_SEC + 100));
        // 프레임을 건너뛰어도 정초를 지난 첫 프레임은 흰색
        assert!(pattern_is_white(Some(QPC_PER_SEC - 5), 3 * QPC_PER_SEC + 7));
    }

    #[test]
    fn fill_rect_clips_and_sets_chroma() {
        let mut b = Buf::new(8, 4);
        let mut v = b.nv12();
        v.fill(Yuv::BLACK);
        v.fill_rect(6, 2, 10, 10, Yuv(200, 1, 2));
        assert_eq!(&b.y[2 * 8..3 * 8], &[16, 16, 16, 16, 16, 16, 200, 200]);
        assert_eq!(&b.c[8..16], &[128, 128, 128, 128, 128, 128, 1, 2]);
    }

    #[test]
    fn nv12_to_i420_copy() {
        let (w, h) = (4, 2);
        let ysrc: Vec<u8> = (0..16).collect(); // pitch 8
        let uvsrc: Vec<u8> = vec![10, 20, 11, 21, 0, 0, 0, 0];
        let mut b = Buf::new(w, h);
        let mut v = b.i420();
        v.copy_from_nv12(&ysrc, &uvsrc, 8);
        assert_eq!(&b.y, &[0, 1, 2, 3, 8, 9, 10, 11]);
        assert_eq!(&b.c[..2], &[10, 11]);
        assert_eq!(&b.c2[..2], &[20, 21]);
    }

    #[test]
    fn pattern_size_follows_settings_box() {
        let boxed = |width, height| VideoSettings {
            width,
            height,
            ..VideoSettings::DEFAULT
        };
        assert_eq!(pattern_source_size(&boxed(2560, 1440)), (2560, 1440));
        assert_eq!(pattern_source_size(&boxed(0, 0)), (1280, 720));
        let s = VideoSettings::DEFAULT;
        let (w, h) = pattern_source_size(&s);
        assert_eq!((w, h), (1920, 1080));
        assert_eq!(s.output_size(w, h), (1920, 1080));
    }
}
