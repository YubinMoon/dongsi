//! 캡처 텍스처(BGRA) → 인코더 입력(4:2:0 YUV) 변환. GPU(D3D11 Video Processor)로만 한다(D-105).
//!
//! Video Processor로 크기 조절과 BGRA→NV12(BT.601 limited) 변환을 한 번에 한다. 결과가 인코더에 가는 길은 둘이다.
//! zero-copy는 hw 풀의 NV12 텍스처에 바로 쓰고([`Converter::convert_to_texture`]), 읽어 오기는 NV12 결과만 스테이징
//! 텍스처로 읽어 온다([`Converter::convert`], 4K BGRA 33MB 대신 NV12 12MB).
//! CPU 변환(swscale)은 없다. Video Processor를 쓸 수 없으면 [`Converter::new`]가 실패해 방송을 시작하지 않고
//! ([`MediaError::NoGpuConvert`]), 방송 중 읽어 오기 변환이 실패하면 [`MediaError::GpuConvert`]로 방송을 멈춘다.

use std::mem::ManuallyDrop;

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CPU_ACCESS_READ,
    D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEX2D_ARRAY_VPOV, D3D11_TEX2D_VPIV,
    D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
    D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_COLOR_SPACE,
    D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT,
    D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
    D3D11_VIDEO_USAGE_OPTIMAL_SPEED, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2DARRAY, ID3D11Texture2D,
    ID3D11VideoContext, ID3D11VideoContext1, ID3D11VideoDevice, ID3D11VideoProcessor,
    ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601,
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::core::Interface;

use crate::MediaError;
use crate::d3d::{D3d, create_d3d};
use crate::yuv::YuvView;

struct SrcTex {
    tex: ID3D11Texture2D,
    w: u32,
    h: u32,
    format: DXGI_FORMAT,
}

/// 비디오 프로세서 하나와 보관본(입력) 뷰. 출력 크기·형식마다 하나다. 출력 뷰는 출력 텍스처마다 따로 만든다.
struct VpCore {
    /// (원본 너비, 원본 높이, 출력 너비, 출력 높이)
    key: (u32, u32, u32, u32),
    _enumerator: ID3D11VideoProcessorEnumerator,
    vp: ID3D11VideoProcessor,
    in_view: ID3D11VideoProcessorInputView,
}

/// [`Converter::convert`]용: 출력 텍스처 하나와 읽어 올 스테이징까지 가진 상태.
struct VpState {
    core: VpCore,
    out_tex: ID3D11Texture2D,
    out_view: ID3D11VideoProcessorOutputView,
    staging: ID3D11Texture2D,
}

/// [`Converter::preview_rgba`]용: 작은 BGRA 출력 텍스처와 그 스테이징.
struct PreviewState {
    core: VpCore,
    out_tex: ID3D11Texture2D,
    out_view: ID3D11VideoProcessorOutputView,
    staging: ID3D11Texture2D,
}

/// VP 출력 형식.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OutKind {
    /// 인코더 입력: BT.601 limited YCbCr.
    Nv12,
    /// 미리 보기: sRGB 전범위 RGB.
    Bgra,
}

struct GpuPath {
    vdev: ID3D11VideoDevice,
    vctx: ID3D11VideoContext,
    /// [`Converter::convert`]의 VP 상태(출력 크기 하나). 원본·출력 크기가 바뀌면 다시 만든다.
    state: Option<VpState>,
    /// [`Converter::convert_to_texture`]의 NV12 VP(출력 크기 하나). 원본·출력 크기가 바뀌면 다시 만든다.
    target: Option<VpCore>,
    preview: Option<PreviewState>,
}

impl GpuPath {
    /// 보관 텍스처를 가리키는 VP 상태를 모두 버린다.
    fn reset(&mut self) {
        self.state = None;
        self.target = None;
        self.preview = None;
    }
}

/// 인코딩하는 출력 크기의 최소 가로·세로(D-109). 출력(원본에 화질 상자를 건 크기, `VideoSettings::output_size`)이 이보다
/// 작은 프레임은 보관하지 않고 버려 마지막 화면을 둔다([`output_too_small`]). 창이 막 뜨거나 최소화될 때 잠깐 아주 작은
/// 프레임이 오고(롤 게임 창은 처음에 2×2), 그 크기로는 하드웨어 인코더가 열리지 않는다.
///
/// 160×64는 h264_nvenc의 최소 146×50을 넉넉히 덮는 고정값이다. AMF·QSV의 최소는 미검증이라, 이보다 커서 인코더를 다시
/// 열지 못하면 비디오 워커의 안전장치(옛 인코더를 두고 그 크기의 프레임만 버림)가 덮는다.
pub(crate) const MIN_OUTPUT_WIDTH: u32 = 160;
pub(crate) const MIN_OUTPUT_HEIGHT: u32 = 64;

/// 출력 크기 `(w, h)`가 인코딩하기에 너무 작은지(D-109, [`MIN_OUTPUT_WIDTH`]×[`MIN_OUTPUT_HEIGHT`]). 판정은 이 함수
/// 하나이고, 비디오 워커가 프레임을 보관([`Converter::ingest`])하기 전에 부른다.
pub(crate) fn output_too_small((w, h): (u32, u32)) -> bool {
    w < MIN_OUTPUT_WIDTH || h < MIN_OUTPUT_HEIGHT
}

