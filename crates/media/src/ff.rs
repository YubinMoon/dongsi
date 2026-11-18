//! ffmpeg(libavcodec) FFI 도우미: 초기화, 로그 연결, 오류 문자열, 사전(AVDictionary).

use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;
use std::sync::Once;

use ffmpeg_sys_next as ff;

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
