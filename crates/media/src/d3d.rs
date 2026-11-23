//! Direct3D 11 장치 준비.

use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0,
    D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_FLAG, D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
    D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice};
use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice;
use windows::core::Interface;

use crate::MediaError;

/// 캡처·변환에 쓰는 D3D11 장치 묶음.
#[derive(Clone)]
pub(crate) struct D3d {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    /// WGC 프레임 풀에 넘기는 WinRT 장치.
    pub winrt_device: IDirect3DDevice,
    /// 장치에 비디오 처리(ID3D11VideoDevice)가 있는지.
    pub video_support: bool,
}

// SAFETY: D3D11 장치는 자유 스레드 객체이고, 즉시 컨텍스트는 아래에서 SetMultithreadProtected(TRUE)로 직렬화한다.
// 컨텍스트를 직접 쓰는 것은 워커 스레드(캡처 복사·변환 Blt·읽어 오기)지만 그 하나만은 아니다: zero-copy(D-099)에서는
// 주 인코더(워커 스레드)와 simulcast 계층 인코더(계층 스레드마다)가 NVENC를 통해 같은 장치에 세션을 열고, NVENC가 이 장치의
// 입력 텍스처를 등록·매핑한다. nvenc는 D3D11에서 ffmpeg의 lock 콜백을 부르지 않으므로(nvenc.c 396~417) 스레드 사이 직렬화는
// 오직 이 SetMultithreadProtected에 의존한다. ffmpeg가 우리 장치에 쓰는 hwcontext lock은 `av_hwframe_transfer_data`뿐이라
// 쓰지 않는다.
unsafe impl Send for D3d {}

impl D3d {
    /// 장치가 사라졌으면(`GetDeviceRemovedReason`이 S_OK가 아니면) 그 사유 코드. 멀쩡하면 `None`.
    /// `DXGI_ERROR_DEVICE_REMOVED`(0x887A0005)·`DEVICE_HUNG`(0x887A0006)·`DEVICE_RESET`(0x887A0007)·
    /// `DRIVER_INTERNAL_ERROR`(0x887A0020) 등이 온다.
    pub fn removed_reason(&self) -> Option<windows::core::HRESULT> {
        // SAFETY: 유효한 장치에 대한 읽기 호출이다.
        unsafe { self.device.GetDeviceRemovedReason() }
            .err()
            .map(|e| e.code())
    }

    /// 장치가 사라졌으면 그 사유를 담은 [`MediaError::DeviceLost`].
    pub fn lost_error(&self) -> Option<MediaError> {
        self.removed_reason().map(|hr| {
            let name = match hr.0 as u32 {
                0x887A_0005 => "DXGI_ERROR_DEVICE_REMOVED",
                0x887A_0006 => "DXGI_ERROR_DEVICE_HUNG",
                0x887A_0007 => "DXGI_ERROR_DEVICE_RESET",
                0x887A_0020 => "DXGI_ERROR_DRIVER_INTERNAL_ERROR",
                0x887A_0001 => "DXGI_ERROR_INVALID_CALL",
                _ => "알 수 없는 사유",
            };
            MediaError::DeviceLost(format!("{name}, 0x{:08X}", hr.0 as u32))
        })
    }
}

fn create_device(
    flags: D3D11_CREATE_DEVICE_FLAG,
) -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext)> {
    const LEVELS: [D3D_FEATURE_LEVEL; 3] = [
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
    ];
    let mut device = None;
    let mut context = None;
    // SAFETY: 출력 포인터는 지역 변수다.
    unsafe {
        D3D11CreateDevice(
            None::<&IDXGIAdapter>,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            flags,
            Some(&LEVELS),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    match (device, context) {
        (Some(d), Some(c)) => Ok((d, c)),
        _ => Err(windows::core::Error::from_hresult(
            windows::Win32::Foundation::E_FAIL,
        )),
    }
}

pub(crate) fn create_d3d() -> Result<D3d, MediaError> {
    let (device, context, video_support) =
        match create_device(D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT) {
            Ok((d, c)) => (d, c, true),
            Err(e) => {
                tracing::warn!("비디오 지원 D3D11 장치 생성 실패, 일반 장치로 재시도: {e}");
                let (d, c) = create_device(D3D11_CREATE_DEVICE_BGRA_SUPPORT)
                    .map_err(|e| MediaError::graphics("D3D11CreateDevice", e))?;
                (d, c, false)
            }
        };
    // SAFETY: 유효한 COM 객체에 대한 호출이다.
    unsafe {
        if let Ok(mt) = context.cast::<ID3D11Multithread>() {
            let _ = mt.SetMultithreadProtected(true);
        }
        let dxgi: IDXGIDevice = device
            .cast()
            .map_err(|e| MediaError::graphics("IDXGIDevice", e))?;
        let inspectable = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)
            .map_err(|e| MediaError::graphics("CreateDirect3D11DeviceFromDXGIDevice", e))?;
        let winrt_device: IDirect3DDevice = inspectable
            .cast()
            .map_err(|e| MediaError::graphics("IDirect3DDevice", e))?;
        Ok(D3d {
            device,
            context,
            winrt_device,
            video_support,
        })
    }
}
