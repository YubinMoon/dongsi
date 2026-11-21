//! 캡처 대상 미리 보기(썸네일): WGC로 대상마다 한 프레임만 받아 줄이고 JPEG로 만든다.
//!
//! 방송 시작 화면이 몇 초마다 부른다. 대상마다 캡처 세션을 잠깐 열고, 첫 프레임을 받으면 바로 닫는다.
//! 줄이기는 GPU 밉맵(상자 필터)으로 목표 크기보다 크거나 같은 단계까지 내린 뒤 CPU 쌍선형 보간으로 맞춘다.
//! 인코딩은 Windows Imaging Component(WIC)의 JPEG 인코더를 쓴다(새 의존성 없음).

use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureAccess,
    GraphicsCaptureAccessKind, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HGLOBAL, HWND};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CPU_ACCESS_READ,
    D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_RESOURCE_MISC_GENERATE_MIPS,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, ID3D11ShaderResourceView,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatJpeg, GUID_WICPixelFormat24bppBGR,
    IWICImagingFactory, WICBitmapEncoderNoCache,
};
use windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, STREAM_SEEK_END, STREAM_SEEK_SET,
};
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::UI::WindowsAndMessaging::IsIconic;
use windows::core::{IInspectable, Interface};

use crate::MediaError;
use crate::SourceId;
use crate::capture::create_item;
use crate::com::keep_mta;
use crate::d3d::{D3d, create_d3d};

/// 대상 하나의 미리 보기.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Thumbnail {
    /// JPEG 그림. 크기는 원본 비율을 지켜 최대 크기 안에 맞춘 값이다.
    Jpeg {
        width: u32,
        height: u32,
        data: Vec<u8>,
    },
    /// 최소화된 창. WGC가 프레임을 주지 않으므로 캡처하지 않는다.
    Minimized,
    /// 프레임이 시간 안에 오지 않았거나 캡처할 수 없는 대상(테스트 패턴, 닫힌 창 등).
    Unavailable,
}

/// 썸네일 전용 D3D 장치. 처음 쓸 때 만들고, 그래픽 오류가 나면 버려 다음에 새로 만든다.
/// 잠금은 동시에 두 번 찍지 않게 하는 역할도 한다.
static DEVICE: Mutex<Option<D3d>> = Mutex::new(None);

/// 노란 테두리 끄기 허용 요청과 MTA 유지는 프로세스에서 한 번만 한다.
static ONCE: OnceLock<()> = OnceLock::new();

/// `ids` 대상들의 미리 보기를 찍는다. 결과는 `ids`와 같은 순서다.
///
/// 대상마다 캡처 세션을 동시에 열고, `timeout`까지 첫 프레임을 기다린다.
/// 그림은 `max_width`×`max_height` 안에 원본 비율로 맞추고, 키우지는 않는다.
pub fn capture_thumbnails(
    ids: &[SourceId],
    max_width: u32,
    max_height: u32,
    timeout: Duration,
) -> Result<Vec<Thumbnail>, MediaError> {
    let mut slot = DEVICE.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        *slot = Some(create_d3d()?);
    }
    let Some(d3d) = slot.as_ref() else {
        return Err(MediaError::Graphics("썸네일 장치가 없습니다".into()));
    };
    ONCE.get_or_init(|| {
        keep_mta();
        match GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless)
            .and_then(|op| op.join())
        {
            Ok(status) => tracing::info!("썸네일: 테두리 없는 캡처 허용 상태 {status:?}"),
            Err(e) => tracing::info!("썸네일: 테두리 없는 캡처 허용 요청 실패: {e}"),
        }
    });

    let (tx, rx) = mpsc::channel::<(usize, Direct3D11CaptureFrame)>();
    let mut out = vec![Thumbnail::Unavailable; ids.len()];
    let mut sessions: Vec<Option<OneShot>> = Vec::with_capacity(ids.len());
    for (i, id) in ids.iter().enumerate() {
        if is_minimized(id) {
            out[i] = Thumbnail::Minimized;
            sessions.push(None);
            continue;
        }
        match OneShot::start(d3d, id, i, tx.clone()) {
            Ok(s) => sessions.push(Some(s)),
            Err(e) => {
                tracing::debug!("썸네일 캡처 시작 실패({id}): {e}");
                sessions.push(None);
            }
        }
    }
    drop(tx);

    let deadline = Instant::now() + timeout;
    let mut pending = sessions.iter().filter(|s| s.is_some()).count();
    let mut graphics_error = false;
    while pending > 0 {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let Ok((i, frame)) = rx.recv_timeout(left) else {
            break;
        };
        // 이미 받은 대상의 뒤늦은 프레임은 버린다.
        let Some(session) = sessions[i].take() else {
            continue;
        };
        pending -= 1;
        match shrink_and_encode(d3d, &frame, max_width, max_height) {
            Ok(t) => out[i] = t,
            Err(e) => {
                tracing::debug!("썸네일 만들기 실패({}): {e}", ids[i]);
                graphics_error |= matches!(e, MediaError::Graphics(_));
            }
        }
        let _ = frame.Close();
        drop(session);
    }
    drop(sessions);
    if graphics_error {
        // 장치 제거(TDR) 같은 경우를 대비해 다음 호출에서 새로 만든다.
        *slot = None;
    }
    Ok(out)
}

