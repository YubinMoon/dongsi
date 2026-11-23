//! 송출 화면 미리 보기: 인코더에 넣는 프레임을 가끔 작은 RGBA로 줄여 둔다.
//!
//! 방송 중 화면의 "다른 사람에게 보이는 모습" 카드가 쓴다. 방송자는 내 방송을 시청하지 않으므로(D-065)
//! 서버를 거치지 않고 여기서 바로 본다. 소리는 없다.
//!
//! - 화면이 [`PreviewSlot::request`]로 부른 뒤 [`WANT_TTL`] 안에만, [`INTERVAL`]마다 한 장 만든다.
//!   화면을 닫거나 창을 내리면(부르지 않으면) 비용이 없다.
//! - 여러 곳이 다른 크기로 부르면(송출 화면 카드와 방송 목록 썸네일, D-076) 그중 가장 큰 크기로 만든다.
//!   작게 원한 쪽은 큰 그림을 받아 줄여 쓴다(번갈아 크기가 바뀌지 않게).
//! - 줄이기는 비디오 워커 스레드에서 인코딩 직전 프레임(4:2:0, BT.601 limited)으로 한다. 그래서 그림은
//!   시청자가 받는 해상도·화면 그대로다(압축 손실만 빼고).
//! - 원본이 목표보다 훨씬 크면 행을 건너뛰어(행 간격을 늘려) 읽은 뒤 swscale로 줄인다. 4K 원본도 워커가
//!   1ms 안팎만 쓰게 하려는 것이다(측정: research.md).

use std::ptr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ffmpeg_sys_next as ff;

use crate::MediaError;
use crate::yuv::{Chroma, YuvView};

/// 미리 보기를 만드는 간격(초당 10장).
pub const PREVIEW_INTERVAL: Duration = Duration::from_millis(100);
/// 마지막 요청 뒤 이만큼 지나면 만들지 않는다(화면이 닫혔거나 창이 내려감).
const WANT_TTL: Duration = Duration::from_secs(1);
/// 요청 크기 한도(기기 픽셀). 화면 카드가 아무리 커도 이보다 크게 만들지 않는다.
/// 워커가 쓰는 시간을 1ms 안팎으로 두려는 값이다(4K 원본 → 560×315 1.2ms, → 1280×720 6ms, research.md).
const MAX_SIDE: u32 = 640;
const MIN_SIDE: u32 = 16;
/// swscale 출력 행 간격의 정렬과 버퍼 끝 여유(바이트). x86 SIMD 경로(같은 크기의 I420 → RGBA 등)는 한 행을
/// 16픽셀(64바이트) 단위로 써서, 너비가 16의 배수가 아니면 행 끝 너머(마지막 행이면 버퍼 끝 너머)에 최대
/// 56바이트를 더 쓴다. 2×2를 딱 맞는 버퍼로 받으면 힙이 깨져 앱이 죽었다(research.md "swscale 출력 넘침").
const SWS_ALIGN: usize = 64;

/// 미리 보기 한 장.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewFrame {
    /// 만든 순서(1부터). 화면은 마지막으로 받은 번호를 보내 같은 그림을 다시 받지 않는다.
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    /// RGBA 8비트, 행 간격 `width * 4`, 알파는 255.
    pub rgba: Vec<u8>,
}

/// 크기를 다르게 부른 곳을 이만큼까지만 따로 기억한다(보통 화면 카드와 썸네일 둘).
const MAX_WANTS: usize = 4;

/// 요청 하나: 이 크기 안의 그림을 `until`까지 원한다.
#[derive(Clone, Copy)]
struct Want {
    until: Instant,
    max: (u32, u32),
}

#[derive(Default)]
struct SlotState {
    /// 아직 끝나지 않은 요청(크기마다 하나).
    wants: Vec<Want>,
    last_made: Option<Instant>,
    seq: u64,
    frame: Option<Arc<PreviewFrame>>,
}

/// 화면(요청)과 워커(생산) 사이의 미리 보기 칸.
#[derive(Default)]
pub(crate) struct PreviewSlot {
    state: Mutex<SlotState>,
}

