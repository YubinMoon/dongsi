//! ffmpeg(libavcodec) FFI 도우미: 초기화, 로그 연결, 오류 문자열, 사전(AVDictionary).

use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Once};

use ffmpeg_sys_next as ff;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::core::Interface;

use crate::MediaError;
use crate::yuv::{Chroma, YuvView};

thread_local! {
    /// 인코더 탐색 중에는 실패 로그가 정상이므로 debug로 낮춘다.
    static QUIET: Cell<bool> = const { Cell::new(false) };
}

/// 이 스레드에서 ffmpeg 로그를 debug 수준으로 낮춘다(탐색용). 돌려받은 값으로 되돌린다.
pub(crate) fn set_quiet(quiet: bool) -> bool {
    QUIET.with(|q| q.replace(quiet))
}

/// ffmpeg 로그를 tracing(`target = "ffmpeg"`)으로 보낸다. 한 번만 설정한다.
pub(crate) fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // SAFETY: 전역 설정 함수. 콜백은 스레드 안전하게 작성했다.
        unsafe {
            ff::av_log_set_level(ff::AV_LOG_INFO);
            ff::av_log_set_callback(Some(log_callback));
        }
    });
}

unsafe extern "C" fn log_callback(
    avcl: *mut c_void,
    level: c_int,
    fmt: *const c_char,
    vl: ff::va_list,
) {
    if level > ff::AV_LOG_INFO {
        return;
    }
    let mut buf = [0 as c_char; 1024];
    let mut prefix: c_int = 1;
    // SAFETY: ffmpeg가 준 인자를 그대로 넘기고, 버퍼 길이를 정확히 준다.
    let line = unsafe {
        ff::av_log_format_line2(
            avcl,
            level,
            fmt,
            vl,
            buf.as_mut_ptr(),
            buf.len() as c_int,
            &mut prefix,
        );
        CStr::from_ptr(buf.as_ptr()).to_string_lossy()
    };
    let line = line.trim_end();
    if line.is_empty() {
        return;
    }
    let quiet = QUIET.with(|q| q.get());
    if quiet || level > ff::AV_LOG_WARNING {
        tracing::debug!(target: "ffmpeg", "{line}");
    } else if level > ff::AV_LOG_ERROR {
        tracing::warn!(target: "ffmpeg", "{line}");
    } else {
        tracing::error!(target: "ffmpeg", "{line}");
    }
}

/// ffmpeg 오류 코드를 문자열로.
pub(crate) fn err_str(code: c_int) -> String {
    let mut buf = [0 as c_char; 256];
    // SAFETY: 버퍼 길이를 정확히 준다.
    unsafe {
        if ff::av_strerror(code, buf.as_mut_ptr(), buf.len()) < 0 {
            return format!("ffmpeg 오류 {code}");
        }
        CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
    }
}

pub(crate) const EAGAIN: c_int = ff::AVERROR(ff::EAGAIN);
pub(crate) const EOF: c_int = ff::AVERROR_EOF;