/// 캡처 텍스처에서 실제로 쓸 왼쪽 위 영역의 크기: 내용 크기 `w`×`h`를 텍스처 크기 안으로 자른다. 창이 막 커지면
/// 프레임 풀(텍스처)이 아직 작아 내용이 텍스처보다 클 수 있다.
pub(crate) fn region_size(tex: &ID3D11Texture2D, w: u32, h: u32) -> (u32, u32) {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: 유효한 텍스처.
    unsafe { tex.GetDesc(&mut desc) };
    (w.min(desc.Width), h.min(desc.Height))
}

/// 이 PC에서 GPU 변환(D3D11 Video Processor, BGRA → NV12)을 쓸 수 있는지 본다(D-105). 앱은 방송을 시작하기 전에 하드웨어
/// 인코더 확인과 같은 자리에서 부른다. 쓸 수 없으면 [`MediaError::NoGpuConvert`]다(장치를 잠깐 만들어 보므로 수십 ms 걸릴 수
/// 있다).
pub fn check_gpu_convert() -> Result<(), MediaError> {
    let d3d = create_d3d().map_err(|e| MediaError::NoGpuConvert(e.to_string()))?;
    Converter::new(d3d).map(drop)
}

/// 마지막 캡처 프레임을 보관하고, 원하는 크기의 NV12로 바꿔 준다(GPU, D-105).
///
/// 보관본이 있으므로 화면이 멈춰 있어도(새 프레임이 없어도) 키프레임 요청이나
/// 해상도 변경 때 마지막 화면을 다시 변환할 수 있다.
pub(crate) struct Converter {
    d3d: D3d,
    src: Option<SrcTex>,
    gpu: GpuPath,
}

// SAFETY: 워커 스레드 하나에서만 쓴다. D3D 객체는 자유 스레드다.
unsafe impl Send for Converter {}

pub(crate) fn tex_desc(
    w: u32,
    h: u32,
    format: DXGI_FORMAT,
    staging: bool,
    bind: u32,
) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: if staging {
            D3D11_USAGE_STAGING
        } else {
            D3D11_USAGE_DEFAULT
        },
        BindFlags: if staging { 0 } else { bind },
        CPUAccessFlags: if staging {
            D3D11_CPU_ACCESS_READ.0 as u32
        } else {
            0
        },
        MiscFlags: 0,
    }
}

impl Converter {
    /// GPU 변환기를 만든다. 장치에 비디오 처리(`ID3D11VideoDevice`)가 없거나 Video Processor가 BGRA 입력·NV12 출력을
    /// 받지 않으면 [`MediaError::NoGpuConvert`]다(D-105: CPU 변환으로 넘어가지 않는다).
    pub fn new(d3d: D3d) -> Result<Self, MediaError> {
        if !d3d.video_support {
            return Err(MediaError::NoGpuConvert(
                "비디오 처리를 지원하는 D3D11 장치를 만들지 못했습니다".into(),
            ));
        }
        let vdev = d3d
            .device
            .cast::<ID3D11VideoDevice>()
            .map_err(|e| MediaError::NoGpuConvert(format!("ID3D11VideoDevice: {e}")))?;
        let vctx = d3d
            .context
            .cast::<ID3D11VideoContext>()
            .map_err(|e| MediaError::NoGpuConvert(format!("ID3D11VideoContext: {e}")))?;
        check_vp_formats(&vdev).map_err(MediaError::NoGpuConvert)?;
        Ok(Converter {
            d3d,
            src: None,
            gpu: GpuPath {
                vdev,
                vctx,
                state: None,
                target: None,
                preview: None,
            },
        })
    }

    /// 보관한 프레임 크기.
    pub fn source_size(&self) -> Option<(u32, u32)> {
        self.src.as_ref().map(|s| (s.w, s.h))
    }

