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
        D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_BEZIER_SEGMENT, D2D1_COLOR_F,
        D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED, D2D1_GRADIENT_STOP, D2D1_PIXEL_FORMAT,
    };
    use windows::Win32::Graphics::Direct2D::{
        D2D1_ANTIALIAS_MODE_ALIASED, D2D1_CAP_STYLE_ROUND, D2D1_DRAW_TEXT_OPTIONS_NONE,
        D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_GAMMA_2_2,
        D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_LINE_JOIN_ROUND,
        D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE,
        D2D1_RENDER_TARGET_PROPERTIES, D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES,
        D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1LinearGradientBrush,
        ID2D1PathGeometry, ID2D1RenderTarget, ID2D1SolidColorBrush, ID2D1StrokeStyle,
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
    use windows_numerics::{Matrix3x2, Vector2};

    use super::bar_segment_offset;

    /// 창 크기(DIP). 모양은 `app.html`의 `#boot`를 줄인 것이다.
    const WIDTH: f32 = 320.0;
    const HEIGHT: f32 = 184.0;
    /// 로고·이름 줄의 세로 가운데, 진행 막대의 위쪽(DIP)
    const BRAND_Y: f32 = 80.0;
    const BAR_Y: f32 = 117.0;
    const BAR_W: f32 = 112.0;
    /// 로고 '화면 눈'의 너비·높이(DIP). 높이는 [`ART`] 비율을 따른다.
    const LOGO_W: f32 = 38.0;
    const LOGO_H: f32 = LOGO_W * (ART.bottom - ART.top) / (ART.right - ART.left);
    const GAP: f32 = 10.0;
    /// icon.svg(1024 단위)에서 그림이 실제로 차지하는 범위
    const ART: D2D_RECT_F = D2D_RECT_F {
        left: 40.0,
        top: 116.0,
        right: 984.0,
        bottom: 850.0,
    };
    const TIMER_ID: usize = 1;

    const BG: D2D1_COLOR_F = rgb(0x0d, 0x0e, 0x11, 1.0); // --bg
    const TEXT: D2D1_COLOR_F = rgb(0xf3, 0xf4, 0xf6, 1.0); // --text
    const TEXT_2: D2D1_COLOR_F = rgb(0xa9, 0xab, 0xb3, 1.0); // --text-2
    const FILL: D2D1_COLOR_F = rgb(0xff, 0xff, 0xff, 0.07); // --fill
    // 로고 색(icon.svg에서 옮김)
    const EYE_TOP: D2D1_COLOR_F = rgb(0x8e, 0x5c, 0xff, 1.0);
    const EYE_BOTTOM: D2D1_COLOR_F = rgb(0x6a, 0x2e, 0xe0, 1.0);
    const LASH: D2D1_COLOR_F = rgb(0x83, 0x57, 0xfc, 1.0);
    const LIME: D2D1_COLOR_F = rgb(0xb8, 0xf2, 0x3a, 1.0);
    const PUPIL: D2D1_COLOR_F = rgb(0x3f, 0x15, 0x90, 1.0);
    const GLINT: D2D1_COLOR_F = rgb(0xff, 0xff, 0xff, 1.0);
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
        /// render target과 거기에 묶인 브러시. 장치가 바뀌면 함께 버리고 다시 만든다.
        target: Option<(ID2D1HwndRenderTarget, Brushes)>,
        scene: Scene,
        start: Instant,
    }

    impl Painter {
        unsafe fn new(hwnd: HWND, dpi: f32, width: u32, height: u32) -> Result<Self> {
            unsafe {
                let factory: ID2D1Factory =
                    D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
                let scene = Scene::new(&factory)?;

                Ok(Self {
                    hwnd,
                    dpi,
                    size: D2D_SIZE_U { width, height },
                    factory,
                    target: None,
                    scene,
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
                    let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                        hwnd: self.hwnd,
                        pixelSize: self.size,
                        presentOptions: D2D1_PRESENT_OPTIONS_NONE,
                    };
                    let rt = self
                        .factory
                        .CreateHwndRenderTarget(&target_props(self.dpi), &hwnd_props)?;
                    let brushes = Brushes::new(&rt)?;
                    self.target = Some((rt, brushes));
                }
                let Some((rt, brushes)) = self.target.as_ref() else {
                    return Ok(());
                };
                self.scene
                    .draw(rt, brushes, self.start.elapsed().as_millis())
            }
        }
    }

    fn target_props(dpi: f32) -> D2D1_RENDER_TARGET_PROPERTIES {
        D2D1_RENDER_TARGET_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: dpi,
            dpiY: dpi,
            ..Default::default()
        }
    }

    /// render target마다 만드는 장치 의존 자원
    struct Brushes {
        text: ID2D1SolidColorBrush,
        track: ID2D1SolidColorBrush,
        segment: ID2D1SolidColorBrush,
        eye: ID2D1LinearGradientBrush,
        lash: ID2D1SolidColorBrush,
        lime: ID2D1SolidColorBrush,
        pupil: ID2D1SolidColorBrush,
        glint: ID2D1SolidColorBrush,
    }

    impl Brushes {
        unsafe fn new(rt: &ID2D1RenderTarget) -> Result<Self> {
            unsafe {
                let solid = |color: &D2D1_COLOR_F| rt.CreateSolidColorBrush(color, None);
                let stops = rt.CreateGradientStopCollection(
                    &[
                        D2D1_GRADIENT_STOP {
                            position: 0.0,
                            color: EYE_TOP,
                        },
                        D2D1_GRADIENT_STOP {
                            position: 1.0,
                            color: EYE_BOTTOM,
                        },
                    ],
                    D2D1_GAMMA_2_2,
                    D2D1_EXTEND_MODE_CLAMP,
                )?;
                // 세로 그라데이션: 눈 외곽 path의 위(y 260)에서 아래(y 830)까지(1024 단위).
                let eye = rt.CreateLinearGradientBrush(
                    &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                        startPoint: point(0.0, 260.0),
                        endPoint: point(0.0, 830.0),
                    },
                    None,
                    &stops,
                )?;
                Ok(Self {
                    text: solid(&TEXT)?,
                    track: solid(&FILL)?,
                    segment: solid(&TEXT_2)?,
                    eye,
                    lash: solid(&LASH)?,
                    lime: solid(&LIME)?,
                    pupil: solid(&PUPIL)?,
                    glint: solid(&GLINT)?,
                })
            }
        }
    }

    const fn point(x: f32, y: f32) -> Vector2 {
        Vector2 { X: x, Y: y }
    }

    /// 창에 그리는 것 중 render target과 무관한 것(factory 자원, 글자 배치). 한 번만 만든다.
    struct Scene {
        name: IDWriteTextLayout,
        /// 로고 왼쪽 끝(DIP). 로고 + 간격 + 이름을 가운데에 둔다.
        left: f32,
        name_height: f32,
        /// 눈 외곽(1024 단위)
        eye: ID2D1PathGeometry,
        /// 속눈썹의 둥근 끝, 눈 외곽선의 둥근 이음
        round: ID2D1StrokeStyle,
    }

    impl Scene {
        unsafe fn new(factory: &ID2D1Factory) -> Result<Self> {
            unsafe {
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

                let left = (WIDTH - (LOGO_W + GAP + metrics.width)) / 2.0;

                // 눈 외곽: M60 560 C 230 160, 794 160, 964 560 C 794 920, 230 920, 60 560 Z (icon.svg에서 옮김)
                let eye = factory.CreatePathGeometry()?;
                let sink = eye.Open()?;
                sink.BeginFigure(point(60.0, 560.0), D2D1_FIGURE_BEGIN_FILLED);
                sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                    point1: point(230.0, 160.0),
                    point2: point(794.0, 160.0),
                    point3: point(964.0, 560.0),
                });
                sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                    point1: point(794.0, 920.0),
                    point2: point(230.0, 920.0),
                    point3: point(60.0, 560.0),
                });
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                sink.Close()?;

                let round = factory.CreateStrokeStyle(
                    &D2D1_STROKE_STYLE_PROPERTIES {
                        startCap: D2D1_CAP_STYLE_ROUND,
                        endCap: D2D1_CAP_STYLE_ROUND,
                        lineJoin: D2D1_LINE_JOIN_ROUND,
                        miterLimit: 10.0,
                        ..Default::default()
                    },
                    None,
                )?;

                Ok(Self {
                    name,
                    left,
                    name_height: metrics.height,
                    eye,
                    round,
                })
            }
        }

        /// 한 장면을 그린다. render target 종류와 상관없다(창, 시험의 오프스크린 DC).
        unsafe fn draw(
            &self,
            rt: &ID2D1RenderTarget,
            brushes: &Brushes,
            elapsed_ms: u128,
        ) -> Result<()> {
            unsafe {
                rt.BeginDraw();
                rt.Clear(Some(&BG));

                // 로고 + 이름(가운데 정렬)
                self.draw_logo(rt, brushes, self.left, BRAND_Y - LOGO_H / 2.0);
                rt.DrawTextLayout(
                    Vector2 {
                        X: self.left + LOGO_W + GAP,
                        Y: BRAND_Y - self.name_height / 2.0,
                    },
                    &self.name,
                    &brushes.text,
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
                    &brushes.track,
                );
                let seg_w = BAR_W * 0.4;
                let seg_left = bar_left + bar_segment_offset(elapsed_ms) * seg_w;
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
                    &brushes.segment,
                );
                rt.PopAxisAlignedClip();
                rt.EndDraw(None, None)
            }
        }

        /// 로고 '화면 눈'을 (left, top)(DIP)에 그린다. 모양·색·좌표는 icon.svg에서 옮김.
        unsafe fn draw_logo(&self, rt: &ID2D1RenderTarget, brushes: &Brushes, left: f32, top: f32) {
            unsafe {
                // 아래 좌표와 선 굵기는 모두 1024 단위이고, DIP로 옮기는 변환은 여기 한 곳이다.
                rt.SetTransform(&logo_transform(left, top));

                // 속눈썹 3줄
                for (from, to) in [
                    (point(296.0, 325.0), point(249.0, 212.0)),
                    (point(512.0, 276.0), point(512.0, 152.0)),
                    (point(728.0, 325.0), point(775.0, 212.0)),
                ] {
                    rt.DrawLine(from, to, &brushes.lash, 72.0, &self.round);
                }
                // 눈
                rt.FillGeometry(&self.eye, &brushes.eye, None);
                rt.DrawGeometry(&self.eye, &brushes.eye, 40.0, &self.round);
                // 라임 화면, 동공, 하이라이트
                rt.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: 262.0,
                            top: 380.0,
                            right: 762.0,
                            bottom: 720.0,
                        },
                        radiusX: 92.0,
                        radiusY: 92.0,
                    },
                    &brushes.lime,
                );
                let circle = |x: f32, y: f32, r: f32| D2D1_ELLIPSE {
                    point: point(x, y),
                    radiusX: r,
                    radiusY: r,
                };
                rt.FillEllipse(&circle(512.0, 550.0, 106.0), &brushes.pupil);
                rt.FillEllipse(&circle(468.0, 504.0, 36.0), &brushes.glint);

                rt.SetTransform(&Matrix3x2 {
                    M11: 1.0,
                    M12: 0.0,
                    M21: 0.0,
                    M22: 1.0,
                    M31: 0.0,
                    M32: 0.0,
                });
            }
        }
    }

    /// icon.svg 좌표(1024 단위)의 [`ART`] 범위를 (left, top)에서 시작하는 [`LOGO_W`] 너비(DIP)로 옮긴다.
    fn logo_transform(left: f32, top: f32) -> Matrix3x2 {
        let k = LOGO_W / (ART.right - ART.left);
        Matrix3x2 {
            M11: k,
            M12: 0.0,
            M21: 0.0,
            M22: k,
            M31: left - ART.left * k,
            M32: top - ART.top * k,
        }
    }

    #[cfg(test)]
    mod tests {
        use std::path::Path;

        use windows::Win32::Graphics::Gdi::{
            BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
            DeleteDC, DeleteObject, GdiFlush, SelectObject,
        };

        use super::super::BAR_PERIOD_MS;
        use super::*;

        /// 창 없이 GDI 메모리 DC에 한 장면을 그린 결과(BGRA, 위에서 아래로)
        struct Frame {
            width: i32,
            height: i32,
            pixels: Vec<u8>,
        }

        fn header(width: i32, height: i32) -> BITMAPINFOHEADER {
            BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // 음수면 위에서 아래로 저장한다.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            }
        }

        fn render(factory: &ID2D1Factory, scene: &Scene, scale: f32) -> Result<Frame> {
            let (width, height) = ((WIDTH * scale) as i32, (HEIGHT * scale) as i32);
            // SAFETY: 이 함수에서 만든 DC·비트맵만 쓰고 끝에 지운다. bits는 비트맵이 사는 동안 width*height*4바이트다.
            unsafe {
                let dc = CreateCompatibleDC(None);
                let info = BITMAPINFO {
                    bmiHeader: header(width, height),
                    ..Default::default()
                };
                let mut bits = std::ptr::null_mut::<c_void>();
                let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
                let previous = SelectObject(dc, bitmap.into());

                let rt = factory.CreateDCRenderTarget(&target_props(96.0 * scale))?;
                rt.BindDC(
                    dc,
                    &RECT {
                        left: 0,
                        top: 0,
                        right: width,
                        bottom: height,
                    },
                )?;
                let brushes = Brushes::new(&rt)?;
                scene.draw(&rt, &brushes, BAR_PERIOD_MS / 2)?;
                let _ = GdiFlush();
                let len = (width * height * 4) as usize;
                let pixels = std::slice::from_raw_parts(bits as *const u8, len).to_vec();

                SelectObject(dc, previous);
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(dc);
                Ok(Frame {
                    width,
                    height,
                    pixels,
                })
            }
        }

        fn write_bmp(path: &Path, frame: &Frame) -> std::io::Result<()> {
            const OFFSET: u32 = 14 + 40;
            let h = header(frame.width, frame.height);
            let mut out = Vec::with_capacity(OFFSET as usize + frame.pixels.len());
            out.extend_from_slice(b"BM");
            out.extend_from_slice(&(OFFSET + frame.pixels.len() as u32).to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&OFFSET.to_le_bytes());
            out.extend_from_slice(&h.biSize.to_le_bytes());
            out.extend_from_slice(&h.biWidth.to_le_bytes());
            out.extend_from_slice(&h.biHeight.to_le_bytes());
            out.extend_from_slice(&h.biPlanes.to_le_bytes());
            out.extend_from_slice(&h.biBitCount.to_le_bytes());
            // 압축 없음(BI_RGB), 나머지 필드는 0
            out.resize(OFFSET as usize, 0);
            out.extend_from_slice(&frame.pixels);
            std::fs::write(path, out)
        }

        /// `DONGSI_SPLASH_DUMP=<폴더>`를 주면 그린 결과를 `splash-1x.bmp`·`splash-2x.bmp`로 남긴다.
        #[test]
        fn 로고는_icon_svg의_색으로_그려진다() {
            // SAFETY: 이 스레드에서만 쓰는 single-threaded factory다.
            let factory: ID2D1Factory =
                unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }.unwrap();
            // SAFETY: 위 factory로 factory 자원을 만든다.
            let scene = unsafe { Scene::new(&factory) }.unwrap();
            let dump = std::env::var_os("DONGSI_SPLASH_DUMP");

            for scale in [1.0_f32, 2.0] {
                let frame = render(&factory, &scene, scale).unwrap();
                if let Some(dir) = &dump {
                    std::fs::create_dir_all(dir).unwrap();
                    let path = Path::new(dir).join(format!("splash-{scale}x.bmp"));
                    write_bmp(&path, &frame).unwrap();
                }

                let to_dip = logo_transform(scene.left, BRAND_Y - LOGO_H / 2.0);
                // icon.svg 좌표(x, y)의 픽셀이 기대한 RGB에 가까운지 본다.
                let check = |what: &str, x: f32, y: f32, rgb: [u8; 3]| {
                    let px = ((x * to_dip.M11 + to_dip.M31) * scale) as i32;
                    let py = ((y * to_dip.M22 + to_dip.M32) * scale) as i32;
                    let i = ((py * frame.width + px) * 4) as usize;
                    let p = &frame.pixels[i..i + 3];
                    let actual = [p[2], p[1], p[0]];
                    assert!(
                        actual.iter().zip(rgb).all(|(a, e)| a.abs_diff(e) <= 6),
                        "{scale}x {what} ({px}, {py}): {actual:02x?}, 기대 {rgb:02x?}"
                    );
                };
                check("동공 중심", 512.0, 550.0, [0x3f, 0x15, 0x90]);
                check("하이라이트", 468.0, 504.0, [0xff, 0xff, 0xff]);
                check("라임 화면", 330.0, 550.0, [0xb8, 0xf2, 0x3a]);
                check("가운데 속눈썹", 512.0, 200.0, [0x83, 0x57, 0xfc]);
                check("눈 위쪽(그라데이션)", 512.0, 330.0, [0x8a, 0x56, 0xfb]);
                check("눈 아래쪽(그라데이션)", 512.0, 780.0, [0x6d, 0x32, 0xe3]);
                for (x, y) in [(60.0, 140.0), (960.0, 140.0), (70.0, 820.0), (950.0, 820.0)] {
                    check("눈 바깥 모서리", x, y, [0x0d, 0x0e, 0x11]);
                }
            }
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