fn is_minimized(id: &SourceId) -> bool {
    match id {
        // SAFETY: 읽기만 하는 API다. 잘못된 HWND면 FALSE.
        SourceId::Window(h) => unsafe { IsIconic(HWND(*h as *mut _)).as_bool() },
        _ => false,
    }
}

/// 첫 프레임 하나만 받는 캡처 세션. 버리면 닫힌다.
struct OneShot {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    token: i64,
}

impl OneShot {
    fn start(
        d3d: &D3d,
        id: &SourceId,
        index: usize,
        tx: mpsc::Sender<(usize, Direct3D11CaptureFrame)>,
    ) -> Result<Self, MediaError> {
        let item = create_item(id)?;
        let size = item
            .Size()
            .map_err(|e| MediaError::capture("대상 크기", e))?;
        if size.Width <= 0 || size.Height <= 0 {
            return Err(MediaError::Capture("대상 크기가 0입니다".into()));
        }
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &d3d.winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            1,
            size,
        )
        .map_err(|e| MediaError::capture("프레임 풀", e))?;
        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|e| MediaError::capture("캡처 세션", e))?;
        let _ = session.SetIsCursorCaptureEnabled(false);
        let _ = session.SetIsBorderRequired(false);
        let tx = Mutex::new(tx);
        let token = pool
            .FrameArrived(
                &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(
                    move |sender, _| {
                        if let Some(pool) = sender.as_ref()
                            && let Ok(frame) = pool.TryGetNextFrame()
                        {
                            let _ = tx
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .send((index, frame));
                        }
                        Ok(())
                    },
                ),
            )
            .map_err(|e| MediaError::capture("FrameArrived 등록", e))?;
        let shot = OneShot {
            pool,
            session,
            token,
        };
        shot.session
            .StartCapture()
            .map_err(|e| MediaError::capture("StartCapture", e))?;
        Ok(shot)
    }
}