    /// 캡처 텍스처의 왼쪽 위 `w`×`h` 영역(텍스처보다 크면 [`region_size`]로 자른다)을 보관본으로 복사한다(GPU 안
    /// 복사). 크기 판정([`output_too_small`])은 호출자(비디오 워커)가 먼저 한다. 영역이 비어 있으면 오류다.
    pub fn ingest(&mut self, tex: &ID3D11Texture2D, w: u32, h: u32) -> Result<(), MediaError> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: 유효한 텍스처.
        unsafe { tex.GetDesc(&mut desc) };
        let (w, h) = (w.min(desc.Width), h.min(desc.Height));
        if w == 0 || h == 0 {
            return Err(MediaError::Graphics(format!(
                "보관할 영역이 비어 있습니다({w}×{h})"
            )));
        }
        let recreate = self
            .src
            .as_ref()
            .is_none_or(|s| s.w != w || s.h != h || s.format != desc.Format);
        if recreate {
            let d = tex_desc(
                w,
                h,
                desc.Format,
                false,
                (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            );
            let mut t = None;
            // SAFETY: 설명자와 출력 포인터가 유효하다.
            unsafe { self.d3d.device.CreateTexture2D(&d, None, Some(&mut t)) }
                .map_err(|e| MediaError::graphics("보관 텍스처 생성", e))?;
            self.src = Some(SrcTex {
                tex: t.ok_or_else(|| MediaError::Graphics("보관 텍스처가 없습니다".into()))?,
                w,
                h,
                format: desc.Format,
            });
            self.gpu.reset();
        }
        let src = self.src.as_ref().expect("방금 만들었다");
        let region = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: w,
            bottom: h,
            back: 1,
        };
        // SAFETY: 두 텍스처 모두 같은 장치의 것이고 영역이 범위 안이다.
        unsafe {
            self.d3d
                .context
                .CopySubresourceRegion(&src.tex, 0, 0, 0, 0, tex, 0, Some(&region));
        }
        Ok(())
    }

    /// 보관한 프레임을 `dst` 크기로 변환해(VP) 읽어 온다. 변환이 실패하면 [`MediaError::GpuConvert`]다(D-105: 다른 변환
    /// 경로가 없어 비디오 워커가 방송을 멈춘다).
    pub fn convert(&mut self, dst: &mut YuvView<'_>) -> Result<(), MediaError> {
        if self.src.is_none() {
            return Err(MediaError::Graphics("변환할 프레임이 없습니다".into()));
        }
        self.convert_gpu(dst)
            .map_err(|e| MediaError::GpuConvert(e.to_string()))
    }

    fn convert_gpu(&mut self, dst: &mut YuvView<'_>) -> Result<(), MediaError> {
        let src = self.src.as_ref().expect("확인함");
        let gpu = &mut self.gpu;
        let (out_w, out_h) = (dst.width as u32, dst.height as u32);
        let key = (src.w, src.h, out_w, out_h);
        if gpu.state.as_ref().is_none_or(|s| s.core.key != key) {
            gpu.state = None;
            gpu.state = Some(create_vp_state(&self.d3d, gpu, src, out_w, out_h)?);
        }
        let st = gpu.state.as_ref().expect("방금 만들었다");
        let ctx = &self.d3d.context;
        blt(gpu, &st.core, &st.out_view)?;
        // SAFETY: 모든 객체가 같은 장치의 것이다. Map한 동안만 포인터를 쓴다.
        unsafe {
            ctx.CopyResource(&st.staging, &st.out_tex);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&st.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| MediaError::graphics("NV12 스테이징 Map", e))?;
            let pitch = mapped.RowPitch as usize;
            let base = mapped.pData as *const u8;
            let y = std::slice::from_raw_parts(base, pitch * out_h as usize);
            let uv = std::slice::from_raw_parts(
                base.add(pitch * out_h as usize),
                pitch * out_h as usize / 2,
            );
            dst.copy_from_nv12(y, uv, pitch);
            ctx.Unmap(&st.staging, 0);
        }
        Ok(())
    }

    /// 보관한 BGRA 원본을 NV12 출력 텍스처 `tex`(array index `slice`)에 VideoProcessorBlt한다. 텍스처는
    /// `D3D11_BIND_RENDER_TARGET` NV12다(hw 풀 텍스처는 index 0).
    ///
    /// - VP(비디오 프로세서·enumerator·입력 뷰)는 출력 크기 하나만 쥐고, 출력 크기가 바뀌면 다시 만든다. 출력 뷰는 호출마다
    ///   만든다: 풀이 텍스처를 해제하고 같은 주소에 다시 만들 수 있어 주소로 캐시하면 안 된다. VP는 보관 텍스처가 다시
    ///   만들어질 때(원본 크기·형식이 바뀔 때) 버린다.
    /// - 색 변환은 [`Converter::convert`]와 같다(입력 sRGB 전범위 → 출력 BT.601 limited).
    /// - Flush도 Map도 하지 않는다(같은 장치의 NVENC가 매핑할 때 동기화한다). 호출이 돌아와도 GPU는 아직 쓰는 중일 수
    ///   있다.
    /// - 보관본이 없으면 `Err`. 실패하면 호출자(비디오 워커)가 zero-copy에서 읽어 오기로 내려간다. 오류 문자열에
    ///   HRESULT가 들어 있다(장치 손실이면 `DXGI_ERROR_DEVICE_REMOVED` 0x887A0005 등).
    pub fn convert_to_texture(
        &mut self,
        tex: &ID3D11Texture2D,
        slice: u32,
    ) -> Result<(), MediaError> {
        let src = self
            .src
            .as_ref()
            .ok_or_else(|| MediaError::Graphics("변환할 프레임이 없습니다".into()))?;
        let gpu = &mut self.gpu;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: 유효한 텍스처.
        unsafe { tex.GetDesc(&mut desc) };
        if desc.Format != DXGI_FORMAT_NV12 {
            return Err(MediaError::Graphics(format!(
                "출력 텍스처가 NV12가 아닙니다({:?})",
                desc.Format
            )));
        }
        if desc.BindFlags & D3D11_BIND_RENDER_TARGET.0 as u32 == 0 {
            return Err(MediaError::Graphics(
                "출력 텍스처에 RENDER_TARGET 바인딩이 없습니다".into(),
            ));
        }
        if slice >= desc.ArraySize {
            return Err(MediaError::Graphics(format!(
                "출력 텍스처 array index {slice}가 범위({})를 벗어났습니다",
                desc.ArraySize
            )));
        }
        let key = (src.w, src.h, desc.Width, desc.Height);
        if gpu.target.as_ref().is_none_or(|c| c.key != key) {
            gpu.target = None;
            gpu.target = Some(create_vp_core(
                gpu,
                src,
                desc.Width,
                desc.Height,
                OutKind::Nv12,
            )?);
        }
        let core = gpu.target.as_ref().expect("방금 만들었다");
        let view = create_out_view(gpu, core, tex, slice, desc.ArraySize > 1)?;
        blt(gpu, core, &view)
    }

    /// 미리 보기: 보관한 원본을 긴 변이 `max` 이하가 되게(비율 유지, 키우지 않음) 줄인 RGBA를 만든다.
    /// 돌려주는 값은 [`PreviewScaler::scale`](crate::preview::PreviewScaler::scale)과 같은 (너비, 높이, RGBA)다.
    ///
    /// GPU에서 작은 BGRA 텍스처로 줄인 뒤 그것만 스테이징으로 읽어 온다(큰 텍스처를 읽지 않는다). 읽어 올 때
    /// GPU가 끝나기를 기다린다. 보관본이 없으면 `Ok(None)`.
    pub fn preview_rgba(&mut self, max: u32) -> Result<Option<(u32, u32, Vec<u8>)>, MediaError> {
        let Some(src) = self.src.as_ref() else {
            return Ok(None);
        };
        let gpu = &mut self.gpu;
        let (dw, dh) = crate::preview::fit(src.w, src.h, max, max);
        if max == 0 || dw == 0 || dh == 0 {
            return Ok(None);
        }
        let key = (src.w, src.h, dw, dh);
        if gpu.preview.as_ref().is_none_or(|p| p.core.key != key) {
            gpu.preview = None;
            gpu.preview = Some(create_preview_state(&self.d3d, gpu, src, dw, dh)?);
        }
        let st = gpu.preview.as_ref().expect("방금 만들었다");
        blt(gpu, &st.core, &st.out_view)?;
        let ctx = &self.d3d.context;
        let mut rgba = vec![0u8; dw as usize * dh as usize * 4];
        // SAFETY: 스테이징은 같은 크기의 BGRA다. Map한 동안만 포인터를 쓴다.
        unsafe {
            ctx.CopyResource(&st.staging, &st.out_tex);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&st.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| MediaError::graphics("미리 보기 스테이징 Map", e))?;
            let pitch = mapped.RowPitch as usize;
            let base = mapped.pData as *const u8;
            let row_bytes = dw as usize * 4;
            for (y, out) in rgba.chunks_exact_mut(row_bytes).enumerate() {
                let row = std::slice::from_raw_parts(base.add(y * pitch), row_bytes);
                for (o, p) in out
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(row.as_chunks::<4>().0.iter())
                {
                    o.copy_from_slice(&[p[2], p[1], p[0], 255]);
                }
            }
            ctx.Unmap(&st.staging, 0);
        }
        Ok(Some((dw, dh, rgba)))
    }
}

