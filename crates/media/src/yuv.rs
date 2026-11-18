//! 4:2:0 YUV(BT.601 limited range) 프레임 버퍼 보기와 테스트 패턴 그리기.

use dongsi_core::VideoSettings;

use crate::clock::QPC_PER_SEC;
use crate::source::TEST_PATTERN_SIZE;

/// 색상 하나(Y, U, V). BT.601 limited range(16~235).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Yuv(pub u8, pub u8, pub u8);

impl Yuv {
    #[cfg(test)]
    pub const BLACK: Yuv = Yuv(16, 128, 128);
    pub const WHITE: Yuv = Yuv(235, 128, 128);
    pub const DARK: Yuv = Yuv(28, 128, 128);
    pub const BAR: Yuv = Yuv(90, 170, 100);
    pub const DIGIT: Yuv = Yuv(150, 110, 140);
    pub const TICK_ON: Yuv = Yuv(120, 90, 160);
    pub const TICK_OFF: Yuv = Yuv(40, 128, 128);
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

    /// Y 평균(검증용).
    #[cfg(test)]
    pub fn mean_luma(&self) -> f64 {
        let mut sum = 0u64;
        for row in 0..self.height {
            sum += self.y[row * self.y_stride..row * self.y_stride + self.width]
                .iter()
                .map(|&v| v as u64)
                .sum::<u64>();
        }
        sum as f64 / (self.width * self.height) as f64
    }
}

/// 테스트 패턴의 원본 크기.
///
/// 패턴은 해상도와 무관하게 출력 크기에서 바로 그린다. 화질 설정의 상자가 0이면
/// 1280×720이고, 상자가 있으면 그 안에 들어가는 가장 큰 16:9 크기다(확대 포함).
/// 그래서 3840×2160 설정이면 4K 패턴이 나온다.
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

/// 테스트 패턴 프레임의 생성 시각 `t`(QPC 100ns) 다음 틱.
///
/// 틱은 매 정초(QPC가 1초의 배수)에 맞춰 `S + floor(k·10^7/fps)` (k = 0..fps-1)에 놓인다.
/// 그래서 매 초 첫 틱은 정확히 정초다.
pub(crate) fn pattern_next_tick(after: i64, fps: u32) -> i64 {
    let fps = fps.max(1) as i64;
    let s = after.div_euclid(QPC_PER_SEC) * QPC_PER_SEC;
    let off = after - s;
    let k = ((off + 1) * fps + QPC_PER_SEC - 1) / QPC_PER_SEC;
    if k >= fps {
        s + QPC_PER_SEC
    } else {
        s + k * QPC_PER_SEC / fps
    }
}

/// 이 프레임이 흰 프레임인지: 정초(QPC % 10^7 == 0)를 지난 뒤 첫 프레임이면 흰색.
///
/// 이전 프레임이 없으면 정확히 정초일 때만 흰색이다.
pub(crate) fn pattern_is_white(prev: Option<i64>, t: i64) -> bool {
    match prev {
        Some(p) => t.div_euclid(QPC_PER_SEC) > p.div_euclid(QPC_PER_SEC),
        None => t.rem_euclid(QPC_PER_SEC) == 0,
    }
}

/// 테스트 패턴 한 장을 그린다.
///
/// - 흰 프레임: 전체 흰색(Y=235).
/// - 그 밖: 어두운 배경, 1초에 한 번 화면을 가로지르는 세로 막대(위치 = 초 안의 시각),
///   0.1초 칸 표시 10개, 프레임 번호와 초(QPC 초의 끝 세 자리) 숫자.
pub(crate) fn render_pattern(view: &mut YuvView<'_>, t: i64, white: bool, index: u64) {
    if white {
        view.fill(Yuv::WHITE);
        return;
    }
    let (w, h) = (view.width, view.height);
    view.fill(Yuv::DARK);
    let sub = t.rem_euclid(QPC_PER_SEC);
    // 움직이는 막대
    let bar_w = (w / 40).max(4);
    let x = (sub as i128 * (w - bar_w) as i128 / QPC_PER_SEC as i128) as usize;
    view.fill_rect(x, 0, bar_w, h, Yuv::BAR);
    // 0.1초 칸
    let cell = (w / 24).max(4);
    let gap = cell / 4;
    let tenth = (sub / (QPC_PER_SEC / 10)) as usize;
    let y0 = h - cell * 2;
    for i in 0..10 {
        let c = if i == tenth {
            Yuv::TICK_ON
        } else {
            Yuv::TICK_OFF
        };
        view.fill_rect(gap + i * (cell + gap), y0, cell, cell, c);
    }
    // 숫자: 프레임 번호, 초
    let unit = (h / 90).max(1);
    draw_number(view, unit * 4, unit * 4, unit, index % 1_000_000, 6);
    let secs = (t.div_euclid(QPC_PER_SEC) % 1000) as u64;
    draw_number(view, unit * 4, unit * 16, unit, secs, 3);
}

