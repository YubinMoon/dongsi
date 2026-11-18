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

// SAFETY: D3D11 장치는 자유 스레드 객체이고, 즉시 컨텍스트는 아래에서
// SetMultithreadProtected(TRUE)로 직렬화한다. 실제로는 워커 스레드 하나만 컨텍스트를 쓴다.
unsafe impl Send for D3d {}

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