fn vp_err(what: &str) -> impl FnOnce(windows::core::Error) -> MediaError {
    let what = what.to_string();
    move |e| MediaError::graphics(&what, e)
}

/// `in_w`×`in_h` → `out_w`×`out_h` 진행 주사(progressive) 변환의 VP 내용 설명.
fn vp_content(in_w: u32, in_h: u32, out_w: u32, out_h: u32) -> D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
    let rate = DXGI_RATIONAL {
        Numerator: 60,
        Denominator: 1,
    };
    D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
        InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
        InputFrameRate: rate,
        InputWidth: in_w,
        InputHeight: in_h,
        OutputFrameRate: rate,
        OutputWidth: out_w,
        OutputHeight: out_h,
        Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
    }
}

/// 이 장치의 Video Processor가 BGRA 입력과 NV12 출력을 받는지 본다([`Converter::new`]). 크기는 아무 값(1280×720)이다.
/// 받지 않으면 이유 문자열.
fn check_vp_formats(vdev: &ID3D11VideoDevice) -> Result<(), String> {
    let content = vp_content(1280, 720, 1280, 720);
    // SAFETY: 설명자가 유효하고, 읽기만 하는 호출이다.
    let (input, output) = unsafe {
        let e = vdev
            .CreateVideoProcessorEnumerator(&content)
            .map_err(|e| format!("VideoProcessorEnumerator: {e}"))?;
        let input = e
            .CheckVideoProcessorFormat(DXGI_FORMAT_B8G8R8A8_UNORM)
            .map_err(|e| format!("BGRA 입력 확인: {e}"))?;
        let output = e
            .CheckVideoProcessorFormat(DXGI_FORMAT_NV12)
            .map_err(|e| format!("NV12 출력 확인: {e}"))?;
        (input, output)
    };
    if input & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT.0 as u32 == 0 {
        return Err("Video Processor가 BGRA 입력을 지원하지 않습니다".into());
    }
    if output & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT.0 as u32 == 0 {
        return Err("Video Processor가 NV12 출력을 지원하지 않습니다".into());
    }
    Ok(())
}

