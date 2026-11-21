//! 시작 창: 앱을 실행하자마자 화면 가운데에 띄우는 작은 로딩 창(로고·이름·진행 막대).
//!
//! WebView2가 뜨는 동안(0.8~1.4초, 2026-11-21 release 빌드 측정) 메인 창은 아무것도 그릴 수 없다.
//! 그래서 메인 창은 숨겨서 만들고(`tauri.conf.json`의 `visible: false`), 이 창을 Win32·Direct2D로 직접 그린다.
//! 프런트엔드가 첫 화면을 준비하면 `app_ready`를 불러 메인 창을 보이고 이 창을 닫는다.
//! 프런트엔드가 끝내 부르지 못해도 [`FALLBACK`] 뒤에는 메인 창을 보인다.
//! 모양은 `app.html`의 시작 화면(`#boot`)과 같다(색은 `app.css` 토큰).

use std::time::Duration;

use tauri::{AppHandle, Manager, WebviewWindow};

/// 프런트엔드가 `app_ready`를 부르지 못해도 이 시간이 지나면 메인 창을 보인다.
pub const FALLBACK: Duration = Duration::from_secs(10);

/// 진행 막대의 한 바퀴(ms). `app.html`의 `boot-slide`와 같다.
const BAR_PERIOD_MS: u128 = 1100;

/// 프런트엔드가 첫 화면을 준비했다(`+layout.svelte`). 메인 창이 부른 것이면 메인 창을 보이고 시작 창을 닫는다.
#[tauri::command]
pub fn app_ready(window: WebviewWindow) {
    if window.label() == "main" {
        show_main_window(window.app_handle());
    }
}

/// 메인 창이 숨어 있으면 보이고, 시작 창을 닫는다. 여러 번 불러도 된다.
pub fn show_main_window(app: &AppHandle) {
    if let Some(main) = app.get_webview_window("main")
        && !main.is_visible().unwrap_or(true)
    {
        let _ = main.show();
        let _ = main.set_focus();
    }
    close();
}

/// 진행 막대에서 움직이는 조각의 왼쪽 끝(조각 너비 단위). 0이면 막대 왼쪽 끝에 붙는다.
/// CSS `translateX(-100%) → translateX(250%)`, ease-in-out과 같다.
fn bar_segment_offset(elapsed_ms: u128) -> f32 {
    let t = (elapsed_ms % BAR_PERIOD_MS) as f32 / BAR_PERIOD_MS as f32;
    let eased = t * t * (3.0 - 2.0 * t);
    -1.0 + 3.5 * eased
}

#[cfg(windows)]
pub use imp::{close, show};

#[cfg(not(windows))]
pub fn show() {}

