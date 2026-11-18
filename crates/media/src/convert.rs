//! 캡처 텍스처(BGRA) → 인코더 입력(4:2:0 YUV) 변환.
//!
//! 기본은 GPU 경로다: D3D11 Video Processor로 크기 조절과 BGRA→NV12(BT.601 limited)
//! 변환을 한 번에 하고, NV12 결과만 스테이징 텍스처로 읽어 온다(4K BGRA 33MB 대신 NV12 12MB).
//! Video Processor를 쓸 수 없으면 CPU 경로(BGRA 스테이징 읽기 + swscale)로 바꾼다.
//! 환경 변수 `DONGSI_MEDIA_CPU_CONVERT=1`이면 처음부터 CPU 경로를 쓴다(측정용).

use std::mem::ManuallyDrop;
use std::ptr;

use ffmpeg_sys_next as ff;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CPU_ACCESS_READ,
    D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
    D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_COLOR_SPACE,
    D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_OPTIMAL_SPEED, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D, ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoContext1,
    ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
    ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601,
    DXGI_FORMAT, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::core::Interface;

use crate::MediaError;
use crate::d3d::D3d;
use crate::yuv::{Chroma, YuvView};

struct SrcTex {
    tex: ID3D11Texture2D,
    w: u32,
    h: u32,
    format: DXGI_FORMAT,
}

struct VpState {
    key: (u32, u32, u32, u32),
    _enumerator: ID3D11VideoProcessorEnumerator,
    vp: ID3D11VideoProcessor,
    in_view: ID3D11VideoProcessorInputView,
    out_tex: ID3D11Texture2D,
    out_view: ID3D11VideoProcessorOutputView,
    staging: ID3D11Texture2D,
}

struct GpuPath {
    vdev: ID3D11VideoDevice,
    vctx: ID3D11VideoContext,
    state: Option<VpState>,
}

struct CpuPath {
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    sws: *mut ff::SwsContext,
    sws_key: (u32, u32, u32, u32, i32),
}

impl Drop for CpuPath {
    fn drop(&mut self) {
        if !self.sws.is_null() {
            // SAFETY: sws_getContext로 만든 컨텍스트.
            unsafe { ff::sws_freeContext(self.sws) };
        }
    }
}

/// 마지막 캡처 프레임을 보관하고, 원하는 크기의 YUV로 바꿔 준다.
///
/// 보관본이 있으므로 화면이 멈춰 있어도(새 프레임이 없어도) 키프레임 요청이나
/// 해상도 변경 때 마지막 화면을 다시 변환할 수 있다.
pub(crate) struct Converter {
    d3d: D3d,
    src: Option<SrcTex>,
    gpu: Option<GpuPath>,
    cpu: CpuPath,
    /// 마지막 변환이 GPU 경로였는지
    pub last_gpu: bool,
}

// SAFETY: 워커 스레드 하나에서만 쓴다. D3D 객체는 자유 스레드, sws 컨텍스트는 소유한다.
unsafe impl Send for Converter {}