/// 보관본(`src`)을 `out_w`×`out_h` `kind` 형식으로 바꾸는 비디오 프로세서와 입력 뷰를 만든다.
fn create_vp_core(
    gpu: &GpuPath,
    src: &SrcTex,
    out_w: u32,
    out_h: u32,
    kind: OutKind,
) -> Result<VpCore, MediaError> {
    let err = vp_err;
    let content = vp_content(src.w, src.h, out_w, out_h);
    let (out_format, out_name) = match kind {
        OutKind::Nv12 => (DXGI_FORMAT_NV12, "NV12"),
        OutKind::Bgra => (DXGI_FORMAT_B8G8R8A8_UNORM, "BGRA"),
    };
    // SAFETY: 설명자와 출력 포인터가 유효하고, 모든 객체가 같은 장치의 것이다.
    unsafe {
        let enumerator = gpu
            .vdev
            .CreateVideoProcessorEnumerator(&content)
            .map_err(err("VideoProcessorEnumerator"))?;
        let support = enumerator
            .CheckVideoProcessorFormat(out_format)
            .map_err(err(&format!("{out_name} 출력 확인")))?;
        if support & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT.0 as u32 == 0 {
            return Err(MediaError::Graphics(format!(
                "Video Processor가 {out_name} 출력을 지원하지 않습니다"
            )));
        }
        let vp = gpu
            .vdev
            .CreateVideoProcessor(&enumerator, 0)
            .map_err(err("CreateVideoProcessor"))?;

        let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: 0,
                },
            },
        };
        let mut in_view = None;
        gpu.vdev
            .CreateVideoProcessorInputView(&src.tex, &enumerator, &in_desc, Some(&mut in_view))
            .map_err(err("입력 뷰"))?;

        let vctx = &gpu.vctx;
        vctx.VideoProcessorSetStreamFrameFormat(&vp, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
        vctx.VideoProcessorSetStreamAutoProcessingMode(&vp, 0, false);
        let src_rect = RECT {
            left: 0,
            top: 0,
            right: src.w as i32,
            bottom: src.h as i32,
        };
        let dst_rect = RECT {
            left: 0,
            top: 0,
            right: out_w as i32,
            bottom: out_h as i32,
        };
        vctx.VideoProcessorSetStreamSourceRect(&vp, 0, true, Some(&src_rect));
        vctx.VideoProcessorSetStreamDestRect(&vp, 0, true, Some(&dst_rect));
        vctx.VideoProcessorSetOutputTargetRect(&vp, true, Some(&dst_rect));
        // 입력: sRGB 전범위 RGB. 출력: NV12면 BT.601 limited range YCbCr, BGRA면 sRGB 전범위 RGB.
        if let Ok(vctx1) = vctx.cast::<ID3D11VideoContext1>() {
            vctx1.VideoProcessorSetStreamColorSpace1(
                &vp,
                0,
                DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
            );
            vctx1.VideoProcessorSetOutputColorSpace1(
                &vp,
                match kind {
                    OutKind::Nv12 => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601,
                    OutKind::Bgra => DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
                },
            );
        } else {
            // 비트필드: Usage(0) RGB_Range(1) YCbCr_Matrix(2) xvYCC(3) Nominal_Range(4-5)
            vctx.VideoProcessorSetStreamColorSpace(
                &vp,
                0,
                &D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 },
            );
            vctx.VideoProcessorSetOutputColorSpace(
                &vp,
                &D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                    _bitfield: match kind {
                        OutKind::Nv12 => 1 << 4,
                        OutKind::Bgra => 0,
                    },
                },
            );
        }

        Ok(VpCore {
            key: (src.w, src.h, out_w, out_h),
            _enumerator: enumerator,
            vp,
            in_view: in_view.ok_or_else(|| MediaError::Graphics("입력 뷰가 없습니다".into()))?,
        })
    }
}

/// `tex`(array index `slice`)의 VP 출력 뷰. `array`면 array 뷰로, 아니면 일반 2D 뷰로 만든다.
fn create_out_view(
    gpu: &GpuPath,
    core: &VpCore,
    tex: &ID3D11Texture2D,
    slice: u32,
    array: bool,
) -> Result<ID3D11VideoProcessorOutputView, MediaError> {
    let desc = if array {
        D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2DARRAY,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2DArray: D3D11_TEX2D_ARRAY_VPOV {
                    MipSlice: 0,
                    FirstArraySlice: slice,
                    ArraySize: 1,
                },
            },
        }
    } else {
        D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        }
    };
    let mut view = None;
    // SAFETY: 설명자와 출력 포인터가 유효하고, 텍스처·enumerator는 같은 장치의 것이다.
    unsafe {
        gpu.vdev
            .CreateVideoProcessorOutputView(tex, &core._enumerator, &desc, Some(&mut view))
    }
    .map_err(vp_err("출력 뷰"))?;
    view.ok_or_else(|| MediaError::Graphics("출력 뷰가 없습니다".into()))
}

/// 보관본 → `out_view` VideoProcessorBlt 한 번.
fn blt(
    gpu: &GpuPath,
    core: &VpCore,
    out_view: &ID3D11VideoProcessorOutputView,
) -> Result<(), MediaError> {
    // SAFETY: 모든 객체가 같은 장치의 것이다. ManuallyDrop은 Blt 뒤에 직접 해제한다.
    unsafe {
        let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: ManuallyDrop::new(Some(core.in_view.clone())),
            ..Default::default()
        };
        let res = gpu
            .vctx
            .VideoProcessorBlt(&core.vp, out_view, 0, std::slice::from_ref(&stream));
        ManuallyDrop::drop(&mut stream.pInputSurface);
        res.map_err(|e| MediaError::graphics("VideoProcessorBlt", e))
    }
}

fn create_texture(
    d3d: &D3d,
    desc: &D3D11_TEXTURE2D_DESC,
    what: &str,
) -> Result<ID3D11Texture2D, MediaError> {
    let mut t = None;
    // SAFETY: 설명자와 출력 포인터가 유효하다.
    unsafe { d3d.device.CreateTexture2D(desc, None, Some(&mut t)) }.map_err(vp_err(what))?;
    t.ok_or_else(|| MediaError::Graphics(format!("{what}가 없습니다")))
}

fn create_vp_state(
    d3d: &D3d,
    gpu: &GpuPath,
    src: &SrcTex,
    out_w: u32,
    out_h: u32,
) -> Result<VpState, MediaError> {
    let core = create_vp_core(gpu, src, out_w, out_h, OutKind::Nv12)?;
    let out_tex = create_texture(
        d3d,
        &tex_desc(
            out_w,
            out_h,
            DXGI_FORMAT_NV12,
            false,
            D3D11_BIND_RENDER_TARGET.0 as u32,
        ),
        "NV12 출력 텍스처",
    )?;
    let out_view = create_out_view(gpu, &core, &out_tex, 0, false)?;
    let staging = create_texture(
        d3d,
        &tex_desc(out_w, out_h, DXGI_FORMAT_NV12, true, 0),
        "NV12 스테이징",
    )?;
    Ok(VpState {
        core,
        out_tex,
        out_view,
        staging,
    })
}