impl PreviewSlot {
    fn lock(&self) -> MutexGuard<'_, SlotState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 화면의 요청. 앞으로 [`WANT_TTL`] 동안 `max_width`×`max_height` 안에 맞춘 그림을 만들게 하고,
    /// 마지막 그림의 번호가 `after`와 다르면 그 그림을 돌려준다.
    pub fn request(
        &self,
        now: Instant,
        after: u64,
        max_width: u32,
        max_height: u32,
    ) -> Option<Arc<PreviewFrame>> {
        let mut s = self.lock();
        let max = (
            max_width.clamp(MIN_SIDE, MAX_SIDE),
            max_height.clamp(MIN_SIDE, MAX_SIDE),
        );
        s.wants.retain(|w| now < w.until);
        let until = now + WANT_TTL;
        match s.wants.iter_mut().find(|w| w.max == max) {
            Some(w) => w.until = until,
            None => {
                if s.wants.len() >= MAX_WANTS {
                    // 가장 먼저 끝나는 요청을 버린다.
                    s.wants.sort_by_key(|w| w.until);
                    s.wants.remove(0);
                }
                s.wants.push(Want { until, max });
            }
        }
        s.frame.as_ref().filter(|f| f.seq != after).cloned()
    }

    /// 워커: 지금 한 장 만들 차례면 크기 한도(끝나지 않은 요청 중 가로·세로 각각 가장 큰 값)를 돌려주고
    /// 차례를 가져간다(실패해도 다음 간격까지 다시 하지 않는다).
    pub fn take_turn(&self, now: Instant) -> Option<(u32, u32)> {
        let mut s = self.lock();
        let max = s
            .wants
            .iter()
            .filter(|w| now < w.until)
            .map(|w| w.max)
            .reduce(|a, b| (a.0.max(b.0), a.1.max(b.1)))?;
        if s.last_made
            .is_some_and(|t| now.saturating_duration_since(t) < PREVIEW_INTERVAL)
        {
            return None;
        }
        s.last_made = Some(now);
        Some(max)
    }

    pub fn put(&self, width: u32, height: u32, rgba: Vec<u8>) {
        let mut s = self.lock();
        s.seq += 1;
        let seq = s.seq;
        s.frame = Some(Arc::new(PreviewFrame {
            seq,
            width,
            height,
            rgba,
        }));
    }
}

/// `w`×`h`를 `max_w`×`max_h` 안에 비율을 지켜 맞춘다. 키우지 않고, 각 변은 1 이상이다.
pub(crate) fn fit(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (0, 0);
    }
    let scale = (max_w as f64 / w as f64)
        .min(max_h as f64 / h as f64)
        .min(1.0);
    (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    )
}

/// 원본 행을 몇 개씩 건너뛸지(1이면 모두 읽음). 건너뛴 뒤에도 목표 높이의 두 배 이상이 남게 한다.
/// 4:2:0이라 크로마 행은 같은 간격으로 건너뛰면 그대로 맞는다(건너뛴 높이가 짝수가 되게 한다).
pub(crate) fn row_step(src_h: u32, dst_h: u32) -> u32 {
    if dst_h == 0 {
        return 1;
    }
    (src_h / dst_h.saturating_mul(2)).max(1)
}

/// 미리 보기 줄이기(swscale 컨텍스트를 크기가 같으면 다시 쓴다). 비디오 워커 하나에서만 쓴다.
pub(crate) struct PreviewScaler {
    sws: *mut ff::SwsContext,
    key: (u32, u32, i32, u32, u32),
}

// SAFETY: 워커 스레드 하나에서만 쓴다. sws 컨텍스트는 이 값이 소유한다.
unsafe impl Send for PreviewScaler {}

impl Drop for PreviewScaler {
    fn drop(&mut self) {
        if !self.sws.is_null() {
            // SAFETY: sws_getContext로 만든 컨텍스트.
            unsafe { ff::sws_freeContext(self.sws) };
        }
    }
}

impl PreviewScaler {
    pub fn new() -> Self {
        PreviewScaler {
            sws: ptr::null_mut(),
            key: (0, 0, 0, 0, 0),
        }
    }