fn tex_desc(w: u32, h: u32, format: DXGI_FORMAT, staging: bool, bind: u32) -> D3D11_TEXTURE2D_DESC {
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
    pub fn new(d3d: D3d) -> Self {
        let force_cpu = std::env::var("DONGSI_MEDIA_CPU_CONVERT").is_ok_and(|v| v == "1");
        let gpu = if d3d.video_support && !force_cpu {
            let vdev = d3d.device.cast::<ID3D11VideoDevice>();
            let vctx = d3d.context.cast::<ID3D11VideoContext>();
            match (vdev, vctx) {
                (Ok(vdev), Ok(vctx)) => Some(GpuPath {
                    vdev,
                    vctx,
                    state: None,
                }),
                _ => None,
            }
        } else {
            None
        };
        if gpu.is_none() {
            tracing::info!("GPU 변환을 쓸 수 없어 CPU 변환(swscale)을 씁니다");
        }
        Converter {
            d3d,
            src: None,
            gpu,
            cpu: CpuPath {
                staging: None,
                sws: ptr::null_mut(),
                sws_key: (0, 0, 0, 0, 0),
            },
            last_gpu: false,
        }
    }

    pub fn is_gpu(&self) -> bool {
        self.gpu.is_some()
    }

    /// 보관한 프레임 크기.
    pub fn source_size(&self) -> Option<(u32, u32)> {
        self.src.as_ref().map(|s| (s.w, s.h))
    }

    /// 캡처 텍스처의 왼쪽 위 `w`×`h` 영역을 보관본으로 복사한다(GPU 안 복사).
    pub fn ingest(&mut self, tex: &ID3D11Texture2D, w: u32, h: u32) -> Result<(), MediaError> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: 유효한 텍스처.
        unsafe { tex.GetDesc(&mut desc) };
        let (w, h) = (w.min(desc.Width).max(2), h.min(desc.Height).max(2));
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
            if let Some(g) = &mut self.gpu {
                g.state = None;
            }
            self.cpu.staging = None;
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

    /// 보관한 프레임을 `dst` 크기로 변환해 쓴다.
    pub fn convert(&mut self, dst: &mut YuvView<'_>) -> Result<(), MediaError> {
        if self.src.is_none() {
            return Err(MediaError::Graphics("변환할 프레임이 없습니다".into()));
        }
        let mut gpu_used = false;
        if self.gpu.is_some() {
            match self.convert_gpu(dst) {
                Ok(()) => gpu_used = true,
                Err(e) => {
                    tracing::warn!("GPU 변환 실패, CPU 변환으로 바꿉니다: {e}");
                    self.gpu = None;
                }
            }
        }
        if !gpu_used {
            self.convert_cpu(dst)?;
        }
        self.last_gpu = gpu_used;
        Ok(())
    }

    fn convert_gpu(&mut self, dst: &mut YuvView<'_>) -> Result<(), MediaError> {
        let src = self.src.as_ref().expect("확인함");
        let gpu = self.gpu.as_mut().expect("확인함");
        let (out_w, out_h) = (dst.width as u32, dst.height as u32);
        let key = (src.w, src.h, out_w, out_h);
        if gpu.state.as_ref().is_none_or(|s| s.key != key) {
            gpu.state = None;
            gpu.state = Some(create_vp_state(&self.d3d, gpu, src, out_w, out_h)?);
        }
        let st = gpu.state.as_ref().expect("방금 만들었다");
        let ctx = &self.d3d.context;
        // SAFETY: 모든 객체가 같은 장치의 것이다. ManuallyDrop은 Blt 뒤에 직접 해제한다.
        unsafe {
            let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: ManuallyDrop::new(Some(st.in_view.clone())),
                ..Default::default()
            };
            let res =
                gpu.vctx
                    .VideoProcessorBlt(&st.vp, &st.out_view, 0, std::slice::from_ref(&stream));
            ManuallyDrop::drop(&mut stream.pInputSurface);
            res.map_err(|e| MediaError::graphics("VideoProcessorBlt", e))?;
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

    fn convert_cpu(&mut self, dst: &mut YuvView<'_>) -> Result<(), MediaError> {
        let src = self.src.as_ref().expect("확인함");
        let (w, h) = (src.w, src.h);
        if self
            .cpu
            .staging
            .as_ref()
            .is_none_or(|s| s.1 != w || s.2 != h)
        {
            let d = tex_desc(w, h, src.format, true, 0);
            let mut t = None;
            // SAFETY: 설명자와 출력 포인터가 유효하다.
            unsafe { self.d3d.device.CreateTexture2D(&d, None, Some(&mut t)) }
                .map_err(|e| MediaError::graphics("BGRA 스테이징 생성", e))?;
            let t = t.ok_or_else(|| MediaError::Graphics("스테이징 텍스처가 없습니다".into()))?;
            self.cpu.staging = Some((t, w, h));
        }
        let staging = &self.cpu.staging.as_ref().expect("방금 만들었다").0;
        let nv12 = matches!(dst.chroma, Chroma::Nv12 { .. });
        let dst_fmt = if nv12 {
            ff::AVPixelFormat::AV_PIX_FMT_NV12
        } else {
            ff::AVPixelFormat::AV_PIX_FMT_YUV420P
        };
        let key = (w, h, dst.width as u32, dst.height as u32, dst_fmt as i32);
        if self.cpu.sws.is_null() || self.cpu.sws_key != key {
            // SAFETY: 이전 컨텍스트를 해제하고 새로 만든다.
            unsafe {
                if !self.cpu.sws.is_null() {
                    ff::sws_freeContext(self.cpu.sws);
                }
                self.cpu.sws = ff::sws_getContext(
                    w as i32,
                    h as i32,
                    ff::AVPixelFormat::AV_PIX_FMT_BGRA,
                    dst.width as i32,
                    dst.height as i32,
                    dst_fmt,
                    ff::SwsFlags::SWS_FAST_BILINEAR as i32,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null(),
                );
            }
            if self.cpu.sws.is_null() {
                return Err(MediaError::Graphics(
                    "swscale 컨텍스트를 만들지 못했습니다".into(),
                ));
            }
            self.cpu.sws_key = key;
        }
        let ctx = &self.d3d.context;
        // SAFETY: 스테이징은 같은 크기의 BGRA다. Map한 동안만 포인터를 쓴다.
        unsafe {
            ctx.CopyResource(staging, &src.tex);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| MediaError::graphics("BGRA 스테이징 Map", e))?;
            let src_planes = [
                mapped.pData as *const u8,
                ptr::null(),
                ptr::null(),
                ptr::null(),
            ];
            let src_strides = [mapped.RowPitch as i32, 0, 0, 0];
            let (dst_planes, dst_strides): ([*mut u8; 4], [i32; 4]) = match &mut dst.chroma {
                Chroma::Nv12 { uv, stride } => (
                    [
                        dst.y.as_mut_ptr(),
                        uv.as_mut_ptr(),
                        ptr::null_mut(),
                        ptr::null_mut(),
                    ],
                    [dst.y_stride as i32, *stride as i32, 0, 0],
                ),
                Chroma::I420 {
                    u,
                    u_stride,
                    v,
                    v_stride,
                } => (
                    [
                        dst.y.as_mut_ptr(),
                        u.as_mut_ptr(),
                        v.as_mut_ptr(),
                        ptr::null_mut(),
                    ],
                    [dst.y_stride as i32, *u_stride as i32, *v_stride as i32, 0],
                ),
            };
            ff::sws_scale(
                self.cpu.sws,
                src_planes.as_ptr(),
                src_strides.as_ptr(),
                0,
                h as i32,
                dst_planes.as_ptr(),
                dst_strides.as_ptr(),
            );
            ctx.Unmap(staging, 0);
        }
        Ok(())
    }
}