fn create_preview_state(
    d3d: &D3d,
    gpu: &GpuPath,
    src: &SrcTex,
    out_w: u32,
    out_h: u32,
) -> Result<PreviewState, MediaError> {
    let core = create_vp_core(gpu, src, out_w, out_h, OutKind::Bgra)?;
    let out_tex = create_texture(
        d3d,
        &tex_desc(
            out_w,
            out_h,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            false,
            D3D11_BIND_RENDER_TARGET.0 as u32,
        ),
        "미리 보기 BGRA 출력 텍스처",
    )?;
    let out_view = create_out_view(gpu, &core, &out_tex, 0, false)?;
    let staging = create_texture(
        d3d,
        &tex_desc(out_w, out_h, DXGI_FORMAT_B8G8R8A8_UNORM, true, 0),
        "미리 보기 BGRA 스테이징",
    )?;
    Ok(PreviewState {
        core,
        out_tex,
        out_view,
        staging,
    })
}

/// 벤치마크용: 지정한 크기의 BGRA 텍스처(그라데이션)를 만든다.
pub(crate) fn create_test_texture(
    d3d: &D3d,
    w: u32,
    h: u32,
) -> Result<ID3D11Texture2D, MediaError> {
    use windows::Win32::Graphics::Direct3D11::D3D11_SUBRESOURCE_DATA;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            pixels[i] = (x * 255 / w) as u8;
            pixels[i + 1] = (y * 255 / h) as u8;
            pixels[i + 2] = ((x + y) % 256) as u8;
            pixels[i + 3] = 255;
        }
    }
    let init = D3D11_SUBRESOURCE_DATA {
        pSysMem: pixels.as_ptr() as *const _,
        SysMemPitch: w * 4,
        SysMemSlicePitch: 0,
    };
    let mut t = None;
    // SAFETY: 초기 데이터는 호출 동안 살아 있다.
    unsafe {
        d3d.device.CreateTexture2D(
            &tex_desc(
                w,
                h,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                false,
                D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ),
            Some(&init),
            Some(&mut t),
        )
    }
    .map_err(|e| MediaError::graphics("테스트 텍스처", e))?;
    t.ok_or_else(|| MediaError::Graphics("테스트 텍스처가 없습니다".into()))
}

#[cfg(test)]
mod tests {
    use dongsi_core::VideoSettings;

    use super::*;

    #[test]
    fn output_too_small_below_160_by_64() {
        assert!(!output_too_small((MIN_OUTPUT_WIDTH, MIN_OUTPUT_HEIGHT)));
        assert!(output_too_small((MIN_OUTPUT_WIDTH - 1, MIN_OUTPUT_HEIGHT)));
        assert!(output_too_small((MIN_OUTPUT_WIDTH, MIN_OUTPUT_HEIGHT - 1)));
        assert!(!output_too_small((1920, 1080)));
        // h264_nvenc 최소(146×50)보다 작은 출력은 모두 거른다.
        assert!(output_too_small((146, 50)));
        assert!(output_too_small((144, 1080)));
    }

    /// 판정은 원본이 아니라 출력 크기(원본에 화질 상자를 건 크기)로 한다(D-109).
    #[test]
    fn small_output_is_judged_after_the_quality_box() {
        let fhd = VideoSettings::DEFAULT;
        let too_small = |sw, sh| output_too_small(fhd.output_size(sw, sh));
        // 롤 게임 창은 처음에 2×2로 온다.
        assert!(too_small(2, 2));
        assert!(too_small(159, 720));
        assert!(too_small(1280, 63));
        assert!(!too_small(160, 64));
        assert!(!too_small(1280, 720));
        // 가로로 아주 긴 창: 원본 세로(100)는 64보다 크지만 FHD 상자에 맞추면 1920×48이라 거른다(원본 크기로는 못 거르던 경우).
        assert_eq!(fhd.output_size(4000, 100), (1920, 48));
        assert!(too_small(4000, 100));
        // QHD 상자면 같은 창이 2560×64라 보낸다.
        let qhd = VideoSettings {
            width: 2560,
            height: 1440,
            ..VideoSettings::DEFAULT
        };
        assert_eq!(qhd.output_size(4000, 100), (2560, 64));
        assert!(!output_too_small(qhd.output_size(4000, 100)));
    }

    #[test]
    fn ingest_copies_the_region_inside_the_texture() {
        let d3d = create_d3d().unwrap();
        let mut c = Converter::new(d3d.clone()).unwrap();
        let tiny = create_test_texture(&d3d, 2, 2).unwrap();
        let big = create_test_texture(&d3d, 640, 360).unwrap();
        assert_eq!(c.source_size(), None);
        c.ingest(&big, 640, 360).unwrap();
        assert_eq!(c.source_size(), Some((640, 360)));
        // 내용은 커졌는데 프레임 풀(텍스처)이 아직 작으면 텍스처 크기로 자른다(이 크기는 워커가 먼저 거른다).
        assert_eq!(region_size(&tiny, 1600, 900), (2, 2));
        assert_eq!(region_size(&big, 1600, 900), (640, 360));
        assert_eq!(region_size(&big, 320, 900), (320, 360));
        c.ingest(&big, 1600, 900).unwrap();
        assert_eq!(c.source_size(), Some((640, 360)));
        c.ingest(&big, 320, 200).unwrap();
        assert_eq!(c.source_size(), Some((320, 200)));
        // 빈 영역은 보관하지 않는다(보관본은 그대로).
        assert!(c.ingest(&big, 0, 200).is_err());
        assert_eq!(c.source_size(), Some((320, 200)));
    }