    /// `src`를 `max` 안에 원본 비율로(키우지 않고) 맞춘 RGBA로 줄인다. 돌려주는 값은 (너비, 높이, RGBA).
    pub fn scale(
        &mut self,
        src: &YuvView<'_>,
        max: (u32, u32),
    ) -> Result<(u32, u32, Vec<u8>), MediaError> {
        let (sw, sh) = (src.width as u32, src.height as u32);
        let (dw, dh) = fit(sw, sh, max.0, max.1);
        if dw == 0 || dh == 0 {
            return Err(MediaError::Graphics("미리 보기 원본 크기가 0입니다".into()));
        }
        // 행 건너뛰기: 건너뛴 높이가 짝수가 되게 한다(4:2:0 크로마 행과 맞추려고).
        let step = row_step(sh, dh) as usize;
        let rows = ((src.height / step) & !1).max(2).min(src.height);
        let fmt = match src.chroma {
            Chroma::Nv12 { .. } => ff::AVPixelFormat::AV_PIX_FMT_NV12,
            Chroma::I420 { .. } => ff::AVPixelFormat::AV_PIX_FMT_YUV420P,
        };
        let key = (sw, rows as u32, fmt as i32, dw, dh);
        if self.sws.is_null() || self.key != key {
            // SAFETY: 이전 컨텍스트를 해제하고 새로 만든다.
            unsafe {
                if !self.sws.is_null() {
                    ff::sws_freeContext(self.sws);
                }
                self.sws = ff::sws_getContext(
                    sw as i32,
                    rows as i32,
                    fmt,
                    dw as i32,
                    dh as i32,
                    ff::AVPixelFormat::AV_PIX_FMT_RGBA,
                    ff::SwsFlags::SWS_BILINEAR as i32,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null(),
                );
            }
            if self.sws.is_null() {
                self.key = (0, 0, 0, 0, 0);
                return Err(MediaError::Graphics(
                    "미리 보기 swscale 컨텍스트를 만들지 못했습니다".into(),
                ));
            }
            self.key = key;
        }
        // 크로마 행은 휘도 두 행에 하나이므로, 휘도를 step 행씩 건너뛰면 크로마도 step 행씩 건너뛴다.
        let (planes, strides): ([*const u8; 4], [i32; 4]) = match &src.chroma {
            Chroma::Nv12 { uv, stride } => (
                [src.y.as_ptr(), uv.as_ptr(), ptr::null(), ptr::null()],
                [(src.y_stride * step) as i32, (*stride * step) as i32, 0, 0],
            ),
            Chroma::I420 {
                u,
                u_stride,
                v,
                v_stride,
            } => (
                [src.y.as_ptr(), u.as_ptr(), v.as_ptr(), ptr::null()],
                [
                    (src.y_stride * step) as i32,
                    (*u_stride * step) as i32,
                    (*v_stride * step) as i32,
                    0,
                ],
            ),
        };
        // 행 간격을 SWS_ALIGN 배수로 하고 끝에 여유를 둔 버퍼로 받은 뒤, 행을 앞으로 모아 dw*4 간격으로 만든다.
        let (row, rows_out) = (dw as usize * 4, dh as usize);
        let stride = row.next_multiple_of(SWS_ALIGN);
        let mut rgba = vec![0u8; stride * rows_out + SWS_ALIGN];
        let dst_planes = [
            rgba.as_mut_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        ];
        let dst_strides = [stride as i32, 0, 0, 0];
        // SAFETY: 원본 평면은 rows 행(행 간격 = 원래 간격 × step)이 모두 슬라이스 안에 있다
        // (rows × step ≤ height, 크로마는 rows/2 × step ≤ height/2). 출력은 행 간격 stride로 dh행에
        // SWS_ALIGN만큼 여유가 더 있다.
        let out = unsafe {
            ff::sws_scale(
                self.sws,
                planes.as_ptr(),
                strides.as_ptr(),
                0,
                rows as i32,
                dst_planes.as_ptr(),
                dst_strides.as_ptr(),
            )
        };
        if out <= 0 {
            return Err(MediaError::Graphics("미리 보기 swscale 실패".into()));
        }
        if stride != row {
            for r in 1..rows_out {
                rgba.copy_within(r * stride..r * stride + row, r * row);
            }
        }
        rgba.truncate(row * rows_out);
        Ok((dw, dh, rgba))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yuv::Yuv;

    struct Buf {
        w: usize,
        h: usize,
        y: Vec<u8>,
        uv: Vec<u8>,
    }

    impl Buf {
        fn new(w: usize, h: usize) -> Self {
            Buf {
                w,
                h,
                y: vec![16; w * h],
                uv: vec![128; w * h / 2],
            }
        }

        fn view(&mut self) -> YuvView<'_> {
            YuvView {
                width: self.w,
                height: self.h,
                y: &mut self.y,
                y_stride: self.w,
                chroma: Chroma::Nv12 {
                    uv: &mut self.uv,
                    stride: self.w,
                },
            }
        }
    }

