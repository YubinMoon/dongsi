//! ffmpeg(libavcodec) FFI 도우미: 초기화, 로그 연결, 오류 문자열, 사전(AVDictionary).

use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Once};

use ffmpeg_sys_next as ff;

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
/// 버퍼 종류는 가리지 않는다: [`FramePool`]이 만든 시스템 메모리 프레임도, (다음 단계에서) `av_hwframe_get_buffer`가
/// 만든 D3D11 하드웨어 프레임도 [`from_raw`](Self::from_raw)로 감싸 같은 방식으로 넘긴다.
pub(crate) struct FrameRef(*mut ff::AVFrame);

// SAFETY: 프레임 객체 하나는 한 번에 한 스레드만 쓴다(소유권을 옮길 뿐). 가리키는 버퍼는 refcount(원자적)다.
unsafe impl Send for FrameRef {}

impl FrameRef {
    /// 이미 만든 AVFrame의 소유권을 받는다. null이면 `None`.
    ///
    /// # Safety
    /// `frame`은 `av_frame_alloc`(또는 `av_frame_clone`)이 만든 프레임이어야 하고, 이후 호출자가 해제하지 않는다.
    #[allow(dead_code)] // 하드웨어 프레임(D3D11) 단계에서 쓴다
    pub unsafe fn from_raw(frame: *mut ff::AVFrame) -> Option<Self> {
        (!frame.is_null()).then_some(FrameRef(frame))
    }

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