    use crate::gpu_pattern::test_util::{
        create_nv12_target, luma_of_bgra, read_bgra, read_nv12_y, wait_gpu,
    };
    use crate::gpu_pattern::{GpuPattern, GpuPatternKind, bar_x, read_marks};
    use std::time::Instant;

    /// Y 평면 둘의 평균 절대 차이.
    fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
        assert_eq!(a.len(), b.len());
        a.iter()
            .zip(b)
            .map(|(&p, &q)| (p as i32 - q as i32).unsigned_abs() as u64)
            .sum::<u64>() as f64
            / a.len() as f64
    }

    /// `convert()`로(GPU 변환 뒤 읽어 오기) `w`×`h` NV12 Y 평면을 얻는다.
    fn convert_y(c: &mut Converter, w: usize, h: usize) -> Vec<u8> {
        let mut y = vec![0u8; w * h];
        let mut uv = vec![0u8; w * h / 2];
        let mut view = YuvView {
            width: w,
            height: h,
            y: &mut y,
            y_stride: w,
            chroma: crate::yuv::Chroma::Nv12 {
                uv: &mut uv,
                stride: w,
            },
        };
        c.convert(&mut view).unwrap();
        y
    }

    #[test]
    fn convert_to_texture_matches_readback() {
        let d3d = create_d3d().unwrap();
        let (sw, sh, n) = (1920u32, 1080u32, 1234u64);
        let mut pat = GpuPattern::new(&d3d, sw, sh, GpuPatternKind::Bars).unwrap();
        let mut c = Converter::new(d3d.clone()).unwrap();
        c.ingest(pat.frame(n).unwrap(), sw, sh).unwrap();
        let target_key = |c: &Converter| c.gpu.target.as_ref().map(|t| t.key);

        // 출력 크기가 바뀌면 VP를 그 크기로 다시 만든다.
        let a = create_nv12_target(&d3d, 1280, 720);
        let b = create_nv12_target(&d3d, 640, 360);
        for (tex, w, h) in [(&a, 1280usize, 720usize), (&b, 640, 360)] {
            c.convert_to_texture(tex, 0).unwrap();
            assert_eq!(target_key(&c), Some((sw, sh, w as u32, h as u32)));
            let (y, stride) = read_nv12_y(&d3d, tex);
            let (num, x) = read_marks(&y, stride, w as u32, h as u32)
                .unwrap_or_else(|| panic!("{w}×{h}에서 번호를 읽지 못함"));
            assert_eq!(num, n & 0xFFFF, "{w}×{h} 프레임 번호");
            let want = bar_x(n, sw) as f64 * w as f64 / sw as f64;
            assert!(
                (x as f64 - want).abs() <= 3.0,
                "{w}×{h} 막대 x {x}, 기대 {want:.1}"
            );
            // 같은 원본을 읽어 오는 convert()(같은 VP, 출력만 스테이징)와 비교한다.
            let via_convert = convert_y(&mut c, w, h);
            let d = mean_abs_diff(&y, &via_convert);
            eprintln!("{w}×{h}: Y 평균 절대 차이 — convert()(GPU 변환 + 읽어 오기) {d:.3}");
            assert!(d < 0.5, "같은 VP인데 달라졌다: {d}");
        }

        // 같은 크기를 다시 쓰면 VP를 그대로 쓰고, 출력 텍스처를 바꿔도(주소가 달라도) 된다.
        let b2 = create_nv12_target(&d3d, 640, 360);
        c.convert_to_texture(&b2, 0).unwrap();
        assert_eq!(target_key(&c), Some((sw, sh, 640, 360)));
        let (y, stride) = read_nv12_y(&d3d, &b2);
        assert_eq!(read_marks(&y, stride, 640, 360).unwrap().0, n & 0xFFFF);

        // 원본 크기가 바뀌면 VP를 버리고 새 크기로 다시 만든다.
        let mut pat2 = GpuPattern::new(&d3d, 1280, 720, GpuPatternKind::Bars).unwrap();
        c.ingest(pat2.frame(7).unwrap(), 1280, 720).unwrap();
        assert_eq!(target_key(&c), None);
        c.convert_to_texture(&b, 0).unwrap();
        let (y, stride) = read_nv12_y(&d3d, &b);
        assert_eq!(read_marks(&y, stride, 640, 360).unwrap().0, 7);
    }

    #[test]
    fn convert_to_texture_rejects_unusable_targets() {
        let d3d = create_d3d().unwrap();
        let mut pat = GpuPattern::new(&d3d, 640, 360, GpuPatternKind::Bars).unwrap();
        let mut c = Converter::new(d3d.clone()).unwrap();
        let out = create_nv12_target(&d3d, 640, 360);
        // 보관본이 없다.
        assert!(c.convert_to_texture(&out, 0).is_err());
        c.ingest(pat.frame(0).unwrap(), 640, 360).unwrap();
        // NV12가 아니다.
        let bgra = create_test_texture(&d3d, 640, 360).unwrap();
        assert!(c.convert_to_texture(&bgra, 0).is_err());
        // array index가 범위 밖이다.
        assert!(c.convert_to_texture(&out, 1).is_err());
        // 정상.
        c.convert_to_texture(&out, 0).unwrap();
    }

    #[test]
    fn preview_rgba_fits_max_and_matches_source_colors() {
        let d3d = create_d3d().unwrap();
        let (sw, sh) = (1920u32, 1080u32);
        let mut pat = GpuPattern::new(&d3d, sw, sh, GpuPatternKind::Bars).unwrap();
        let mut c = Converter::new(d3d.clone()).unwrap();
        // 보관본이 없으면 None.
        assert!(c.preview_rgba(480).unwrap().is_none());
        let tex = pat.frame(500).unwrap().clone();
        c.ingest(&tex, sw, sh).unwrap();

        let (w, h, rgba) = c.preview_rgba(480).unwrap().expect("GPU 미리 보기");
        assert_eq!((w, h), (480, 270));
        assert_eq!(rgba.len(), 480 * 270 * 4);
        assert!(rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
        // 원본(1920×1080)을 4×4 평균으로 줄인 것과 비교한다.
        let (src, _, _) = read_bgra(&d3d, &tex);
        let mut want = vec![0u8; rgba.len()];
        for y in 0..270usize {
            for x in 0..480usize {
                let mut acc = [0u32; 3];
                for dy in 0..4 {
                    for dx in 0..4 {
                        let i = ((y * 4 + dy) * 1920 + x * 4 + dx) * 4;
                        acc[0] += src[i + 2] as u32; // R
                        acc[1] += src[i + 1] as u32;
                        acc[2] += src[i] as u32;
                    }
                }
                let o = (y * 480 + x) * 4;
                want[o] = (acc[0] / 16) as u8;
                want[o + 1] = (acc[1] / 16) as u8;
                want[o + 2] = (acc[2] / 16) as u8;
                want[o + 3] = 255;
            }
        }
        let rgb_diff = |a: &[u8], b: &[u8]| -> f64 {
            let (mut sum, mut n) = (0u64, 0u64);
            for (p, q) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0.iter()) {
                for k in 0..3 {
                    sum += (p[k] as i32 - q[k] as i32).unsigned_abs() as u64;
                    n += 1;
                }
            }
            sum as f64 / n as f64
        };
        let mean = |a: &[u8], k: usize| -> f64 {
            a.as_chunks::<4>()
                .0
                .iter()
                .map(|p| p[k] as u64)
                .sum::<u64>() as f64
                / (a.len() / 4) as f64
        };
        let d = rgb_diff(&rgba, &want);
        eprintln!(
            "preview 480×270: RGB 평균 절대 차이(4×4 평균 대비) {d:.3}, 채널 평균 R/G/B 미리보기 {:.1}/{:.1}/{:.1} 기대 {:.1}/{:.1}/{:.1}",
            mean(&rgba, 0),
            mean(&rgba, 1),
            mean(&rgba, 2),
            mean(&want, 0),
            mean(&want, 1),
            mean(&want, 2)
        );
        assert!(d < 2.0, "미리 보기가 원본과 다르다: {d}");
        for k in 0..3 {
            assert!(
                (mean(&rgba, k) - mean(&want, k)).abs() < 2.0,
                "채널 {k} 평균"
            );
        }

        // 긴 변 기준: 세로로 긴 상자도 비율을 지킨다. 원본보다 크면 키우지 않는다.
        let (w, h, _) = c.preview_rgba(320).unwrap().unwrap();
        assert_eq!((w, h), (320, 180));
        let (w, h, rgba) = c.preview_rgba(4000).unwrap().unwrap();
        assert_eq!((w, h, rgba.len()), (1920, 1080, 1920 * 1080 * 4));
        assert!(c.preview_rgba(0).unwrap().is_none());
        // 번호 블록이 미리 보기에서도 읽힌다(휘도를 RGBA에서 뽑는다).
        let (w, h, rgba) = c.preview_rgba(960).unwrap().unwrap();
        let bgra: Vec<u8> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0], 255])
            .collect();
        let (num, _) =
            read_marks(&luma_of_bgra(&bgra), w as usize, w, h).expect("미리 보기에서 번호");
        assert_eq!(num, 500);
    }

    /// 비용 한 번 재기(되풀이하지 않는다). GPU 완료까지 재려고 시험 안에서만 Flush·쿼리를 쓴다.
    #[test]
    #[cfg_attr(
        not(feature = "full-tests"),
        ignore = "비용 측정(GPU에서 QHD를 여러 번 변환)"
    )]
    fn measure_convert_to_texture_and_preview_cost() {
        let d3d = create_d3d().unwrap();
        let mut pat = GpuPattern::new(&d3d, 2560, 1440, GpuPatternKind::Bars).unwrap();
        let mut c = Converter::new(d3d.clone()).unwrap();
        c.ingest(pat.frame(1).unwrap(), 2560, 1440).unwrap();
        wait_gpu(&d3d);
        let qhd = create_nv12_target(&d3d, 2560, 1440);
        let fhd = create_nv12_target(&d3d, 1920, 1080);
        let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
        let report = |name: &str, c: &mut Converter, out: &ID3D11Texture2D| {
            let t = Instant::now();
            c.convert_to_texture(out, 0).unwrap();
            let submit = ms(t);
            wait_gpu(&d3d);
            eprintln!("{name}: 호출 {submit:.3}ms, GPU 완료까지 {:.3}ms", ms(t));
        };
        report("QHD→QHD 처음(VP 상태 만들기 포함)", &mut c, &qhd);
        report("QHD→QHD 두 번째(캐시)", &mut c, &qhd);
        report("QHD→FHD 처음(VP 다시 만들기 포함)", &mut c, &fhd);
        report("QHD→FHD 두 번째(캐시)", &mut c, &fhd);
        let t = Instant::now();
        let (w, h, _) = c.preview_rgba(640).unwrap().unwrap();
        eprintln!(
            "preview_rgba(640) 처음 {w}×{h}(VP 상태 만들기 포함, 읽어 오기까지): {:.3}ms",
            ms(t)
        );
        let t = Instant::now();
        c.preview_rgba(640).unwrap().unwrap();
        eprintln!("preview_rgba(640) 두 번째: {:.3}ms", ms(t));
    }
}