    fn px(rgba: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    #[test]
    fn fit_keeps_ratio_and_never_grows() {
        assert_eq!(fit(1920, 1080, 480, 270), (480, 270));
        assert_eq!(fit(1920, 1080, 640, 270), (480, 270));
        assert_eq!(fit(1280, 720, 1920, 1080), (1280, 720));
        // 아주 납작한 창(182×26)은 너비에 맞춘다.
        assert_eq!(fit(182, 26, 480, 270), (182, 26));
        assert_eq!(fit(1000, 10, 100, 100), (100, 1));
        assert_eq!(fit(0, 10, 100, 100), (0, 0));
    }

    #[test]
    fn row_step_leaves_twice_the_target() {
        assert_eq!(row_step(1080, 270), 2);
        assert_eq!(row_step(2160, 360), 3);
        assert_eq!(row_step(720, 360), 1);
        assert_eq!(row_step(100, 270), 1);
        assert_eq!(row_step(100, 0), 1);
    }

    #[test]
    fn slot_makes_frames_only_while_wanted_and_at_interval() {
        let slot = PreviewSlot::default();
        let t0 = Instant::now();
        // 아무도 부르지 않았으면 만들지 않는다.
        assert_eq!(slot.take_turn(t0), None);
        assert!(slot.request(t0, 0, 480, 270).is_none());
        assert_eq!(slot.take_turn(t0), Some((480, 270)));
        // 간격 안에서는 다시 만들지 않는다.
        assert_eq!(slot.take_turn(t0 + Duration::from_millis(50)), None);
        slot.put(2, 2, vec![0; 16]);
        let f = slot.request(t0, 0, 480, 270).expect("새 그림");
        assert_eq!((f.seq, f.width, f.height), (1, 2, 2));
        // 같은 번호면 다시 주지 않는다.
        assert!(slot.request(t0, 1, 480, 270).is_none());
        // 다른 번호(예: 새 방송에서 번호가 처음부터)면 준다.
        assert!(slot.request(t0, 7, 480, 270).is_some());
        assert_eq!(slot.take_turn(t0 + PREVIEW_INTERVAL), Some((480, 270)));
        // 요청이 끊기고 WANT_TTL이 지나면 만들지 않는다.
        assert_eq!(slot.take_turn(t0 + WANT_TTL + PREVIEW_INTERVAL), None);
        // 크기 한도는 범위 안으로.
        let slot = PreviewSlot::default();
        slot.request(t0, 1, 100_000, 0);
        assert_eq!(
            slot.take_turn(t0 + Duration::from_millis(500)),
            Some((MAX_SIDE, MIN_SIDE))
        );
    }

    #[test]
    fn slot_uses_the_largest_of_live_requests() {
        let slot = PreviewSlot::default();
        let t0 = Instant::now();
        // 화면 카드(400×225)와 썸네일(480×270)이 같이 부르면 큰 쪽으로 만든다.
        slot.request(t0, 0, 400, 225);
        slot.request(t0 + Duration::from_millis(10), 0, 480, 270);
        assert_eq!(
            slot.take_turn(t0 + Duration::from_millis(20)),
            Some((480, 270))
        );
        // 가로·세로를 따로 본다(세로로 긴 요청과 섞이면 둘 다 담는 상자).
        slot.request(t0 + Duration::from_millis(30), 0, 200, 400);
        assert_eq!(
            slot.take_turn(t0 + Duration::from_millis(30) + PREVIEW_INTERVAL),
            Some((480, 400))
        );
        // 썸네일 요청이 끝나면(1초 동안 다시 부르지 않음) 카드 크기로 돌아온다.
        let later = t0 + WANT_TTL + Duration::from_millis(40);
        slot.request(later, 0, 400, 225);
        assert_eq!(slot.take_turn(later), Some((400, 225)));
        // 같은 크기로 여러 번 부르면 하나로 센다.
        for i in 0..10 {
            slot.request(later + Duration::from_millis(i), 0, 400, 225);
        }
        assert_eq!(slot.lock().wants.len(), 1);
        // 크기가 다른 요청이 많아도 MAX_WANTS개까지만 기억한다.
        for i in 0..10u32 {
            slot.request(later, 0, 100 + i, 100);
        }
        assert_eq!(slot.lock().wants.len(), MAX_WANTS);
    }

    #[test]
    fn scales_nv12_to_rgba_with_colors() {
        // 1920×1080: 왼쪽 절반 흰색, 오른쪽 절반 검정 → 480×270
        let mut b = Buf::new(1920, 1080);
        {
            let mut v = b.view();
            v.fill(Yuv::BLACK);
            v.fill_rect(0, 0, 960, 1080, Yuv::WHITE);
        }
        let mut s = PreviewScaler::new();
        let (w, h, rgba) = s.scale(&b.view(), (480, 270)).unwrap();
        assert_eq!((w, h), (480, 270));
        assert_eq!(rgba.len(), 480 * 270 * 4);
        let white = px(&rgba, w, 100, 135);
        let black = px(&rgba, w, 380, 135);
        assert!(white[..3].iter().all(|&c| c >= 245), "{white:?}");
        assert!(black[..3].iter().all(|&c| c <= 10), "{black:?}");
        assert_eq!(white[3], 255);
        // 같은 크기는 컨텍스트를 다시 쓴다.
        let key = s.key;
        s.scale(&b.view(), (480, 270)).unwrap();
        assert_eq!(s.key, key);
    }

    #[test]
    fn scales_i420_and_odd_targets() {
        let (w, h) = (640usize, 360usize);
        let mut y = vec![16u8; w * h];
        let mut u = vec![128u8; w * h / 4];
        let mut v = vec![128u8; w * h / 4];
        let mut view = YuvView {
            width: w,
            height: h,
            y: &mut y,
            y_stride: w,
            chroma: Chroma::I420 {
                u: &mut u,
                u_stride: w / 2,
                v: &mut v,
                v_stride: w / 2,
            },
        };
        view.fill(Yuv::WHITE);
        let mut s = PreviewScaler::new();
        let (dw, dh, rgba) = s.scale(&view, (333, 333)).unwrap();
        assert_eq!((dw, dh), (333, 187));
        assert!(px(&rgba, dw, 166, 93)[..3].iter().all(|&c| c >= 245));
    }

    /// 줄이지 않는 작은 I420 원본(너비가 16의 배수가 아님). 예전에는 swscale이 딱 맞는 출력 버퍼 끝을 넘어 써서
    /// 2×2에서 힙이 깨져 시험 프로세스가 STATUS_HEAP_CORRUPTION으로 죽었다. 행을 모은 결과도 확인한다.
    #[test]
    fn small_i420_sources_stay_in_bounds() {
        for (w, h) in [(2usize, 2usize), (14, 2), (30, 4), (64, 64), (558, 314)] {
            let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
            let mut y = vec![16u8; w * h];
            let mut u = vec![128u8; cw * ch];
            let mut v = vec![128u8; cw * ch];
            let mut view = YuvView {
                width: w,
                height: h,
                y: &mut y,
                y_stride: w,
                chroma: Chroma::I420 {
                    u: &mut u,
                    u_stride: cw,
                    v: &mut v,
                    v_stride: cw,
                },
            };
            view.fill(Yuv::BLACK);
            view.fill_rect(0, 0, w / 2, h, Yuv::WHITE);
            let mut s = PreviewScaler::new();
            for _ in 0..3 {
                let (dw, dh, rgba) = s.scale(&view, (640, 360)).unwrap();
                assert_eq!((dw as usize, dh as usize), (w, h));
                assert_eq!(rgba.len(), w * h * 4);
                for row in 0..dh {
                    let white = px(&rgba, dw, 0, row);
                    let black = px(&rgba, dw, dw - 1, row);
                    assert!(
                        white[..3].iter().all(|&c| c >= 245),
                        "{w}x{h} {row}: {white:?}"
                    );
                    assert!(
                        black[..3].iter().all(|&c| c <= 10),
                        "{w}x{h} {row}: {black:?}"
                    );
                    assert_eq!(white[3], 255);
                }
            }
        }
    }

    /// 워커가 쓰는 시간(참고용). `cargo test -p dongsi-media preview -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_scale() {
        for (sw, sh, mw, mh) in [
            (1280, 720, 560, 315),
            (1920, 1080, 560, 315),
            (2560, 1440, 560, 315),
            (3840, 2160, 560, 315),
            (3840, 2160, 640, 360),
            (3840, 2160, 1280, 720),
        ] {
            let mut b = Buf::new(sw, sh);
            for (i, p) in b.y.iter_mut().enumerate() {
                *p = (i % 251) as u8;
            }
            let mut s = PreviewScaler::new();
            s.scale(&b.view(), (mw, mh)).unwrap();
            let n = 50;
            let t = Instant::now();
            for _ in 0..n {
                s.scale(&b.view(), (mw, mh)).unwrap();
            }
            let ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
            eprintln!("{sw}x{sh} -> {mw}x{mh}: {ms:.2} ms");
        }
    }
}