#[cfg(not(windows))]
pub fn close() {}

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::time::Instant;

    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Direct2D::Common::{
        D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED,
        D2D1_FIGURE_END_CLOSED, D2D1_PIXEL_FORMAT,
    };
    use windows::Win32::Graphics::Direct2D::{
        D2D1_ANTIALIAS_MODE_ALIASED, D2D1_DRAW_TEXT_OPTIONS_NONE,
        D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
        D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES, D2D1_ROUNDED_RECT,
        D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1PathGeometry,
    };
    use windows::Win32::Graphics::DirectWrite::{
        DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
        DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_METRICS, DWriteCreateFactory, IDWriteFactory,
        IDWriteTextLayout,
    };
    use windows::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, EndPaint, GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTOPRIMARY,
        MONITORINFO, MonitorFromPoint, PAINTSTRUCT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
        SetThreadDpiAwarenessContext,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
        IDC_APPSTARTING, KillTimer, LoadCursorW, MSG, PostMessageW, PostQuitMessage,
        RegisterClassExW, SW_SHOWNOACTIVATE, SetTimer, ShowWindow, TranslateMessage, WM_CLOSE,
        WM_DESTROY, WM_ERASEBKGND, WM_PAINT, WM_TIMER, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_POPUP,
    };
    use windows::core::{Result, w};
    use windows_numerics::Vector2;

    use super::bar_segment_offset;

    /// 창 크기(DIP). 모양은 `app.html`의 `#boot`를 줄인 것이다.
    const WIDTH: f32 = 320.0;
    const HEIGHT: f32 = 184.0;
    /// 로고·이름 줄의 세로 가운데, 진행 막대의 위쪽(DIP)
    const BRAND_Y: f32 = 80.0;
    const BAR_Y: f32 = 117.0;
    const BAR_W: f32 = 112.0;
    const LOGO: f32 = 30.0;
    const GAP: f32 = 11.0;
    const TIMER_ID: usize = 1;

    const BG: D2D1_COLOR_F = rgb(0x0d, 0x0e, 0x11, 1.0); // --bg
    const TEXT: D2D1_COLOR_F = rgb(0xf3, 0xf4, 0xf6, 1.0); // --text
    const TEXT_2: D2D1_COLOR_F = rgb(0xa9, 0xab, 0xb3, 1.0); // --text-2
    const FILL: D2D1_COLOR_F = rgb(0xff, 0xff, 0xff, 0.07); // --fill
    /// 테두리: --line-strong(흰색 12%)을 --bg 위에 섞은 색. COLORREF는 0x00BBGGRR.
    const BORDER: COLORREF = COLORREF(0x002e_2b2a);

    const fn rgb(r: u8, g: u8, b: u8, a: f32) -> D2D1_COLOR_F {
        D2D1_COLOR_F {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a,
        }
    }

    /// 시작 창 HWND(0이면 없음). 다른 스레드에서 닫을 때 쓴다.
    static HWND_VALUE: AtomicIsize = AtomicIsize::new(0);
    /// 창이 만들어지기 전에 닫으라고 했는가
    static CLOSE_REQUESTED: AtomicBool = AtomicBool::new(false);

    thread_local! {
        static PAINTER: RefCell<Option<Painter>> = const { RefCell::new(None) };
    }

    /// 시작 창을 자기 스레드에 띄운다. 실패하면 로그만 남긴다(메인 창은 그대로 뜬다).
    pub fn show() {
        let spawned = std::thread::Builder::new().name("splash".into()).spawn(|| {
            if let Err(e) = run() {
                tracing::warn!("시작 창을 띄우지 못했습니다: {e}");
            }
            HWND_VALUE.store(0, Ordering::SeqCst);
        });
        if let Err(e) = spawned {
            tracing::warn!("시작 창 스레드를 만들지 못했습니다: {e}");
        }
    }

    /// 시작 창을 닫는다. 어느 스레드에서 불러도 되고, 여러 번 불러도 된다.
    pub fn close() {
        CLOSE_REQUESTED.store(true, Ordering::SeqCst);
        let raw = HWND_VALUE.load(Ordering::SeqCst);
        if raw != 0 {
            // SAFETY: 창 스레드가 살아 있는 동안만 0이 아니다. 이미 사라진 창이면 PostMessageW가 실패할 뿐이다.
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(raw as *mut c_void)),
                    WM_CLOSE,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }

    fn run() -> Result<()> {
        // SAFETY: 이 스레드가 만든 창에만 쓰는 Win32 호출이다. 핸들은 이 함수와 창 프로시저 밖으로 나가지 않는다
        // (HWND_VALUE는 PostMessageW용 값이다).
        unsafe {
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let instance = GetModuleHandleW(None)?;
            let class = w!("DongsiSplash");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                hCursor: LoadCursorW(None, IDC_APPSTARTING)?,
                lpszClassName: class,
                ..Default::default()
            };
            if RegisterClassExW(&wc) == 0 {
                return Err(windows::core::Error::from_thread());
            }

            // 주 모니터 작업 영역 가운데(메인 창도 주 모니터 가운데에 뜬다).
            let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(monitor, &mut info);
            let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            let scale = dpi_x as f32 / 96.0;
            let (w, h) = (
                (WIDTH * scale).round() as i32,
                (HEIGHT * scale).round() as i32,
            );
            let work: RECT = info.rcWork;
            let x = work.left + (work.right - work.left - w) / 2;
            let y = work.top + (work.bottom - work.top - h) / 2;

            if CLOSE_REQUESTED.load(Ordering::SeqCst) {
                return Ok(());
            }
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class,
                w!("dongsi"),
                WS_POPUP,
                x,
                y,
                w,
                h,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            let corner = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corner as *const _ as *const c_void,
                std::mem::size_of_val(&corner) as u32,
            );
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_BORDER_COLOR,
                &BORDER as *const _ as *const c_void,
                std::mem::size_of::<COLORREF>() as u32,
            );

            let painter = Painter::new(hwnd, dpi_x as f32, w as u32, h as u32)?;
            PAINTER.with(|p| *p.borrow_mut() = Some(painter));
            HWND_VALUE.store(hwnd.0 as isize, Ordering::SeqCst);
            if CLOSE_REQUESTED.load(Ordering::SeqCst) {
                let _ = DestroyWindow(hwnd);
            } else {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                SetTimer(Some(hwnd), TIMER_ID, 16, None);
            }

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            PAINTER.with(|p| p.borrow_mut().take());
        }
        Ok(())
    }

    extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        // SAFETY: 이 스레드의 창 프로시저. 넘겨받은 hwnd에만 쓴다.
        unsafe {
            match msg {
                WM_ERASEBKGND => LRESULT(1),
                WM_TIMER => {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    LRESULT(0)
                }
                WM_PAINT => {
                    let mut ps = PAINTSTRUCT::default();
                    BeginPaint(hwnd, &mut ps);
                    PAINTER.with(|p| {
                        if let Some(painter) = p.borrow_mut().as_mut() {
                            painter.paint();
                        }
                    });
                    let _ = EndPaint(hwnd, &ps);
                    LRESULT(0)
                }
                WM_CLOSE => {
                    let _ = DestroyWindow(hwnd);
                    LRESULT(0)
                }
                WM_DESTROY => {
                    let _ = KillTimer(Some(hwnd), TIMER_ID);
                    HWND_VALUE.store(0, Ordering::SeqCst);
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
    }

    struct Painter {
        hwnd: HWND,
        dpi: f32,
        size: D2D_SIZE_U,
        factory: ID2D1Factory,
        target: Option<ID2D1HwndRenderTarget>,
        name: IDWriteTextLayout,
        /// 로고 왼쪽 끝(DIP). 로고 + 간격 + 이름을 가운데에 둔다.
        left: f32,
        name_height: f32,
        play: ID2D1PathGeometry,
        start: Instant,
    }

    impl Painter {
        unsafe fn new(hwnd: HWND, dpi: f32, width: u32, height: u32) -> Result<Self> {
            unsafe {
                let factory: ID2D1Factory =
                    D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
                let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
                let format = dwrite.CreateTextFormat(
                    w!("Segoe UI Variable Display"),
                    None,
                    DWRITE_FONT_WEIGHT_SEMI_BOLD,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    21.0,
                    w!("en-us"),
                )?;
                let text: Vec<u16> = "dongsi".encode_utf16().collect();
                let name = dwrite.CreateTextLayout(&text, &format, WIDTH, HEIGHT)?;
                let mut metrics = DWRITE_TEXT_METRICS::default();
                name.GetMetrics(&mut metrics)?;

                let left = (WIDTH - (LOGO + GAP + metrics.width)) / 2.0;

                // 재생 삼각형(icons.ts의 play: M7 4.5v15l12.5-7.5z, 24 단위)을 로고 가운데 15 DIP로.
                let play = factory.CreatePathGeometry()?;
                let sink = play.Open()?;
                let k = 15.0 / 24.0;
                let (ox, oy) = (left + (LOGO - 15.0) / 2.0, BRAND_Y - 15.0 / 2.0);
                let pt = |x: f32, y: f32| Vector2 {
                    X: ox + x * k,
                    Y: oy + y * k,
                };
                sink.BeginFigure(pt(7.0, 4.5), D2D1_FIGURE_BEGIN_FILLED);
                sink.AddLines(&[pt(7.0, 19.5), pt(19.5, 12.0)]);
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                sink.Close()?;

                Ok(Self {
                    hwnd,
                    dpi,
                    size: D2D_SIZE_U { width, height },
                    factory,
                    target: None,
                    name,
                    left,
                    name_height: metrics.height,
                    play,
                    start: Instant::now(),
                })
            }
        }

        fn paint(&mut self) {
            // SAFETY: 이 창 스레드에서만 부른다(WM_PAINT).
            if let Err(e) = unsafe { self.draw() } {
                // 장치가 바뀌었으면(D2DERR_RECREATE_TARGET 등) 다음 그리기에서 다시 만든다.
                tracing::debug!("시작 창 그리기 실패: {e}");
                self.target = None;
            }
        }

        unsafe fn draw(&mut self) -> Result<()> {
            unsafe {
                if self.target.is_none() {
                    let props = D2D1_RENDER_TARGET_PROPERTIES {
                        pixelFormat: D2D1_PIXEL_FORMAT {
                            format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            alphaMode: D2D1_ALPHA_MODE_IGNORE,
                        },
                        dpiX: self.dpi,
                        dpiY: self.dpi,
                        ..Default::default()
                    };
                    let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                        hwnd: self.hwnd,
                        pixelSize: self.size,
                        presentOptions: D2D1_PRESENT_OPTIONS_NONE,
                    };
                    self.target = Some(self.factory.CreateHwndRenderTarget(&props, &hwnd_props)?);
                }
                let Some(rt) = self.target.as_ref() else {
                    return Ok(());
                };
                let text = rt.CreateSolidColorBrush(&TEXT, None)?;
                let bg = rt.CreateSolidColorBrush(&BG, None)?;
                let track = rt.CreateSolidColorBrush(&FILL, None)?;
                let segment = rt.CreateSolidColorBrush(&TEXT_2, None)?;

                rt.BeginDraw();
                rt.Clear(Some(&BG));

                // 로고 + 이름(가운데 정렬)
                let left = self.left;
                let logo_top = BRAND_Y - LOGO / 2.0;
                let logo = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left,
                        top: logo_top,
                        right: left + LOGO,
                        bottom: logo_top + LOGO,
                    },
                    radiusX: 8.0,
                    radiusY: 8.0,
                };
                rt.FillRoundedRectangle(&logo, &text);
                rt.FillGeometry(&self.play, &bg, None);
                rt.DrawTextLayout(
                    Vector2 {
                        X: left + LOGO + GAP,
                        Y: BRAND_Y - self.name_height / 2.0,
                    },
                    &self.name,
                    &text,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );

                // 진행 막대
                let bar_left = (WIDTH - BAR_W) / 2.0;
                let bar = D2D_RECT_F {
                    left: bar_left,
                    top: BAR_Y,
                    right: bar_left + BAR_W,
                    bottom: BAR_Y + 2.0,
                };
                rt.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: bar,
                        radiusX: 1.0,
                        radiusY: 1.0,
                    },
                    &track,
                );
                let seg_w = BAR_W * 0.4;
                let seg_left =
                    bar_left + bar_segment_offset(self.start.elapsed().as_millis()) * seg_w;
                rt.PushAxisAlignedClip(&bar, D2D1_ANTIALIAS_MODE_ALIASED);
                rt.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: seg_left,
                            top: BAR_Y,
                            right: seg_left + seg_w,
                            bottom: BAR_Y + 2.0,
                        },
                        radiusX: 1.0,
                        radiusY: 1.0,
                    },
                    &segment,
                );
                rt.PopAxisAlignedClip();
                rt.EndDraw(None, None)?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 진행_막대_조각은_왼쪽_밖에서_오른쪽_밖까지_움직인다() {
        assert_eq!(bar_segment_offset(0), -1.0);
        assert!((bar_segment_offset(BAR_PERIOD_MS / 2) - 0.75).abs() < 1e-6);
        assert!(bar_segment_offset(BAR_PERIOD_MS - 1) > 2.4);
        // 한 바퀴 뒤에는 처음으로 돌아온다.
        assert_eq!(bar_segment_offset(BAR_PERIOD_MS), -1.0);
    }
}