pub(crate) fn cstring(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

/// AVDictionary 소유 래퍼.
pub(crate) struct Dict(*mut ff::AVDictionary);

impl Dict {
    pub fn new() -> Self {
        Dict(ptr::null_mut())
    }

    pub fn set(&mut self, key: &str, value: impl ToString) {
        let k = cstring(key);
        let v = cstring(&value.to_string());
        // SAFETY: 문자열을 복사해 넣는다(flags 0).
        unsafe {
            ff::av_dict_set(&mut self.0, k.as_ptr(), v.as_ptr(), 0);
        }
    }

    pub fn as_mut_ptr(&mut self) -> *mut *mut ff::AVDictionary {
        &mut self.0
    }

    /// 남은(인코더가 쓰지 않은) 항목 이름들.
    pub fn keys(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut e: *const ff::AVDictionaryEntry = ptr::null();
        let empty = cstring("");
        loop {
            // SAFETY: av_dict_iterate 대신 AV_DICT_IGNORE_SUFFIX로 모든 항목을 돈다.
            e = unsafe { ff::av_dict_get(self.0, empty.as_ptr(), e, ff::AV_DICT_IGNORE_SUFFIX) };
            if e.is_null() {
                break;
            }
            // SAFETY: 항목의 key는 NUL로 끝나는 문자열이다.
            out.push(
                unsafe { CStr::from_ptr((*e).key) }
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        out
    }
}

impl Drop for Dict {
    fn drop(&mut self) {
        // SAFETY: av_dict_free는 null도 받는다.
        unsafe { ff::av_dict_free(&mut self.0) };
    }
}

/// AVPacket 소유 래퍼.
pub(crate) struct Packet(pub *mut ff::AVPacket);

impl Packet {
    pub fn new() -> Option<Self> {
        // SAFETY: 실패하면 null.
        let p = unsafe { ff::av_packet_alloc() };
        (!p.is_null()).then_some(Packet(p))
    }

    /// 패킷 데이터 사본.
    pub fn data(&self) -> &[u8] {
        // SAFETY: receive_packet이 채운 패킷이다.
        unsafe {
            let p = &*self.0;
            if p.data.is_null() || p.size <= 0 {
                &[]
            } else {
                std::slice::from_raw_parts(p.data, p.size as usize)
            }
        }
    }

    /// 패킷의 `AV_PKT_DATA_QUALITY_STATS` side data 바이트(없으면 빈 슬라이스). 다음 `unref` 전까지만 유효하다.
    pub fn quality_stats(&self) -> &[u8] {
        let mut size = 0usize;
        // SAFETY: receive_packet이 채운 패킷이다. 돌려준 포인터는 패킷이 쥔 버퍼를 가리킨다.
        unsafe {
            let p = ff::av_packet_get_side_data(
                self.0,
                ff::AVPacketSideDataType::AV_PKT_DATA_QUALITY_STATS,
                &mut size,
            );
            if p.is_null() || size == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(p, size)
            }
        }
    }

    pub fn unref(&mut self) {
        // SAFETY: 유효한 패킷.
        unsafe { ff::av_packet_unref(self.0) };
    }
}

impl Drop for Packet {
    fn drop(&mut self) {
        // SAFETY: av_packet_alloc으로 만든 패킷.
        unsafe { ff::av_packet_free(&mut self.0) };
    }
}

/// AVFrame 소유 래퍼.
pub(crate) struct Frame(pub *mut ff::AVFrame);

impl Frame {
    pub fn new() -> Option<Self> {
        // SAFETY: 실패하면 null.
        let f = unsafe { ff::av_frame_alloc() };
        (!f.is_null()).then_some(Frame(f))
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        // SAFETY: av_frame_alloc으로 만든 프레임.
        unsafe { ff::av_frame_free(&mut self.0) };
    }
}

/// 프레임 참조 하나(refcount 버퍼를 가리키는 AVFrame 소유 래퍼). Drop = `av_frame_free`.
///
/// 픽셀 버퍼는 refcount라서 [`clone_ref`](Self::clone_ref)(`av_frame_clone`)은 데이터를 복사하지 않고 같은 버퍼를
/// 가리키는 새 프레임을 만든다. 버퍼는 마지막 참조가 없어질 때 원래 풀로 돌아간다. 그래서 프레임을 넘겨 받은 쪽
/// (simulcast 계층, 프레임을 쥐는 인코더)이 쥐고 있는 동안 워커가 같은 버퍼를 다시 쓸 수 없다. 워커는 프레임마다 풀에서
/// 새 버퍼를 받는다([`FramePool`]).
///
/// 버퍼 종류는 가리지 않는다: [`FramePool`]이 만든 시스템 메모리 프레임도, [`HwFramePool`]이 `av_hwframe_get_buffer`로
/// 만든 D3D11 하드웨어 프레임도 같은 방식으로 넘긴다.
pub(crate) struct FrameRef(*mut ff::AVFrame);

// SAFETY: 프레임 객체 하나는 한 번에 한 스레드만 쓴다(소유권을 옮길 뿐). 가리키는 버퍼는 refcount(원자적)다.
unsafe impl Send for FrameRef {}

impl FrameRef {
    pub fn as_ptr(&self) -> *const ff::AVFrame {
        self.0
    }

    pub fn as_mut_ptr(&mut self) -> *mut ff::AVFrame {
        self.0
    }

    pub fn width(&self) -> usize {
        // SAFETY: 살아 있는 프레임.
        unsafe { (*self.0).width.max(0) as usize }
    }

    pub fn height(&self) -> usize {
        // SAFETY: 살아 있는 프레임.
        unsafe { (*self.0).height.max(0) as usize }
    }

    /// `AVPixelFormat` 값.
    pub fn format(&self) -> i32 {
        // SAFETY: 살아 있는 프레임.
        unsafe { (*self.0).format }
    }

    /// 같은 버퍼를 가리키는 새 프레임 참조(데이터 복사 없음). 속성(pts, 색 정보 등)은 복사된다.
    pub fn clone_ref(&self) -> Option<FrameRef> {
        // SAFETY: 살아 있는 프레임. 실패하면 null.
        let f = unsafe { ff::av_frame_clone(self.0) };
        (!f.is_null()).then_some(FrameRef(f))
    }

    /// 이 프레임의 버퍼를 나만 쥐고 있는지(쓰기 가능).
    pub fn is_writable(&self) -> bool {
        // SAFETY: 살아 있는 프레임.
        unsafe { ff::av_frame_is_writable(self.0) > 0 }
    }

    /// 픽셀을 쓰기 위한 보기. 버퍼를 나만 쥐고 있고(다른 참조가 없고) 4:2:0 NV12·YUV420P일 때만 얻는다.
    pub fn view_mut(&mut self) -> Option<YuvView<'_>> {
        if !self.is_writable() {
            return None;
        }
        // SAFETY: 쓰기 가능한 프레임이라 이 버퍼를 읽는 다른 참조가 없다. 평면 크기는 풀이 같은 정렬로 계산해 만든
        // 버퍼 안에 있다(`av_image_fill_arrays`).
        unsafe {
            let f = &*self.0;
            let (w, h) = (f.width as usize, f.height as usize);
            let ls = |i: usize| f.linesize[i] as usize;
            let plane =
                |i: usize, rows: usize| std::slice::from_raw_parts_mut(f.data[i], ls(i) * rows);
            let chroma = if f.format == ff::AVPixelFormat::AV_PIX_FMT_NV12 as i32 {
                Chroma::Nv12 {
                    uv: plane(1, h / 2),
                    stride: ls(1),
                }
            } else if f.format == ff::AVPixelFormat::AV_PIX_FMT_YUV420P as i32 {
                Chroma::I420 {
                    u: plane(1, h / 2),
                    u_stride: ls(1),
                    v: plane(2, h / 2),
                    v_stride: ls(2),
                }
            } else {
                return None;
            };
            Some(YuvView {
                width: w,
                height: h,
                y: plane(0, h),
                y_stride: ls(0),
                chroma,
            })
        }
    }
}

impl Drop for FrameRef {
    fn drop(&mut self) {
        // SAFETY: av_frame_alloc·av_frame_clone으로 만든 프레임. 버퍼 참조도 같이 놓는다.
        unsafe { ff::av_frame_free(&mut self.0) };
    }
}

/// 풀 버퍼 줄 간격 정렬(바이트). SIMD 쓰기·읽기에 충분한 값.
const POOL_ALIGN: c_int = 64;
/// 버퍼 끝 여유(바이트). swscale·인코더의 SIMD가 끝을 조금 넘어 읽어도 안전하게(`av_frame_get_buffer`도 같은 이유로 둔다).
const POOL_PAD: usize = 64;
/// 풀이 만든 버퍼가 이만큼을 넘으면 경고한다(동시에 살아 있는 프레임 수의 최고치). 워커·주 인코더·계층(칸 1 + 인코딩
/// 1 + 인코더 안)을 합쳐도 이보다 한참 적다. 넘으면 어딘가가 프레임을 놓지 않는 것이다.
const POOL_WARN_BUFFERS: usize = 24;

/// 풀 버퍼를 새로 만들 때마다 센다(= 동시에 살아 있던 버퍼 수의 최고치).
unsafe extern "C" fn pool_alloc(opaque: *mut c_void, size: usize) -> *mut ff::AVBufferRef {
    // SAFETY: opaque는 `FramePool::new`가 넘긴 `Arc<AtomicUsize>`(풀이 살아 있는 동안 유효).
    unsafe { &*(opaque as *const AtomicUsize) }.fetch_add(1, Ordering::Relaxed);
    // SAFETY: 크기는 풀이 정했다. 0으로 채워 둔다(여유 구간이 쓰레기 값이 되지 않게).
    unsafe { ff::av_buffer_allocz(size) }
}

/// 풀이 없어질 때(마지막 버퍼가 돌아온 뒤) 세는 값을 놓는다.
unsafe extern "C" fn pool_opaque_free(opaque: *mut c_void) {
    // SAFETY: `Arc::into_raw`로 넘긴 값을 한 번만 놓는다.
    drop(unsafe { Arc::from_raw(opaque as *const AtomicUsize) });
}

/// 프레임 버퍼 풀: 크기·픽셀 형식 하나에 풀 하나(`AVBufferPool`).
///
/// - [`get`](Self::get)은 놀고 있는 버퍼를 꺼내고, 없으면 새로 만든다. **기다리지 않는다**(워커가 막히면 안 된다).
/// - 버퍼는 그 버퍼를 가리키는 마지막 [`FrameRef`]가 없어질 때 풀로 돌아온다. 풀 자체는 refcount라서
///   [`FramePool`]을 먼저 버려도(인코더를 다시 열 때) 아직 쥐고 있는 프레임은 그대로 쓸 수 있고, 마지막 프레임이 없어질 때
///   풀이 같이 해제된다.
/// - 풀이 만든 버퍼 수([`allocated`](Self::allocated))는 동시에 살아 있던 프레임 수의 최고치다. 계속 늘면 누수다.
pub(crate) struct FramePool {
    pool: *mut ff::AVBufferPool,
    width: i32,
    height: i32,
    format: ff::AVPixelFormat,
    allocated: Arc<AtomicUsize>,
    warned: bool,
}

// SAFETY: AVBufferPool은 내부 잠금으로 여러 스레드에서 쓸 수 있다(프레임은 다른 스레드에서 놓인다).
unsafe impl Send for FramePool {}

impl FramePool {
    /// `width`×`height`, `format`(NV12·YUV420P) 프레임을 위한 풀. 버퍼는 처음 `get`할 때 만든다.
    pub fn new(width: u32, height: u32, format: ff::AVPixelFormat) -> Option<Self> {
        let (w, h) = (width as c_int, height as c_int);
        // SAFETY: 크기 계산만 한다.
        let size = unsafe { ff::av_image_get_buffer_size(format, w, h, POOL_ALIGN) };
        if size <= 0 {
            return None;
        }
        let allocated = Arc::new(AtomicUsize::new(0));
        let opaque = Arc::into_raw(allocated.clone()) as *mut c_void;
        // SAFETY: 콜백은 위에서 정의했고 opaque는 풀이 없어질 때 `pool_opaque_free`가 놓는다.
        let pool = unsafe {
            ff::av_buffer_pool_init2(
                size as usize + POOL_PAD,
                opaque,
                Some(pool_alloc),
                Some(pool_opaque_free),
            )
        };
        if pool.is_null() {
            // SAFETY: 위에서 넘긴 값을 되찾아 놓는다(풀이 만들어지지 않아 콜백이 불리지 않는다).
            drop(unsafe { Arc::from_raw(opaque as *const AtomicUsize) });
            return None;
        }
        Some(FramePool {
            pool,
            width: w,
            height: h,
            format,
            allocated,
            warned: false,
        })
    }

    /// 지금까지 만든 버퍼 수(동시에 살아 있던 프레임 수의 최고치).
    pub fn allocated(&self) -> usize {
        self.allocated.load(Ordering::Relaxed)
    }

    /// 풀 버퍼를 가리키는 새 프레임. 내용은 이전 프레임의 것일 수 있으니 전부 덮어써야 한다. 나만 쥐고 있으므로 쓸 수 있다.
    pub fn get(&mut self) -> Option<FrameRef> {
        // SAFETY: 살아 있는 풀. 실패하면 null.
        let mut buf = unsafe { ff::av_buffer_pool_get(self.pool) };
        if buf.is_null() {
            return None;
        }
        // SAFETY: 실패하면 null.
        let f = unsafe { ff::av_frame_alloc() };
        if f.is_null() {
            // SAFETY: 위에서 받은 버퍼를 풀에 돌려준다.
            unsafe { ff::av_buffer_unref(&mut buf) };
            return None;
        }
        // 버퍼를 프레임에 먼저 넘기면 이후 실패해도 FrameRef가 해제한다.
        let frame = FrameRef(f);
        // SAFETY: 프레임은 방금 만들었고 버퍼는 크기에 맞게 만든 것이다. 평면 위치·줄 간격은 크기를 잰 것과 같은
        // 인자로 채운다.
        unsafe {
            let fr = &mut *frame.0;
            fr.buf[0] = buf;
            fr.format = self.format as c_int;
            fr.width = self.width;
            fr.height = self.height;
            let r = ff::av_image_fill_arrays(
                fr.data.as_mut_ptr(),
                fr.linesize.as_mut_ptr(),
                (*buf).data,
                self.format,
                self.width,
                self.height,
                POOL_ALIGN,
            );
            if r < 0 {
                return None;
            }
        }
        let n = self.allocated();
        if n > POOL_WARN_BUFFERS && !self.warned {
            self.warned = true;
            tracing::warn!(
                buffers = n,
                width = self.width,
                height = self.height,
                "프레임 버퍼가 계속 늘고 있습니다(어딘가가 프레임을 놓지 않는 것 같습니다)"
            );
        }
        Some(frame)
    }
}

impl Drop for FramePool {
    fn drop(&mut self) {
        tracing::debug!(
            buffers = self.allocated(),
            width = self.width,
            height = self.height,
            "프레임 풀 해제(만든 버퍼 수 = 동시에 살아 있던 최고치)"
        );
        // SAFETY: 풀 참조 하나를 놓는다. 아직 쥔 버퍼가 있으면 그 버퍼가 돌아올 때 풀이 해제된다.
        unsafe { ff::av_buffer_pool_uninit(&mut self.pool) };
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// D3D11 하드웨어 프레임(NVENC zero-copy, D-099 2단계)
// ---------------------------------------------------------------------------------------------------------------------

/// [`HwFramePool::new`]가 거는 텍스처 BindFlags. NV12 텍스처를 Video Processor 출력(`D3D11_BIND_RENDER_TARGET`)으로 쓰고
/// NVENC가 그대로 등록·인코딩한다(이 PC의 RTX 3060에서 시험으로 확인, `ff::tests`). `D3D11_BIND_VIDEO_ENCODER`는 걸지 않는다.
pub(crate) const HW_POOL_BIND_FLAGS: u32 = D3D11_BIND_RENDER_TARGET.0 as u32;

/// 프로세스 안에서 지금 살아 있는 hw 풀 프레임(텍스처) 수.
static HW_LIVE: AtomicUsize = AtomicUsize::new(0);
/// 풀 세대 번호 발급기(프로세스 안 단조 증가, 0은 쓰지 않는다).
static HW_GENERATION: AtomicU64 = AtomicU64::new(1);

/// 프로세스 전체에서 지금 살아 있는 hw 풀 프레임 수(시험용, 파이프라인을 내린 뒤 0이어야 한다).
///
/// 텍스처 하나가 한 번 센다: `clone_ref`·인코더가 쥔 참조는 같은 텍스처를 가리키므로 따로 세지 않고, 마지막 참조가
/// 없어질 때 줄어든다.
pub(crate) fn live_hw_frames() -> usize {
    HW_LIVE.load(Ordering::Relaxed)
}

/// hw 프레임의 `opaque_ref`에 달아 두는 표지. 프레임 참조(`av_frame_clone`·`av_frame_ref`)가 `opaque_ref`를 같이 참조하므로
/// 이 값은 텍스처 하나당 하나이고, 마지막 참조가 풀릴 때 [`token_free`]가 살아 있는 수를 줄인다.
#[repr(C)]
struct PoolToken {
    magic: u64,
    generation: u64,
    pool_live: Arc<AtomicUsize>,
}

const TOKEN_MAGIC: u64 = 0x646f_6e67_7369_4857; // "dongsiHW"

unsafe extern "C" fn token_free(_opaque: *mut c_void, data: *mut u8) {
    // SAFETY: `HwFramePool::get`이 `Box::into_raw`로 넘긴 값을 한 번만 놓는다.
    let token = unsafe { Box::from_raw(data as *mut PoolToken) };
    token.pool_live.fetch_sub(1, Ordering::Relaxed);
    HW_LIVE.fetch_sub(1, Ordering::Relaxed);
}

/// 우리 D3D11 장치를 감싼 `AV_HWDEVICE_TYPE_D3D11VA` 장치 참조.
///
/// 장치·즉시 컨텍스트는 AddRef해서 넘기고 ffmpeg가 장치를 해제할 때 Release한다(`hwcontext_d3d11va.h`). lock/unlock 콜백은
/// 비워 둬서 기본 mutex를 쓴다: nvenc는 D3D11에서 잠금을 부르지 않고, hwcontext가 잠그는 것은 `av_hwframe_transfer_data`뿐이다.
/// 장치의 스레드 보호는 `create_d3d`가 이미 켰다(`SetMultithreadProtected`).
pub(crate) struct HwDevice(*mut ff::AVBufferRef);

// SAFETY: AVBufferRef의 refcount는 원자적이고, 장치 컨텍스트는 초기화 뒤 바뀌지 않는다.
unsafe impl Send for HwDevice {}
unsafe impl Sync for HwDevice {}

impl HwDevice {
    pub fn new(device: &ID3D11Device, context: &ID3D11DeviceContext) -> Result<Self, MediaError> {
        init();
        // SAFETY: 할당·초기화 순서는 hwcontext.h의 사용법을 따른다. 실패하면 할당한 참조를 놓는다(장치 해제 때 ffmpeg가
        // 우리가 넣은 COM 참조를 Release한다).
        unsafe {
            let mut buf = ff::av_hwdevice_ctx_alloc(ff::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA);
            if buf.is_null() {
                return Err(MediaError::Graphics(
                    "av_hwdevice_ctx_alloc(D3D11VA) 실패".into(),
                ));
            }
            let dev_ctx = (*buf).data as *mut ff::AVHWDeviceContext;
            let hwctx = (*dev_ctx).hwctx as *mut ff::AVD3D11VADeviceContext;
            // clone()이 AddRef, into_raw()가 그 참조를 ffmpeg에 넘긴다.
            (*hwctx).device = device.clone().into_raw() as *mut ff::ID3D11Device;
            (*hwctx).device_context = context.clone().into_raw() as *mut ff::ID3D11DeviceContext;
            let r = ff::av_hwdevice_ctx_init(buf);
            if r < 0 {
                ff::av_buffer_unref(&mut buf);
                return Err(MediaError::Graphics(format!(
                    "av_hwdevice_ctx_init(D3D11VA): {}",
                    err_str(r)
                )));
            }
            Ok(HwDevice(buf))
        }
    }
}

impl Drop for HwDevice {
    fn drop(&mut self) {
        // SAFETY: 참조 하나를 놓는다. 풀(frames ctx)과 인코더가 쥔 참조는 따로라서 그들이 놓을 때까지 장치는 산다.
        unsafe { ff::av_buffer_unref(&mut self.0) };
    }
}

/// D3D11 NV12 hwframes 풀(`AV_PIX_FMT_D3D11`, sw_format NV12). 크기 하나에 풀 하나.
///
/// - `initial_pool_size = 0`: 텍스처를 하나씩 따로 만들고(array texture 아님) 필요할 때 늘어난다. 고갈이 없고
///   [`get`](Self::get)은 기다리지 않는다. 다 쓴 텍스처(마지막 [`FrameRef`]가 없어진 것)는 풀로 돌아와 재사용된다.
/// - 인코더 하나는 평생 frames ctx 하나에 묶인다. 같은 풀의 [`frames_ctx_ref`](Self::frames_ctx_ref)로 연 인코더들은 풀이
///   다시 만들어지면(세대가 바뀌면) 모두 다시 열어야 한다. nvenc의 등록 캐시가 텍스처 포인터 비교라서 다른 풀의 프레임을 같은
///   인코더에 보내면 안 된다.
/// - 풀을 먼저 버려도 아직 쥔 프레임은 그대로 쓸 수 있다(프레임이 frames ctx 참조를 쥔다).
/// - 프레임마다 풀에서 새 텍스처를 받는다. NVENC가 매핑한 동안 텍스처에 쓰면 안 되므로 텍스처를 직접 재사용하지 않는다.
pub(crate) struct HwFramePool {
    frames: *mut ff::AVBufferRef,
    width: u32,
    height: u32,
    generation: u64,
    live: Arc<AtomicUsize>,
}

// SAFETY: frames ctx는 refcount(원자적)이고, 풀 안의 텍스처 할당은 AVBufferPool이 잠근다. 프레임은 다른 스레드에서 놓인다.
unsafe impl Send for HwFramePool {}

impl HwFramePool {
    /// `width`×`height` NV12 풀. 텍스처 BindFlags는 [`HW_POOL_BIND_FLAGS`]. 텍스처는 처음 [`get`](Self::get)할 때 만든다.
    pub fn new(dev: &HwDevice, width: u32, height: u32) -> Result<Self, MediaError> {
        Self::with_bind_flags(dev, width, height, HW_POOL_BIND_FLAGS)
    }

    /// [`new`](Self::new)와 같되 텍스처 `D3D11_TEXTURE2D_DESC.BindFlags`를 직접 정한다(예: `RENDER_TARGET | VIDEO_ENCODER`).
    pub fn with_bind_flags(
        dev: &HwDevice,
        width: u32,
        height: u32,
        bind_flags: u32,
    ) -> Result<Self, MediaError> {
        let fail = |m: String| MediaError::Graphics(format!("hwframes: {m}"));
        // SAFETY: 할당·초기화 순서는 hwcontext.h의 사용법을 따른다. 실패하면 참조를 놓는다.
        unsafe {
            let mut frames = ff::av_hwframe_ctx_alloc(dev.0);
            if frames.is_null() {
                return Err(fail("av_hwframe_ctx_alloc 실패".into()));
            }
            let ctx = (*frames).data as *mut ff::AVHWFramesContext;
            (*ctx).format = ff::AVPixelFormat::AV_PIX_FMT_D3D11;
            (*ctx).sw_format = ff::AVPixelFormat::AV_PIX_FMT_NV12;
            (*ctx).width = width as c_int;
            (*ctx).height = height as c_int;
            (*ctx).initial_pool_size = 0;
            let hwctx = (*ctx).hwctx as *mut ff::AVD3D11VAFramesContext;
            (*hwctx).BindFlags = bind_flags;
            let r = ff::av_hwframe_ctx_init(frames);
            if r < 0 {
                ff::av_buffer_unref(&mut frames);
                return Err(fail(format!("av_hwframe_ctx_init: {}", err_str(r))));
            }
            Ok(HwFramePool {
                frames,
                width,
                height,
                generation: HW_GENERATION.fetch_add(1, Ordering::Relaxed),
                live: Arc::new(AtomicUsize::new(0)),
            })
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// 풀마다 다른 번호(프로세스 안 단조 증가). 인코더 다시 열기 판단용.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 새 텍스처 프레임(`av_hwframe_get_buffer`). 기다리지 않는다. 내용은 이전 프레임의 것일 수 있다.
    pub fn get(&mut self) -> Result<FrameRef, MediaError> {
        let fail = |m: String| MediaError::Graphics(format!("hw 프레임: {m}"));
        // SAFETY: 살아 있는 frames ctx. 프레임은 FrameRef가 곧바로 쥐어 어느 경로로 나가도 해제된다.
        unsafe {
            let f = ff::av_frame_alloc();
            if f.is_null() {
                return Err(fail("av_frame_alloc 실패".into()));
            }
            let frame = FrameRef(f);
            let r = ff::av_hwframe_get_buffer(self.frames, f, 0);
            if r < 0 {
                return Err(fail(format!("av_hwframe_get_buffer: {}", err_str(r))));
            }
            let token = Box::into_raw(Box::new(PoolToken {
                magic: TOKEN_MAGIC,
                generation: self.generation,
                pool_live: self.live.clone(),
            }));
            let opaque = ff::av_buffer_create(
                token as *mut u8,
                size_of::<PoolToken>(),
                Some(token_free),
                ptr::null_mut(),
                0,
            );
            if opaque.is_null() {
                drop(Box::from_raw(token));
                return Err(fail("av_buffer_create 실패".into()));
            }
            // 프레임이 풀리면 opaque_ref도 풀리고 token_free가 센 값을 줄인다.
            self.live.fetch_add(1, Ordering::Relaxed);
            HW_LIVE.fetch_add(1, Ordering::Relaxed);
            (*f).opaque_ref = opaque;
            Ok(frame)
        }
    }

    /// 이 풀에서 받은 프레임 중 지금 참조가 남은 텍스처 수.
    pub fn live(&self) -> usize {
        self.live.load(Ordering::Relaxed)
    }

    /// 인코더를 열 때 `AVCodecContext.hw_frames_ctx`에 넣을 새 참조(`av_buffer_ref`). 호출자가 소유한다(코덱 컨텍스트에
    /// 넣으면 `avcodec_free_context`가 놓는다).
    pub fn frames_ctx_ref(&self) -> *mut ff::AVBufferRef {
        // SAFETY: 살아 있는 frames ctx. 실패하면 null.
        unsafe { ff::av_buffer_ref(self.frames) }
    }
}

impl Drop for HwFramePool {
    fn drop(&mut self) {
        // SAFETY: 풀 참조 하나를 놓는다. 쥔 프레임·인코더가 있으면 그들이 놓을 때 frames ctx가 해제된다.
        unsafe { ff::av_buffer_unref(&mut self.frames) };
    }
}

impl FrameRef {
    /// D3D11 hw 프레임이면 (텍스처, array index). `data[0]` = `ID3D11Texture2D*`, `data[1]` = index(우리 풀은 0).
    /// 텍스처는 AddRef한 참조라서 프레임보다 오래 쥐어도 되지만, 프레임이 풀려 풀로 돌아간 텍스처는 다른 프레임이 다시 쓴다.
    pub fn d3d11_texture(&self) -> Option<(ID3D11Texture2D, u32)> {
        // SAFETY: 살아 있는 프레임. D3D11 프레임의 data[0]은 유효한 ID3D11Texture2D 포인터다.
        unsafe {
            let f = &*self.0;
            if f.format != ff::AVPixelFormat::AV_PIX_FMT_D3D11 as c_int || f.data[0].is_null() {
                return None;
            }
            let raw = f.data[0] as *mut c_void;
            let tex = ID3D11Texture2D::from_raw_borrowed(&raw)?.clone();
            Some((tex, f.data[1] as usize as u32))
        }
    }

    /// 이 프레임이 나온 [`HwFramePool`]의 세대(hw 프레임이 아니면 `None`). `clone_ref` 참조도 같은 값이다.
    pub fn pool_generation(&self) -> Option<u64> {
        // SAFETY: 살아 있는 프레임. opaque_ref는 크기와 magic으로 우리 표지인지 확인한 뒤에만 읽는다.
        unsafe {
            let f = &*self.0;
            if f.format != ff::AVPixelFormat::AV_PIX_FMT_D3D11 as c_int || f.opaque_ref.is_null() {
                return None;
            }
            let b = &*f.opaque_ref;
            if b.size != size_of::<PoolToken>() || b.data.is_null() {
                return None;
            }
            let t = &*(b.data as *const PoolToken);
            (t.magic == TOKEN_MAGIC).then_some(t.generation)
        }
    }
}

/// hw 프레임 풀의 frames ctx(`AVHWFramesContext`) 참조 하나와 그 풀의 세대. 인코더를 hw 입력으로 열 때 넘긴다.
///
/// 인코더 하나는 평생 frames ctx 하나에 묶이므로(nvenc의 등록 캐시가 텍스처 포인터 비교다) 인코더는 연 세대를 기억하고,
/// 다른 세대의 프레임은 받지 않는다. 참조라서 풀(`HwFramePool`)을 먼저 버려도 유효하다.
pub(crate) struct HwFramesRef {
    buf: *mut ff::AVBufferRef,
    generation: u64,
}

// SAFETY: AVBufferRef의 refcount는 원자적이고, 가리키는 frames ctx는 초기화 뒤 바뀌지 않는다.
unsafe impl Send for HwFramesRef {}

impl HwFramesRef {
    /// 풀의 frames ctx. 실패하면 `None`.
    pub fn of_pool(pool: &HwFramePool) -> Option<Self> {
        let buf = pool.frames_ctx_ref();
        (!buf.is_null()).then(|| HwFramesRef {
            buf,
            generation: pool.generation(),
        })
    }

    /// hw 프레임이 속한 풀의 frames ctx(`frame->hw_frames_ctx`의 새 참조). 우리 풀의 hw 프레임이 아니면 `None`.
    pub fn of_frame(frame: &FrameRef) -> Option<Self> {
        let generation = frame.pool_generation()?;
        // SAFETY: 살아 있는 hw 프레임. hw_frames_ctx는 av_hwframe_get_buffer가 채운 프레임 자신의 참조다.
        let buf = unsafe {
            let f = &*frame.as_ptr();
            if f.hw_frames_ctx.is_null() {
                return None;
            }
            ff::av_buffer_ref(f.hw_frames_ctx)
        };
        (!buf.is_null()).then_some(HwFramesRef { buf, generation })
    }

    /// 이 frames ctx를 만든 풀의 세대.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 코덱 컨텍스트의 `hw_frames_ctx`에 넣을 새 참조(`av_buffer_ref`). 호출자가 소유한다. 실패하면 null.
    pub fn new_ref(&self) -> *mut ff::AVBufferRef {
        // SAFETY: 살아 있는 참조.
        unsafe { ff::av_buffer_ref(self.buf) }
    }
}

impl Drop for HwFramesRef {
    fn drop(&mut self) {
        // SAFETY: 이 객체가 쥔 참조 하나를 놓는다.
        unsafe { ff::av_buffer_unref(&mut self.buf) };
    }
}

/// AVCodecContext 소유 래퍼.
pub(crate) struct CodecCtx(pub *mut ff::AVCodecContext);

impl Drop for CodecCtx {
    fn drop(&mut self) {
        // SAFETY: avcodec_alloc_context3으로 만든 컨텍스트.
        unsafe { ff::avcodec_free_context(&mut self.0) };
    }
}

// SAFETY: ffmpeg 객체는 동시에 쓰지만 않으면 스레드 사이에서 옮겨도 된다.
unsafe impl Send for Packet {}
unsafe impl Send for Frame {}
unsafe impl Send for CodecCtx {}

#[cfg(test)]
mod tests {
    use std::mem::ManuallyDrop;
    use std::sync::Mutex;
    use std::time::Instant;

    use windows::Win32::Graphics::Direct3D11::{
        D3D11_BIND_SHADER_RESOURCE, D3D11_BIND_VIDEO_ENCODER, D3D11_CPU_ACCESS_READ,
        D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV,
        D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
        D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
        D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
        D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
        D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D, ID3D11VideoContext,
        ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
        ID3D11VideoProcessorInputView,
    };
    use windows::Win32::Graphics::Dxgi::Common::{
        DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
    };
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;

    use super::*;
    use crate::d3d::{D3d, create_d3d};

    /// hw 풀 프레임 수는 프로세스 전체 값이라 hw 시험을 한 번에 하나씩 돌린다.
    static HW_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        HW_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// D3D11 장치를 못 만드는 드문 환경이면 `None`(시험을 건너뛴다).
    fn device() -> Option<(D3d, HwDevice)> {
        let d3d = match create_d3d() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("D3D11 장치를 만들 수 없어 건너뜁니다: {e}");
                return None;
            }
        };
        let dev = HwDevice::new(&d3d.device, &d3d.context).expect("hwdevice");
        Some((d3d, dev))
    }

    fn desc_of(tex: &ID3D11Texture2D) -> D3D11_TEXTURE2D_DESC {
        let mut d = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: 유효한 텍스처.
        unsafe { tex.GetDesc(&mut d) };
        d
    }

    /// BGRA 원본을 NV12 풀 텍스처에 Video Processor로 그리는 시험용 도우미(텍스처 내용 채우기와, 풀 텍스처가
    /// Video Processor 출력 뷰가 되는지 확인).
    struct Blt {
        d3d: D3d,
        vctx: ID3D11VideoContext,
        vdev: ID3D11VideoDevice,
        enumerator: ID3D11VideoProcessorEnumerator,
        vp: ID3D11VideoProcessor,
        src: ID3D11Texture2D,
        in_view: ID3D11VideoProcessorInputView,
        w: u32,
        h: u32,
    }

    impl Blt {
        fn new(d3d: &D3d, w: u32, h: u32) -> Result<Self, String> {
            let e = |what: &str| {
                let what = what.to_string();
                move |e: windows::core::Error| format!("{what}: {e}")
            };
            let vdev: ID3D11VideoDevice = d3d.device.cast().map_err(e("VideoDevice"))?;
            let vctx: ID3D11VideoContext = d3d.context.cast().map_err(e("VideoContext"))?;
            let rate = DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            };
            let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: rate,
                InputWidth: w,
                InputHeight: h,
                OutputFrameRate: rate,
                OutputWidth: w,
                OutputHeight: h,
                Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
            };
            // SAFETY: 설명자와 출력 포인터가 유효하고 모든 객체가 같은 장치의 것이다.
            unsafe {
                let enumerator = vdev
                    .CreateVideoProcessorEnumerator(&content)
                    .map_err(e("Enumerator"))?;
                let vp = vdev
                    .CreateVideoProcessor(&enumerator, 0)
                    .map_err(e("VideoProcessor"))?;
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: w,
                    Height: h,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                };
                let mut src = None;
                d3d.device
                    .CreateTexture2D(&desc, None, Some(&mut src))
                    .map_err(e("BGRA 원본"))?;
                let src = src.ok_or("BGRA 원본이 없습니다")?;
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
                vdev.CreateVideoProcessorInputView(&src, &enumerator, &in_desc, Some(&mut in_view))
                    .map_err(e("입력 뷰"))?;
                Ok(Blt {
                    d3d: d3d.clone(),
                    vctx,
                    vdev,
                    enumerator,
                    vp,
                    src,
                    in_view: in_view.ok_or("입력 뷰가 없습니다")?,
                    w,
                    h,
                })
            }
        }

        /// 풀 텍스처에 대한 출력 뷰를 만든다(실패하면 그 BindFlags로는 Video Processor 출력이 안 된다).
        fn output_view(
            &self,
            tex: &ID3D11Texture2D,
        ) -> Result<windows::Win32::Graphics::Direct3D11::ID3D11VideoProcessorOutputView, String>
        {
            let desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                },
            };
            let mut view = None;
            // SAFETY: 같은 장치의 텍스처·열거자.
            unsafe {
                self.vdev.CreateVideoProcessorOutputView(
                    tex,
                    &self.enumerator,
                    &desc,
                    Some(&mut view),
                )
            }
            .map_err(|e| format!("출력 뷰: {e}"))?;
            view.ok_or_else(|| "출력 뷰가 없습니다".to_string())
        }

        /// 원본을 `seed`에 따라 다른 그림으로 채우고 `tex`에 변환해 그린다.
        fn fill(&mut self, tex: &ID3D11Texture2D, seed: u32) -> Result<(), String> {
            let (w, h) = (self.w as usize, self.h as usize);
            let mut px = vec![0u8; w * h * 4];
            for y in 0..h {
                for x in 0..w {
                    let i = (y * w + x) * 4;
                    px[i] = (x as u32 * 255 / w as u32 + seed * 7) as u8;
                    px[i + 1] = (y as u32 * 255 / h as u32) as u8;
                    px[i + 2] = (((x + y) / 2) as u32 + seed * 3) as u8;
                    px[i + 3] = 255;
                }
            }
            let view = self.output_view(tex)?;
            // SAFETY: 원본 텍스처에 호출 동안 살아 있는 버퍼를 올린다. ManuallyDrop은 Blt 뒤에 직접 해제한다.
            unsafe {
                self.d3d.context.UpdateSubresource(
                    &self.src,
                    0,
                    None,
                    px.as_ptr() as *const _,
                    (w * 4) as u32,
                    0,
                );
                let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
                    Enable: true.into(),
                    pInputSurface: ManuallyDrop::new(Some(self.in_view.clone())),
                    ..Default::default()
                };
                let r =
                    self.vctx
                        .VideoProcessorBlt(&self.vp, &view, 0, std::slice::from_ref(&stream));
                ManuallyDrop::drop(&mut stream.pInputSurface);
                r.map_err(|e| format!("VideoProcessorBlt: {e}"))
            }
        }
    }

    /// NV12 텍스처의 휘도 평면(Y)에서 (최소, 최대)를 읽는다.
    fn luma_range(d3d: &D3d, tex: &ID3D11Texture2D) -> (u8, u8) {
        let mut d = desc_of(tex);
        d.Usage = D3D11_USAGE_STAGING;
        d.BindFlags = 0;
        d.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        d.MiscFlags = 0;
        // SAFETY: 같은 크기·형식의 스테이징에 복사해 읽고 Unmap한다.
        unsafe {
            let mut st = None;
            d3d.device.CreateTexture2D(&d, None, Some(&mut st)).unwrap();
            let st = st.unwrap();
            d3d.context.CopyResource(&st, tex);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            d3d.context
                .Map(&st, 0, D3D11_MAP_READ, 0, Some(&mut m))
                .unwrap();
            let (mut lo, mut hi) = (u8::MAX, u8::MIN);
            for y in 0..d.Height as usize {
                let row = std::slice::from_raw_parts(
                    (m.pData as *const u8).add(y * m.RowPitch as usize),
                    d.Width as usize,
                );
                for &v in row {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            d3d.context.Unmap(&st, 0);
            (lo, hi)
        }
    }

    #[test]
    fn hw_pool_textures_are_nv12_render_targets() {
        let _g = lock();
        let Some((_d3d, dev)) = device() else { return };
        let mut pool = HwFramePool::new(&dev, 640, 360).unwrap();
        assert_eq!((pool.width(), pool.height()), (640, 360));
        let frames: Vec<FrameRef> = (0..6).map(|_| pool.get().unwrap()).collect();
        let mut raws = Vec::new();
        for f in &frames {
            assert_eq!(f.format(), ff::AVPixelFormat::AV_PIX_FMT_D3D11 as i32);
            assert_eq!((f.width(), f.height()), (640, 360));
            let (tex, index) = f.d3d11_texture().expect("D3D11 텍스처");
            assert_eq!(index, 0, "array texture를 쓰지 않는다");
            let d = desc_of(&tex);
            assert_eq!((d.Width, d.Height), (640, 360));
            assert_eq!(d.Format, DXGI_FORMAT_NV12);
            assert_eq!(d.BindFlags, HW_POOL_BIND_FLAGS);
            assert_eq!(d.BindFlags, D3D11_BIND_RENDER_TARGET.0 as u32);
            assert_eq!((d.ArraySize, d.MipLevels), (1, 1));
            raws.push(tex.as_raw());
        }
        raws.sort();
        raws.dedup();
        assert_eq!(raws.len(), 6, "프레임마다 서로 다른 텍스처");
        assert_eq!(pool.live(), 6);
        drop(frames);
        assert_eq!(pool.live(), 0);
        drop(pool);
        assert_eq!(live_hw_frames(), 0);
    }

    #[test]
    fn hw_pool_tracks_live_frames_and_outlives_the_pool() {
        let _g = lock();
        let Some((_d3d, dev)) = device() else { return };
        let base = live_hw_frames();
        assert_eq!(base, 0);
        let mut pool = HwFramePool::new(&dev, 320, 240).unwrap();
        let a = pool.get().unwrap();
        let b = pool.get().unwrap();
        assert_eq!((pool.live(), live_hw_frames()), (2, 2));

        // clone_ref는 같은 텍스처·같은 세대. 텍스처 수는 늘지 않는다.
        let a2 = a.clone_ref().unwrap();
        assert_eq!(a2.pool_generation(), Some(pool.generation()));
        assert_eq!(
            a2.d3d11_texture().unwrap().0.as_raw(),
            a.d3d11_texture().unwrap().0.as_raw()
        );
        assert!(!a.is_writable(), "참조가 둘이면 쓰기 불가");
        assert_eq!((pool.live(), live_hw_frames()), (2, 2));
        drop(a);
        assert_eq!(pool.live(), 2, "복제 참조가 남았다");
        drop(b);
        assert_eq!(pool.live(), 1);

        // 다 쓴 텍스처는 풀로 돌아와 재사용되고(새로 만들지 않는다), 한 번에 하나만 살아 있다.
        let raw_a2 = a2.d3d11_texture().unwrap().0.as_raw();
        drop(a2);
        assert_eq!(pool.live(), 0);
        let mut seen = vec![raw_a2];
        for _ in 0..4 {
            let f = pool.get().unwrap();
            seen.push(f.d3d11_texture().unwrap().0.as_raw());
        }
        seen.sort();
        seen.dedup();
        assert!(seen.len() <= 2, "텍스처가 재사용된다: {}", seen.len());

        // 풀이 달라지면 세대가 다르다(단조 증가).
        let pool2 = HwFramePool::new(&dev, 320, 240).unwrap();
        assert!(pool2.generation() > pool.generation());
        let pool3 = HwFramePool::new(&dev, 640, 360).unwrap();
        assert!(pool3.generation() > pool2.generation());

        // 풀을 버려도 남은 프레임은 유효하다(텍스처를 읽고 복제할 수 있다). 마지막 참조가 없어지면 0이 된다.
        let mut pool = pool;
        let keep = pool.get().unwrap();
        let keep2 = keep.clone_ref().unwrap();
        let generation = pool.generation();
        drop(pool);
        assert_eq!(live_hw_frames(), 1);
        assert_eq!(keep.pool_generation(), Some(generation));
        let (tex, _) = keep2.d3d11_texture().unwrap();
        assert_eq!((desc_of(&tex).Width, desc_of(&tex).Height), (320, 240));
        drop(keep);
        assert_eq!(live_hw_frames(), 1);
        drop(keep2);
        drop(tex);
        assert_eq!(live_hw_frames(), 0);

        // 시스템 메모리 프레임은 hw 프레임이 아니다.
        let mut sys = FramePool::new(64, 64, ff::AVPixelFormat::AV_PIX_FMT_NV12).unwrap();
        let f = sys.get().unwrap();
        assert!(f.d3d11_texture().is_none());
        assert_eq!(f.pool_generation(), None);
    }

    #[test]
    fn hw_pool_accepts_video_encoder_bind_flag() {
        let _g = lock();
        let Some((_d3d, dev)) = device() else { return };
        let flags = D3D11_BIND_RENDER_TARGET.0 as u32 | D3D11_BIND_VIDEO_ENCODER.0 as u32;
        let mut pool = HwFramePool::with_bind_flags(&dev, 640, 360, flags).unwrap();
        let f = pool.get().unwrap();
        let d = desc_of(&f.d3d11_texture().unwrap().0);
        assert_eq!(d.BindFlags, flags);
        assert_eq!(d.Format, DXGI_FORMAT_NV12);
    }

    #[test]
    fn hw_pool_texture_is_a_video_processor_target() {
        let _g = lock();
        let Some((d3d, dev)) = device() else { return };
        if !d3d.video_support {
            eprintln!("Video Processor가 없어 건너뜁니다");
            return;
        }
        let mut pool = HwFramePool::new(&dev, 640, 360).unwrap();
        let mut blt = Blt::new(&d3d, 640, 360).expect("Video Processor");
        let f = pool.get().unwrap();
        let (tex, _) = f.d3d11_texture().unwrap();
        blt.fill(&tex, 1)
            .expect("RENDER_TARGET만 건 풀 텍스처에 Blt");
        let (lo, hi) = luma_range(&d3d, &tex);
        assert!(hi - lo > 100, "Blt가 휘도를 채웠다: {lo}..{hi}");
    }

    // ----- NVENC (full-tests) -----

    fn is_nvidia(d3d: &D3d) -> bool {
        // SAFETY: 유효한 장치의 어댑터 정보.
        unsafe {
            let dxgi: IDXGIDevice = d3d.device.cast().unwrap();
            let adapter = dxgi.GetAdapter().unwrap();
            adapter
                .GetDesc()
                .map(|d| d.VendorId == 0x10DE)
                .unwrap_or(false)
        }
    }

    /// h264_nvenc를 pix_fmt D3D11로 연다(`frames`는 소유권을 넘긴다: 컨텍스트가 해제한다). encoder.rs의 NVENC 옵션과 같다.
    fn open_nvenc(frames: *mut ff::AVBufferRef, w: u32, h: u32) -> Result<CodecCtx, String> {
        init();
        let name = cstring("h264_nvenc");
        // SAFETY: 열기 순서는 encoder.rs와 같다. frames는 컨텍스트가 소유한다.
        unsafe {
            let codec = ff::avcodec_find_encoder_by_name(name.as_ptr());
            if codec.is_null() {
                ff::av_buffer_unref(&mut { frames });
                return Err("h264_nvenc 없음".into());
            }
            let ctx = CodecCtx(ff::avcodec_alloc_context3(codec));
            let c = &mut *ctx.0;
            c.hw_frames_ctx = frames;
            c.pix_fmt = ff::AVPixelFormat::AV_PIX_FMT_D3D11;
            c.width = w as i32;
            c.height = h as i32;
            c.time_base = ff::AVRational { num: 1, den: 60 };
            c.framerate = ff::AVRational { num: 60, den: 1 };
            c.bit_rate = 8_000_000;
            c.rc_max_rate = 8_000_000;
            c.rc_buffer_size = 266_666;
            c.gop_size = 600;
            c.max_b_frames = 0;
            c.flags &= !(ff::AV_CODEC_FLAG_GLOBAL_HEADER as i32);
            let mut d = Dict::new();
            d.set("preset", "p4");
            d.set("tune", "ull");
            d.set("rc", "cbr");
            d.set("zerolatency", 1);
            d.set("delay", 0);
            d.set("forced-idr", 1);
            d.set("no-scenecut", 1);
            d.set("rc-lookahead", 0);
            d.set("b_ref_mode", "disabled");
            d.set("multipass", "disabled");
            d.set("profile", "high");
            let r = ff::avcodec_open2(ctx.0, codec, d.as_mut_ptr());
            if r < 0 {
                return Err(format!("avcodec_open2: {}", err_str(r)));
            }
            Ok(ctx)
        }
    }

    /// 프레임 참조 하나를 인코더에 보내고 나온 패킷 수와 키프레임 수를 더한다. `frame`이 `None`이면 비운다(flush).
    fn send(
        ctx: &CodecCtx,
        pkt: &Packet,
        frame: Option<&FrameRef>,
        pts: i64,
        stats: &mut (usize, usize, usize),
    ) -> Result<(), String> {
        // SAFETY: 열린 인코더에 프레임을 참조로 보내고 패킷을 모두 꺼낸다.
        unsafe {
            if let Some(f) = frame {
                let p = f.as_ptr() as *mut ff::AVFrame;
                (*p).pts = pts;
                let r = ff::avcodec_send_frame(ctx.0, p);
                if r < 0 {
                    return Err(format!("avcodec_send_frame: {}", err_str(r)));
                }
            } else {
                ff::avcodec_send_frame(ctx.0, ptr::null());
            }
            loop {
                let r = ff::avcodec_receive_packet(ctx.0, pkt.0);
                if r == EAGAIN || r == EOF {
                    return Ok(());
                }
                if r < 0 {
                    return Err(format!("avcodec_receive_packet: {}", err_str(r)));
                }
                stats.0 += 1;
                stats.1 += (*pkt.0).size as usize;
                if (*pkt.0).flags & ff::AV_PKT_FLAG_KEY != 0 {
                    stats.2 += 1;
                }
                ff::av_packet_unref(pkt.0);
            }
        }
    }

    /// 풀 하나에서 프레임마다 새 텍스처를 받아 그리고, 같은 frames ctx로 연 인코더 `n`개에 그 프레임의 참조를 보낸다.
    /// `threads`면 인코더마다 스레드를 두어 같은 텍스처를 동시에 인코딩한다. 인코더별 (패킷 수, 바이트, 키프레임 수).
    fn run_shared(
        flags: u32,
        n: usize,
        frames_n: usize,
        threads: bool,
    ) -> Option<Vec<(usize, usize, usize)>> {
        let (d3d, dev) = device()?;
        if !d3d.video_support {
            eprintln!("Video Processor가 없어 건너뜁니다");
            return None;
        }
        if !is_nvidia(&d3d) {
            eprintln!("NVIDIA 어댑터가 아니라 건너뜁니다");
            return None;
        }
        let (w, h) = (1280, 720);
        let mut pool = HwFramePool::with_bind_flags(&dev, w, h, flags).unwrap();
        let t = Instant::now();
        let mut encs = Vec::new();
        for _ in 0..n {
            match open_nvenc(pool.frames_ctx_ref(), w, h) {
                Ok(c) => encs.push(c),
                Err(e) if e == "h264_nvenc 없음" => {
                    eprintln!("h264_nvenc가 없어 건너뜁니다");
                    return None;
                }
                Err(e) => panic!("NVENC 열기 실패(bind flags {flags:#x}): {e}"),
            }
        }
        eprintln!("NVENC {n}개 열기 {:?} (bind flags {flags:#x})", t.elapsed());
        let mut blt = Blt::new(&d3d, w, h).expect("Video Processor");
        let mut stats = vec![(0usize, 0usize, 0usize); n];
        let produce = |blt: &mut Blt, pool: &mut HwFramePool, i: usize| {
            let f = pool.get().unwrap();
            blt.fill(&f.d3d11_texture().unwrap().0, i as u32).unwrap();
            f
        };
        if threads {
            let (txs, handles): (Vec<_>, Vec<_>) = encs
                .into_iter()
                .map(|ctx| {
                    let (tx, rx) = std::sync::mpsc::channel::<(FrameRef, i64)>();
                    let hd = std::thread::spawn(move || {
                        let pkt = Packet::new().unwrap();
                        let mut st = (0, 0, 0);
                        for (f, pts) in rx {
                            send(&ctx, &pkt, Some(&f), pts, &mut st).unwrap();
                        }
                        send(&ctx, &pkt, None, 0, &mut st).unwrap();
                        st
                    });
                    (tx, hd)
                })
                .unzip();
            for i in 0..frames_n {
                let f = produce(&mut blt, &mut pool, i);
                for tx in &txs {
                    tx.send((f.clone_ref().unwrap(), i as i64)).unwrap();
                }
            }
            drop(txs);
            for (i, hd) in handles.into_iter().enumerate() {
                stats[i] = hd.join().unwrap();
            }
        } else {
            let pkt = Packet::new().unwrap();
            for i in 0..frames_n {
                let f = produce(&mut blt, &mut pool, i);
                for (k, ctx) in encs.iter().enumerate() {
                    let r = f.clone_ref().unwrap();
                    send(ctx, &pkt, Some(&r), i as i64, &mut stats[k]).unwrap();
                }
            }
            for (k, ctx) in encs.iter().enumerate() {
                send(ctx, &pkt, None, 0, &mut stats[k]).unwrap();
            }
            drop(encs);
        }
        // 인코더·프레임이 모두 없어지면 풀 프레임도 없다.
        assert_eq!(pool.live(), 0, "인코더를 닫은 뒤에도 프레임이 남았다");
        drop(pool);
        assert_eq!(live_hw_frames(), 0);
        Some(stats)
    }

    fn check_all_encoded(stats: &[(usize, usize, usize)], frames: usize) {
        for (i, s) in stats.iter().enumerate() {
            assert_eq!(s.0, frames, "인코더 {i}: 패킷 수");
            assert!(s.1 > 1000, "인코더 {i}: 데이터가 거의 없다 ({} B)", s.1);
            assert!(s.2 >= 1, "인코더 {i}: 키프레임이 없다");
        }
    }

    /// (가) RENDER_TARGET만 건 풀 텍스처를 NVENC가 등록·인코딩한다. 한 인코더.
    #[test]
    #[cfg_attr(not(feature = "full-tests"), ignore = "NVENC 세션을 연다")]
    fn nvenc_encodes_render_target_only_pool_textures() {
        let _g = lock();
        let Some(stats) = run_shared(HW_POOL_BIND_FLAGS, 1, 30, false) else {
            return;
        };
        eprintln!("RENDER_TARGET만: 패킷 {:?}", stats);
        check_all_encoded(&stats, 30);
    }

    /// (나) 같은 풀·같은 프레임 참조를 같은 frames ctx로 연 인코더 셋에 차례로 보낸다.
    #[test]
    #[cfg_attr(not(feature = "full-tests"), ignore = "NVENC 세션을 연다")]
    fn nvenc_three_encoders_share_pool_frames_in_turn() {
        let _g = lock();
        let Some(stats) = run_shared(HW_POOL_BIND_FLAGS, 3, 30, false) else {
            return;
        };
        eprintln!("세션 셋(차례로): 패킷 {:?}", stats);
        check_all_encoded(&stats, 30);
    }

    /// (나) 같은 텍스처를 세 NVENC 세션이 스레드 셋에서 동시에 매핑·인코딩한다(계층 인코더 구조).
    #[test]
    #[cfg_attr(not(feature = "full-tests"), ignore = "NVENC 세션을 연다")]
    fn nvenc_three_encoders_share_pool_frames_concurrently() {
        let _g = lock();
        let Some(stats) = run_shared(HW_POOL_BIND_FLAGS, 3, 30, true) else {
            return;
        };
        eprintln!("세션 셋(동시): 패킷 {:?}", stats);
        check_all_encoded(&stats, 30);
    }
}