fn create_vp_state(
    d3d: &D3d,
    gpu: &GpuPath,
    src: &SrcTex,
    out_w: u32,
    out_h: u32,
) -> Result<VpState, MediaError> {
    let err = |what: &str| {
        let what = what.to_string();
        move |e: windows::core::Error| MediaError::graphics(&what, e)
    };
    let rate = DXGI_RATIONAL {
        Numerator: 60,
        Denominator: 1,
    };
    let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
        InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
        InputFrameRate: rate,
        InputWidth: src.w,
        InputHeight: src.h,
        OutputFrameRate: rate,
        OutputWidth: out_w,
        OutputHeight: out_h,
        Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
    };
    // SAFETY: 설명자와 출력 포인터가 유효하고, 모든 객체가 같은 장치의 것이다.
    unsafe {
        let enumerator = gpu
            .vdev
            .CreateVideoProcessorEnumerator(&content)
            .map_err(err("VideoProcessorEnumerator"))?;
        let support = enumerator
            .CheckVideoProcessorFormat(DXGI_FORMAT_NV12)
            .map_err(err("NV12 출력 확인"))?;
        if support & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT.0 as u32 == 0 {
            return Err(MediaError::Graphics(
                "Video Processor가 NV12 출력을 지원하지 않습니다".into(),
            ));
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

        let mut out_tex = None;
        d3d.device
            .CreateTexture2D(
                &tex_desc(
                    out_w,
                    out_h,
                    DXGI_FORMAT_NV12,
                    false,
                    D3D11_BIND_RENDER_TARGET.0 as u32,
                ),
                None,
                Some(&mut out_tex),
            )
            .map_err(err("NV12 출력 텍스처"))?;
        let out_tex: ID3D11Texture2D =
            out_tex.ok_or_else(|| MediaError::Graphics("NV12 출력 텍스처가 없습니다".into()))?;
        let out_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut out_view = None;
        gpu.vdev
            .CreateVideoProcessorOutputView(&out_tex, &enumerator, &out_desc, Some(&mut out_view))
            .map_err(err("출력 뷰"))?;

        let mut staging = None;
        d3d.device
            .CreateTexture2D(
                &tex_desc(out_w, out_h, DXGI_FORMAT_NV12, true, 0),
                None,
                Some(&mut staging),
            )
            .map_err(err("NV12 스테이징"))?;

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
        // 입력: sRGB 전범위 RGB, 출력: BT.601 limited range YCbCr.
        if let Ok(vctx1) = vctx.cast::<ID3D11VideoContext1>() {
            vctx1.VideoProcessorSetStreamColorSpace1(
                &vp,
                0,
                DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
            );
            vctx1.VideoProcessorSetOutputColorSpace1(
                &vp,
                DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601,
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
                &D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 1 << 4 },
            );
        }

        Ok(VpState {
            key: (src.w, src.h, out_w, out_h),
            _enumerator: enumerator,
            vp,
            in_view: in_view.ok_or_else(|| MediaError::Graphics("입력 뷰가 없습니다".into()))?,
            out_tex,
            out_view: out_view.ok_or_else(|| MediaError::Graphics("출력 뷰가 없습니다".into()))?,
            staging: staging
                .ok_or_else(|| MediaError::Graphics("NV12 스테이징이 없습니다".into()))?,
        })
    }
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