/// 7세그먼트 숫자. 각 자리는 가로 4u, 세로 7u 칸을 쓴다(+2u 간격).
fn draw_number(view: &mut YuvView<'_>, x: usize, y: usize, u: usize, value: u64, digits: u32) {
    const SEGMENTS: [u8; 10] = [
        0b0111111, 0b0000110, 0b1011011, 0b1001111, 0b1100110, 0b1101101, 0b1111101, 0b0000111,
        0b1111111, 0b1101111,
    ];
    for i in 0..digits {
        let d = (value / 10u64.pow(digits - 1 - i) % 10) as usize;
        let dx = x + i as usize * 6 * u;
        let seg = SEGMENTS[d];
        // a b c d e f g
        let rects = [
            (dx, y, 4 * u, u),                 // a
            (dx + 3 * u, y, u, 4 * u),         // b
            (dx + 3 * u, y + 3 * u, u, 4 * u), // c
            (dx, y + 6 * u, 4 * u, u),         // d
            (dx, y + 3 * u, u, 4 * u),         // e
            (dx, y, u, 4 * u),                 // f
            (dx, y + 3 * u, 4 * u, u),         // g
        ];
        for (bit, r) in rects.iter().enumerate() {
            if seg & (1 << bit) != 0 {
                view.fill_rect(r.0, r.1, r.2, r.3, Yuv::DIGIT);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dongsi_core::{QualityPreset, VideoCodec};

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
    fn ticks_align_to_whole_seconds() {
        let fps = 60;
        let mut t = 5 * QPC_PER_SEC - 1;
        let mut ticks = Vec::new();
        for _ in 0..61 {
            t = pattern_next_tick(t, fps);
            ticks.push(t);
        }
        assert_eq!(ticks[0], 5 * QPC_PER_SEC);
        assert_eq!(ticks[60], 6 * QPC_PER_SEC);
        assert_eq!(ticks[1], 5 * QPC_PER_SEC + 166_666);
        assert!(ticks.windows(2).all(|w| w[1] > w[0]));
        // 정확히 틱 위에 있으면 다음 틱으로
        assert_eq!(
            pattern_next_tick(5 * QPC_PER_SEC, fps),
            5 * QPC_PER_SEC + 166_666
        );
        // 30fps: 1초에 30틱
        let mut t = 0;
        let mut n = 0;
        while t < QPC_PER_SEC {
            t = pattern_next_tick(t, 30);
            n += 1;
        }
        assert_eq!(n, 30);
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
    fn pattern_frames_white_and_dark() {
        for layout in 0..2 {
            let mut b = Buf::new(64, 36);
            let mut v = if layout == 0 { b.nv12() } else { b.i420() };
            render_pattern(&mut v, 7 * QPC_PER_SEC, true, 0);
            assert_eq!(v.mean_luma(), 235.0);
            render_pattern(&mut v, 7 * QPC_PER_SEC + 1_234_567, false, 42);
            assert!(v.mean_luma() < 60.0, "{}", v.mean_luma());
        }
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
        let s = QualityPreset::P2160p60.settings(VideoCodec::H264);
        assert_eq!(pattern_source_size(&s), (3840, 2160));
        let s = QualityPreset::Source.settings(VideoCodec::H264);
        assert_eq!(pattern_source_size(&s), (1280, 720));
        let s = QualityPreset::P1080p60.settings(VideoCodec::H264);
        let (w, h) = pattern_source_size(&s);
        assert_eq!((w, h), (1920, 1080));
        assert_eq!(s.output_size(w, h), (1920, 1080));
    }
}
