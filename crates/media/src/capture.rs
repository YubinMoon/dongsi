//! Windows.Graphics.Capture(WGC)로 모니터·창 캡처.
//!
//! 자유 스레드 프레임 풀을 써서 시스템 스레드에서 `FrameArrived`를 받는다.
//! 콜백은 프레임을 꺼내 넘기기만 하고, 변환·인코딩은 워커 스레드가 한다.

use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureAccess,
    GraphicsCaptureAccessKind, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{IInspectable, Interface};

use crate::d3d::D3d;
use crate::{MediaError, SourceId};

const PIXEL_FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;
const POOL_BUFFERS: i32 = 3;

/// WGC가 넘겨준 프레임 하나.
pub(crate) struct CapturedFrame {
    frame: Direct3D11CaptureFrame,
    /// `SystemRelativeTime`(QPC 100ns)
    pub qpc: i64,
    /// 내용 크기(`ContentSize`)
    pub width: u32,
    pub height: u32,
}

impl CapturedFrame {
    pub fn texture(&self) -> Result<ID3D11Texture2D, MediaError> {
        let surface = self
            .frame
            .Surface()
            .map_err(|e| MediaError::capture("프레임 surface", e))?;
        let access: IDirect3DDxgiInterfaceAccess = surface
            .cast()
            .map_err(|e| MediaError::capture("IDirect3DDxgiInterfaceAccess", e))?;
        // SAFETY: WGC surface는 ID3D11Texture2D를 감싼다.
        unsafe { access.GetInterface::<ID3D11Texture2D>() }
            .map_err(|e| MediaError::capture("ID3D11Texture2D", e))
    }
}

pub(crate) fn create_item(source: &SourceId) -> Result<GraphicsCaptureItem, MediaError> {
    if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
        return Err(MediaError::CaptureUnsupported);
    }
    let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
        .map_err(|e| MediaError::capture("IGraphicsCaptureItemInterop", e))?;
    // SAFETY: 잘못된 핸들이면 API가 오류를 돌려준다.
    unsafe {
        match source {
            SourceId::Monitor(h) => interop
                .CreateForMonitor::<GraphicsCaptureItem>(HMONITOR(*h as *mut _))
                .map_err(|e| MediaError::SourceNotFound(format!("{source}: {}", e.message()))),
            SourceId::Window(h) => interop
                .CreateForWindow::<GraphicsCaptureItem>(HWND(*h as *mut _))
                .map_err(|e| MediaError::SourceNotFound(format!("{source}: {}", e.message()))),
            SourceId::TestPattern => Err(MediaError::Capture(
                "테스트 패턴은 WGC 대상이 아닙니다".into(),
            )),
        }
    }
}

pub(crate) struct WgcCapture {
    item: GraphicsCaptureItem,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    frame_token: i64,
    closed_token: i64,
    pool_size: SizeInt32,
    d3d: D3d,
    stopped: bool,
}

impl WgcCapture {
    /// 캡처를 시작한다. `on_frame`은 시스템 스레드에서 프레임마다 불린다.
    pub fn start(
        d3d: &D3d,
        source: &SourceId,
        on_frame: impl Fn(CapturedFrame) + Send + 'static,
        on_closed: impl Fn() + Send + 'static,
    ) -> Result<Self, MediaError> {
        let item = create_item(source)?;
        let size = item
            .Size()
            .map_err(|e| MediaError::capture("대상 크기", e))?;
        if size.Width <= 0 || size.Height <= 0 {
            return Err(MediaError::Capture(format!(
                "대상 크기가 0입니다({}×{})",
                size.Width, size.Height
            )));
        }
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &d3d.winrt_device,
            PIXEL_FORMAT,
            POOL_BUFFERS,
            size,
        )
        .map_err(|e| MediaError::capture("프레임 풀", e))?;
        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|e| MediaError::capture("캡처 세션", e))?;
        let _ = session.SetIsCursorCaptureEnabled(true);
        // 노란 테두리 끄기: Windows 11에서는 허용 요청 후 설정한다. 실패해도 진행한다.
        if let Ok(op) =
            GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless)
        {
            let _ = op.join();
        }
        if let Err(e) = session.SetIsBorderRequired(false) {
            tracing::debug!("IsBorderRequired(false) 실패: {e}");
        }
        // 기본 최소 간격 제한을 풀고(1ms), 프레임 수 제한은 직접 한다.
        if let Err(e) = session.SetMinUpdateInterval(TimeSpan { Duration: 10_000 }) {
            tracing::debug!("MinUpdateInterval 설정 실패: {e}");
        }

        let frame_token = pool
            .FrameArrived(
                &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(
                    move |sender, _| {
                        let Some(pool) = sender.as_ref() else {
                            return Ok(());
                        };
                        // 쌓인 프레임을 모두 꺼내고 마지막 것만 넘긴다(밀린 프레임은 버린다).
                        let mut latest = None;
                        while let Ok(frame) = pool.TryGetNextFrame() {
                            latest = Some(frame);
                        }
                        if let Some(frame) = latest {
                            let qpc = frame.SystemRelativeTime().map(|t| t.Duration).unwrap_or(0);
                            let size = frame.ContentSize().unwrap_or_default();
                            on_frame(CapturedFrame {
                                frame,
                                qpc,
                                width: size.Width.max(0) as u32,
                                height: size.Height.max(0) as u32,
                            });
                        }
                        Ok(())
                    },
                ),
            )
            .map_err(|e| MediaError::capture("FrameArrived 등록", e))?;
        let closed_token = item
            .Closed(
                &TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_, _| {
                    on_closed();
                    Ok(())
                }),
            )
            .map_err(|e| MediaError::capture("Closed 등록", e))?;
        session
            .StartCapture()
            .map_err(|e| MediaError::capture("StartCapture", e))?;
        Ok(WgcCapture {
            item,
            pool,
            session,
            frame_token,
            closed_token,
            pool_size: size,
            d3d: d3d.clone(),
            stopped: false,
        })
    }

    /// 대상의 현재 크기.
    pub fn item_size(&self) -> (u32, u32) {
        let s = self.item.Size().unwrap_or(self.pool_size);
        (s.Width.max(0) as u32, s.Height.max(0) as u32)
    }

    /// 내용 크기가 바뀌면 프레임 풀을 새 크기로 다시 만든다.
    pub fn follow_size(&mut self, w: u32, h: u32) {
        if w == 0
            || h == 0
            || (self.pool_size.Width as u32 == w && self.pool_size.Height as u32 == h)
        {
            return;
        }
        let size = SizeInt32 {
            Width: w as i32,
            Height: h as i32,
        };
        match self
            .pool
            .Recreate(&self.d3d.winrt_device, PIXEL_FORMAT, POOL_BUFFERS, size)
        {
            Ok(()) => {
                tracing::debug!(w, h, "캡처 대상 크기 변경, 프레임 풀 재생성");
                self.pool_size = size;
            }
            Err(e) => tracing::warn!("프레임 풀 재생성 실패: {e}"),
        }
    }

    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        let _ = self.pool.RemoveFrameArrived(self.frame_token);
        let _ = self.item.RemoveClosed(self.closed_token);
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

impl Drop for WgcCapture {
    fn drop(&mut self) {
        self.stop();
    }
}