impl Drop for OneShot {
    fn drop(&mut self) {
        let _ = self.pool.RemoveFrameArrived(self.token);
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

/// `(w, h)`를 `(max_w, max_h)` 안에 비율을 지켜 맞춘 크기. 키우지 않는다.
fn fit(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
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

/// 목표 크기보다 작아지지 않는 가장 작은 밉맵 단계.
fn mip_level(w: u32, h: u32, tw: u32, th: u32) -> u32 {
    let mut level = 0;
    while (w >> (level + 1)) >= tw && (h >> (level + 1)) >= th && level < 15 {
        level += 1;
    }
    level
}

fn shrink_and_encode(
    d3d: &D3d,
    frame: &Direct3D11CaptureFrame,
    max_w: u32,
    max_h: u32,
) -> Result<Thumbnail, MediaError> {
    let surface = frame
        .Surface()
        .map_err(|e| MediaError::capture("프레임 surface", e))?;
    let access: IDirect3DDxgiInterfaceAccess = surface
        .cast()
        .map_err(|e| MediaError::capture("IDirect3DDxgiInterfaceAccess", e))?;
    // SAFETY: WGC surface는 ID3D11Texture2D를 감싼다.
    let tex: ID3D11Texture2D =
        unsafe { access.GetInterface() }.map_err(|e| MediaError::capture("ID3D11Texture2D", e))?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: 출력 구조체를 넘긴다.
    unsafe { tex.GetDesc(&mut desc) };
    let content = frame.ContentSize().unwrap_or_default();
    let cw = (content.Width.max(0) as u32).min(desc.Width);
    let ch = (content.Height.max(0) as u32).min(desc.Height);
    if cw == 0 || ch == 0 {
        return Ok(Thumbnail::Unavailable);
    }
    let (tw, th) = fit(cw, ch, max_w, max_h);
    let (bgra, lw, lh) = read_scaled(d3d, &tex, cw, ch, mip_level(cw, ch, tw, th))?;
    let bgr = resize_bgra_to_bgr(&bgra, lw, lh, tw, th);
    let data = encode_jpeg(&bgr, tw, th)?;
    Ok(Thumbnail::Jpeg {
        width: tw,
        height: th,
        data,
    })
}

/// 프레임의 `(0, 0, cw, ch)` 영역을 밉맵 `level` 단계로 줄여 CPU로 읽는다(BGRA, 빈틈없는 줄).
fn read_scaled(
    d3d: &D3d,
    src: &ID3D11Texture2D,
    cw: u32,
    ch: u32,
    level: u32,
) -> Result<(Vec<u8>, u32, u32), MediaError> {
    let lw = (cw >> level).max(1);
    let lh = (ch >> level).max(1);
    let gfx = |what: &'static str| move |e: windows::core::Error| MediaError::graphics(what, e);
    let tex_desc = |w: u32, h: u32, mips: u32, staging: bool| D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: mips,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: if staging {
            D3D11_USAGE_STAGING
        } else {
            D3D11_USAGE_DEFAULT
        },
        BindFlags: if staging {
            0
        } else {
            (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32
        },
        CPUAccessFlags: if staging {
            D3D11_CPU_ACCESS_READ.0 as u32
        } else {
            0
        },
        MiscFlags: if staging {
            0
        } else {
            D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32
        },
    };
    let region = D3D11_BOX {
        left: 0,
        top: 0,
        front: 0,
        right: cw,
        bottom: ch,
        back: 1,
    };
    // SAFETY: 모든 자원은 같은 장치에서 만들고, 크기·형식을 맞춰 복사한다.
    unsafe {
        let mut staging = None;
        d3d.device
            .CreateTexture2D(&tex_desc(lw, lh, 1, true), None, Some(&mut staging))
            .map_err(gfx("스테이징 텍스처"))?;
        let staging = staging.ok_or_else(|| MediaError::Graphics("스테이징 텍스처".into()))?;
        if level == 0 {
            d3d.context
                .CopySubresourceRegion(&staging, 0, 0, 0, 0, src, 0, Some(&region));
        } else {
            let mut mips = None;
            d3d.device
                .CreateTexture2D(&tex_desc(cw, ch, level + 1, false), None, Some(&mut mips))
                .map_err(gfx("밉맵 텍스처"))?;
            let mips: ID3D11Texture2D =
                mips.ok_or_else(|| MediaError::Graphics("밉맵 텍스처".into()))?;
            d3d.context
                .CopySubresourceRegion(&mips, 0, 0, 0, 0, src, 0, Some(&region));
            let mut srv: Option<ID3D11ShaderResourceView> = None;
            d3d.device
                .CreateShaderResourceView(&mips, None, Some(&mut srv))
                .map_err(gfx("셰이더 리소스 뷰"))?;
            let srv = srv.ok_or_else(|| MediaError::Graphics("셰이더 리소스 뷰".into()))?;
            d3d.context.GenerateMips(&srv);
            d3d.context
                .CopySubresourceRegion(&staging, 0, 0, 0, 0, &mips, level, None);
        }
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        d3d.context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(gfx("Map"))?;
        let row = lw as usize * 4;
        let pitch = mapped.RowPitch as usize;
        let base = mapped.pData as *const u8;
        let mut out = vec![0u8; row * lh as usize];
        for y in 0..lh as usize {
            std::ptr::copy_nonoverlapping(base.add(y * pitch), out.as_mut_ptr().add(y * row), row);
        }
        d3d.context.Unmap(&staging, 0);
        Ok((out, lw, lh))
    }
}

/// BGRA(`sw`×`sh`)를 쌍선형 보간으로 `dw`×`dh` BGR로 줄인다. 알파는 버린다.
fn resize_bgra_to_bgr(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let (sw, sh, dw, dh) = (sw as usize, sh as usize, dw as usize, dh as usize);
    let mut out = vec![0u8; dw * dh * 3];
    let sx = sw as f32 / dw as f32;
    let sy = sh as f32 / dh as f32;
    for y in 0..dh {
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (sh - 1) as f32);
        let y0 = fy as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let wy = fy - y0 as f32;
        for x in 0..dw {
            let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (sw - 1) as f32);
            let x0 = fx as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let wx = fx - x0 as f32;
            let px = |xx: usize, yy: usize, c: usize| src[(yy * sw + xx) * 4 + c] as f32;
            for c in 0..3 {
                let top = px(x0, y0, c) * (1.0 - wx) + px(x1, y0, c) * wx;
                let bottom = px(x0, y1, c) * (1.0 - wx) + px(x1, y1, c) * wx;
                out[(y * dw + x) * 3 + c] = (top * (1.0 - wy) + bottom * wy).round() as u8;
            }
        }
    }
    out
}

/// BGR(빈틈없는 줄)을 JPEG로. WIC 기본 품질을 쓴다.
fn encode_jpeg(bgr: &[u8], w: u32, h: u32) -> Result<Vec<u8>, MediaError> {
    let wic = |what: &'static str| move |e: windows::core::Error| MediaError::graphics(what, e);
    // SAFETY: COM 객체 호출이다. 버퍼 크기는 w*h*3이고 줄 간격은 w*3이다.
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(wic("WIC 팩터리"))?;
        let stream =
            CreateStreamOnHGlobal(HGLOBAL::default(), true).map_err(wic("메모리 스트림"))?;
        let encoder = factory
            .CreateEncoder(&GUID_ContainerFormatJpeg, std::ptr::null())
            .map_err(wic("JPEG 인코더"))?;
        encoder
            .Initialize(&stream, WICBitmapEncoderNoCache)
            .map_err(wic("JPEG 인코더 준비"))?;
        let mut frame = None;
        let mut props = None;
        encoder
            .CreateNewFrame(&mut frame, &mut props)
            .map_err(wic("JPEG 프레임"))?;
        let frame = frame.ok_or_else(|| MediaError::Graphics("JPEG 프레임".into()))?;
        frame
            .Initialize(props.as_ref())
            .map_err(wic("JPEG 프레임 준비"))?;
        frame.SetSize(w, h).map_err(wic("JPEG 크기"))?;
        let mut format = GUID_WICPixelFormat24bppBGR;
        frame
            .SetPixelFormat(&mut format)
            .map_err(wic("JPEG 픽셀 형식"))?;
        if format != GUID_WICPixelFormat24bppBGR {
            return Err(MediaError::Graphics(
                "JPEG 인코더가 24bpp BGR을 받지 않습니다".into(),
            ));
        }
        frame.WritePixels(h, w * 3, bgr).map_err(wic("JPEG 쓰기"))?;
        frame.Commit().map_err(wic("JPEG 프레임 마무리"))?;
        encoder.Commit().map_err(wic("JPEG 마무리"))?;
        let mut len = 0u64;
        stream
            .Seek(0, STREAM_SEEK_END, Some(&mut len))
            .map_err(wic("스트림 길이"))?;
        stream
            .Seek(0, STREAM_SEEK_SET, None)
            .map_err(wic("스트림 처음으로"))?;
        let mut data = vec![0u8; len as usize];
        let mut read = 0u32;
        stream
            .Read(data.as_mut_ptr().cast(), len as u32, Some(&mut read))
            .ok()
            .map_err(wic("스트림 읽기"))?;
        data.truncate(read as usize);
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_ratio_and_never_grows() {
        assert_eq!(fit(2560, 1440, 480, 270), (480, 270));
        assert_eq!(fit(1920, 1200, 480, 270), (432, 270));
        assert_eq!(fit(800, 2000, 480, 270), (108, 270));
        assert_eq!(fit(320, 200, 480, 270), (320, 200));
        assert_eq!(fit(0, 10, 480, 270), (0, 0));
    }

    #[test]
    fn mip_level_stops_before_target() {
        assert_eq!(mip_level(2560, 1440, 480, 270), 2); // 640×360
        assert_eq!(mip_level(3840, 2160, 480, 270), 3); // 480×270
        assert_eq!(mip_level(1553, 896, 468, 270), 1); // 776×448
        assert_eq!(mip_level(320, 200, 320, 200), 0);
    }

    #[test]
    fn resize_flat_color() {
        let src: Vec<u8> = [10u8, 20, 30, 255].repeat(8 * 4);
        let out = resize_bgra_to_bgr(&src, 8, 4, 3, 2);
        assert_eq!(out, [10u8, 20, 30].repeat(6));
    }

    #[test]
    fn jpeg_header() {
        keep_mta();
        let data = encode_jpeg(&[128u8; 16 * 9 * 3], 16, 9).unwrap();
        assert_eq!(&data[..2], &[0xFF, 0xD8]);
        assert_eq!(&data[data.len() - 2..], &[0xFF, 0xD9]);
    }

    /// 이 PC의 주 모니터를 찍는다(CI가 아닌 실제 PC에서 돈다). 그림은 메모리에만 둔다.
    #[test]
    fn thumbnail_of_primary_monitor() {
        let list = crate::list_sources().unwrap();
        let primary = list.iter().find(|s| s.primary).unwrap();
        let started = Instant::now();
        let out = capture_thumbnails(
            &[primary.id.clone(), SourceId::TestPattern],
            480,
            270,
            Duration::from_secs(2),
        )
        .unwrap();
        let elapsed = started.elapsed();
        let again = Instant::now();
        let second = capture_thumbnails(
            std::slice::from_ref(&primary.id),
            480,
            270,
            Duration::from_secs(2),
        )
        .unwrap();
        eprintln!("두 번째 호출 {:?}", again.elapsed());
        assert!(matches!(second[0], Thumbnail::Jpeg { .. }));
        match &out[0] {
            Thumbnail::Jpeg {
                width,
                height,
                data,
            } => {
                assert!(*width <= 480 && *height <= 270 && *width > 0 && *height > 0);
                assert_eq!(&data[..2], &[0xFF, 0xD8]);
                eprintln!(
                    "주 모니터 썸네일 {width}×{height}, {} bytes, {elapsed:?}",
                    data.len()
                );
            }
            other => panic!("주 모니터 썸네일이 없습니다: {other:?}"),
        }
        assert_eq!(out[1], Thumbnail::Unavailable);
    }

    /// 측정용: 이 PC의 모든 화면·창을 몇 번 찍어 시간과 크기를 본다(`--ignored`로 돌린다). 그림은 저장하지 않는다.
    #[test]
    #[ignore]
    fn thumbnails_of_all_sources() {
        let list = crate::list_sources().unwrap();
        let ids: Vec<SourceId> = list.iter().map(|s| s.id.clone()).collect();
        for round in 0..4 {
            let started = Instant::now();
            let out = capture_thumbnails(&ids, 480, 270, Duration::from_millis(500)).unwrap();
            let elapsed = started.elapsed();
            let jpeg: Vec<usize> = out
                .iter()
                .filter_map(|t| match t {
                    Thumbnail::Jpeg { data, .. } => Some(data.len()),
                    _ => None,
                })
                .collect();
            let minimized = out.iter().filter(|t| **t == Thumbnail::Minimized).count();
            eprintln!(
                "{round}: 대상 {}개, 그림 {}개(합 {} bytes), 최소화 {minimized}, 없음 {}, {elapsed:?}",
                ids.len(),
                jpeg.len(),
                jpeg.iter().sum::<usize>(),
                ids.len() - jpeg.len() - minimized,
            );
        }
    }
}
