//! 비디오 인코더(libavcodec): 후보 목록, 저지연 설정, 탐색, 인코딩.

use std::collections::VecDeque;
use std::ptr;
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dongsi_core::VideoCodec;
use ffmpeg_sys_next as ff;
use serde::Serialize;

use crate::MediaError;
use crate::bitstream::{Av1Fixer, H264Fixer};
use crate::clock::{QPC_PER_SEC, qpc_now_100ns};
use crate::ff::{CodecCtx, Dict, EAGAIN, EOF, FramePool, FrameRef, Packet, cstring, err_str};
use crate::trace::{self, Sink, VideoRow};
use crate::yuv::Yuv;

/// 이 PC에서 쓸 수 있는 인코더 하나.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncoderInfo {
    pub codec: VideoCodec,
    /// ffmpeg 인코더 이름(예: `h264_nvenc`)
    pub name: String,
    pub hardware: bool,
}

/// 인코더 후보. 코덱마다 앞에 있는 것이 우선이다.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Candidate {
    pub codec: VideoCodec,
    pub name: &'static str,
    pub hardware: bool,
}

const fn c(codec: VideoCodec, name: &'static str, hardware: bool) -> Candidate {
    Candidate {
        codec,
        name,
        hardware,
    }
}

/// 코덱은 H.264·AV1만 쓴다(D-075). H.264 소프트웨어 인코더는 libopenh264(BSD)만 쓴다. GPL인 libx264는
/// 넣지 않는다(D-062, ffmpeg는 LGPL 빌드).
pub(crate) const CANDIDATES: &[Candidate] = &[
    c(VideoCodec::H264, "h264_nvenc", true),
    c(VideoCodec::H264, "h264_amf", true),
    c(VideoCodec::H264, "h264_qsv", true),
    c(VideoCodec::H264, "libopenh264", false),
    c(VideoCodec::Av1, "av1_nvenc", true),
    c(VideoCodec::Av1, "av1_amf", true),
    c(VideoCodec::Av1, "av1_qsv", true),
    c(VideoCodec::Av1, "libsvtav1", false),
];

/// 인코더 크기·속도 설정.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EncoderParams {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
}

impl EncoderParams {
    fn bit_rate(&self) -> i64 {
        self.bitrate_kbps as i64 * 1000
    }

    /// VBV 버퍼 크기(비트): 2프레임, 최소 50ms.
    fn vbv_bits(&self) -> i64 {
        let secs = f64::max(2.0 / self.fps.max(1) as f64, 0.05);
        (self.bit_rate() as f64 * secs) as i64
    }
}

/// 인코딩 결과 패킷 하나.
#[derive(Debug)]
pub(crate) struct EncodedPacket {
    pub data: Vec<u8>,
    pub keyframe: bool,
    /// 입력 프레임의 캡처 시각(QPC 100ns)
    pub qpc: i64,
}

/// 짝 대기열의 입력 프레임 하나.
#[derive(Clone, Copy)]
struct Pending {
    pts: i64,
    qpc: i64,
    /// 기록용(기록이 꺼져 있으면 `None`)
    tin: Option<TraceIn>,
}

/// 입력 쪽에서만 알 수 있는 기록 값.
#[derive(Clone, Copy)]
struct TraceIn {
    arrival_qpc: i64,
    enc_in_qpc: i64,
    key_req: bool,
}

/// [`VideoEncoder::take_qpc`]가 짝을 찾은 방법.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Matching {
    Exact,
    Skip,
    Fallback,
}

impl Matching {
    fn name(self) -> &'static str {
        match self {
            Matching::Exact => "exact",
            Matching::Skip => "skip",
            Matching::Fallback => "fallback",
        }
    }
}

/// [`VideoEncoder::take_qpc`] 결과.
struct Taken {
    qpc: i64,
    matching: Matching,
    entry: Option<Pending>,
}

enum Fixer {
    H264(H264Fixer),
    Av1(Av1Fixer),
}

pub(crate) struct VideoEncoder {
    ctx: CodecCtx,
    /// 입력 프레임 버퍼 풀([`new_frame`](Self::new_frame)). 인코더를 다시 열면(크기·설정 변경, 복구) 이 풀도 같이 새로 만든다.
    /// 풀은 refcount라서 인코더가 없어진 뒤에도 계층이 쥔 프레임은 그대로 유효하다.
    pool: FramePool,
    /// 입력 프레임 픽셀 형식(`AVPixelFormat`)
    pix_fmt: ff::AVPixelFormat,
    pkt: Packet,
    pub info: EncoderInfo,
    pub params: EncoderParams,
    last_pts: i64,
    /// 출력 패킷을 입력 프레임에 짝짓는 대기열(pts, 캡처 QPC).
    pending: VecDeque<Pending>,
    /// 프레임 기록(`DONGSI_MEDIA_TRACE`). 꺼져 있으면 `None`. [`open_best`]만 켠다(탐색 시험은 안 씀).
    trace: Option<&'static Sink>,
    fixer: Fixer,
    /// 비트레이트만 바꿀 때 인코더를 다시 만들지 않아도 되는지
    dynamic_bitrate: bool,
    /// 출력이 입력보다 늦게 나올 수 있는 인코더(짧게 기다린다)
    async_output: bool,
}

// SAFETY: ffmpeg 컨텍스트는 한 스레드에서만 쓰며, 스레드 사이 이동은 안전하다.
unsafe impl Send for VideoEncoder {}

fn threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get())
}

/// 인코더별 저지연 옵션.
fn configure(cand: &Candidate, p: &EncoderParams, ctx: &mut ff::AVCodecContext, d: &mut Dict) {
    let px_rate = p.width as u64 * p.height as u64 * p.fps as u64;
    let heavy = px_rate > 1920 * 1080 * 60;
    let name = cand.name;
    if name.ends_with("_nvenc") {
        d.set("preset", if heavy { "p2" } else { "p4" });
        d.set("tune", "ull");
        d.set("rc", "cbr");
        d.set("zerolatency", 1);
        d.set("delay", 0);
        d.set("forced-idr", 1);
        d.set("no-scenecut", 1);
        d.set("rc-lookahead", 0);
        d.set("b_ref_mode", "disabled");
        d.set("multipass", "disabled");
        d.set(
            "profile",
            match cand.codec {
                VideoCodec::H264 => "high",
                VideoCodec::Av1 => "main",
            },
        );
    } else if name.ends_with("_amf") {
        d.set("usage", "ultralowlatency");
        d.set("quality", "speed");
        d.set("rc", "cbr");
        d.set("enforce_hrd", 1);
        d.set("filler_data", 0);
        d.set("header_insertion_mode", "idr");
        if cand.codec == VideoCodec::H264 {
            d.set("profile", "high");
        }
    } else if name.ends_with("_qsv") {
        d.set("preset", "veryfast");
        d.set("async_depth", 1);
        d.set("look_ahead", 0);
        d.set("low_delay_brc", 1);
        d.set("forced_idr", 1);
        d.set("repeat_pps", 1);
        if cand.codec == VideoCodec::H264 {
            d.set("profile", "high");
        }
    } else {
        match name {
            "libopenh264" => {
                d.set("rc_mode", "bitrate");
                d.set("allow_skip_frames", 0);
                ctx.thread_count = threads().min(4) as i32;
            }
            "libsvtav1" => {
                d.set("preset", if heavy { 12 } else { 10 });
                // 저지연 예측 구조(pred-struct=1)는 CBR만 된다(rc_max_rate == bit_rate → CBR).
                // 초기·최적 버퍼는 최대 버퍼보다 작아야 한다.
                let buf_ms = p.vbv_bits() * 1000 / p.bit_rate().max(1);
                let level_ms = (buf_ms * 3 / 4).max(1);
                d.set(
                    "svtav1-params",
                    format!(
                        "pred-struct=1:scd=0:enable-tf=0:fast-decode=1:buf-initial-sz={level_ms}:buf-optimal-sz={level_ms}"
                    ),
                );
            }
            _ => {}
        }
    }
}

fn pick_pix_fmt(ctx: *const ff::AVCodecContext, codec: *const ff::AVCodec) -> ff::AVPixelFormat {
    use ff::AVPixelFormat::{AV_PIX_FMT_NONE, AV_PIX_FMT_NV12, AV_PIX_FMT_YUV420P};
    let mut configs: *const std::ffi::c_void = ptr::null();
    let mut n: i32 = 0;
    // SAFETY: 결과 배열은 ffmpeg가 소유하는 정적 데이터다.
    let list: &[ff::AVPixelFormat] = unsafe {
        let r = ff::avcodec_get_supported_config(
            ctx,
            codec,
            ff::AVCodecConfig::AV_CODEC_CONFIG_PIX_FORMAT,
            0,
            &mut configs,
            &mut n,
        );
        if r < 0 || configs.is_null() || n <= 0 {
            &[]
        } else {
            std::slice::from_raw_parts(configs as *const ff::AVPixelFormat, n as usize)
        }
    };
    // 목록이 없으면(제한 없음) NV12. GPU 변환 결과가 NV12라 복사만 하면 된다.
    if list.is_empty() || list.contains(&AV_PIX_FMT_NV12) {
        AV_PIX_FMT_NV12
    } else if list.contains(&AV_PIX_FMT_YUV420P) {
        AV_PIX_FMT_YUV420P
    } else {
        AV_PIX_FMT_NONE
    }
}

impl VideoEncoder {
    /// 인코더를 연다.
    pub fn open(cand: &Candidate, p: &EncoderParams) -> Result<Self, MediaError> {
        crate::ff::init();
        let fail = |message: String| MediaError::Encoder {
            encoder: cand.name.to_string(),
            message,
        };
        let name = cstring(cand.name);
        // SAFETY: 아래 FFI 호출은 ffmpeg 문서의 사용 순서를 따른다. 포인터는 모두 RAII 래퍼가 해제한다.
        unsafe {
            let codec = ff::avcodec_find_encoder_by_name(name.as_ptr());
            if codec.is_null() {
                return Err(fail("ffmpeg에 이 인코더가 없습니다".into()));
            }
            let ctx = CodecCtx(ff::avcodec_alloc_context3(codec));
            if ctx.0.is_null() {
                return Err(fail("컨텍스트 할당 실패".into()));
            }
            let c = &mut *ctx.0;
            let pix_fmt = pick_pix_fmt(ctx.0, codec);
            if pix_fmt == ff::AVPixelFormat::AV_PIX_FMT_NONE {
                return Err(fail("NV12/YUV420P 입력을 지원하지 않습니다".into()));
            }
            c.width = p.width as i32;
            c.height = p.height as i32;
            c.pix_fmt = pix_fmt;
            c.time_base = ff::AVRational {
                num: 1,
                den: QPC_PER_SEC as i32,
            };
            c.framerate = ff::AVRational {
                num: p.fps as i32,
                den: 1,
            };
            c.bit_rate = p.bit_rate();
            c.rc_max_rate = p.bit_rate();
            c.rc_buffer_size = p.vbv_bits() as i32;
            c.rc_initial_buffer_occupancy = c.rc_buffer_size;
            c.gop_size = (p.fps * 10) as i32;
            c.max_b_frames = 0;
            // 화면 RGB(sRGB) → BT.601 limited range. VUI/색 정보로 알린다.
            c.color_range = ff::AVColorRange::AVCOL_RANGE_MPEG;
            c.colorspace = ff::AVColorSpace::AVCOL_SPC_SMPTE170M;
            c.color_primaries = ff::AVColorPrimaries::AVCOL_PRI_BT709;
            c.color_trc = ff::AVColorTransferCharacteristic::AVCOL_TRC_IEC61966_2_1;
            // 매개변수 집합을 키프레임마다 비트스트림 안에 둔다(global header 끔).
            c.flags &= !(ff::AV_CODEC_FLAG_GLOBAL_HEADER as i32);

            let mut dict = Dict::new();
            configure(cand, p, c, &mut dict);
            let r = ff::avcodec_open2(ctx.0, codec, dict.as_mut_ptr());
            if r < 0 {
                return Err(fail(format!("열기 실패: {}", err_str(r))));
            }
            let unused = dict.keys();
            if !unused.is_empty() {
                tracing::debug!(encoder = cand.name, ?unused, "인코더가 쓰지 않은 옵션");
            }

            let pool = FramePool::new(p.width, p.height, pix_fmt)
                .ok_or_else(|| fail("프레임 풀을 만들지 못했습니다".into()))?;
            let pkt = Packet::new().ok_or_else(|| fail("패킷 할당 실패".into()))?;

            let extradata = if c.extradata.is_null() || c.extradata_size <= 0 {
                &[][..]
            } else {
                std::slice::from_raw_parts(c.extradata, c.extradata_size as usize)
            };
            let fixer = match cand.codec {
                VideoCodec::H264 => {
                    let mut f = H264Fixer::default();
                    f.learn(extradata);
                    Fixer::H264(f)
                }
                VideoCodec::Av1 => {
                    let mut f = Av1Fixer::default();
                    f.learn(extradata);
                    Fixer::Av1(f)
                }
            };
            Ok(VideoEncoder {
                ctx,
                pool,
                pix_fmt,
                pkt,
                info: EncoderInfo {
                    codec: cand.codec,
                    name: cand.name.to_string(),
                    hardware: cand.hardware,
                },
                params: *p,
                last_pts: i64::MIN,
                pending: VecDeque::new(),
                trace: None,
                fixer,
                dynamic_bitrate: cand.name.ends_with("_nvenc"),
                async_output: cand.name == "libsvtav1",
            })
        }
    }

    /// 새 입력 프레임(쓰기용). 풀에서 받은 버퍼라 내용은 이전 프레임의 것일 수 있으니 전부 덮어써야 한다.
    /// 프레임마다 새로 받는다: 이전 프레임은 simulcast 계층이나 인코더가 아직 쥐고 있을 수 있어 다시 쓰지 않는다.
    pub fn new_frame(&mut self) -> Result<FrameRef, MediaError> {
        let Some(mut f) = self.pool.get() else {
            return Err(self.fail("입력 프레임 버퍼를 받지 못했습니다".into()));
        };
        // SAFETY: 방금 받은 프레임이고 컨텍스트는 열려 있다.
        unsafe {
            let c = &*self.ctx.0;
            let fr = &mut *f.as_mut_ptr();
            fr.color_range = c.color_range;
            fr.colorspace = c.colorspace;
            fr.color_primaries = c.color_primaries;
            fr.color_trc = c.color_trc;
        }
        Ok(f)
    }

    /// 이 인코더가 받을 수 있는 입력 프레임인지(크기·픽셀 형식).
    pub fn accepts(&self, frame: &FrameRef) -> bool {
        frame.width() == self.params.width as usize
            && frame.height() == self.params.height as usize
            && frame.format() == self.pix_fmt as i32
    }

    fn fail(&self, message: String) -> MediaError {
        MediaError::Encoder {
            encoder: self.info.name.clone(),
            message,
        }
    }

    /// 프레임 하나를 인코딩한다. 프레임의 pts·키프레임 표시는 여기서 정한다(프레임 참조마다 따로라서, 같은 버퍼를 가리키는
    /// 다른 참조에는 영향이 없다). 인코더가 프레임을 쥐고 있어도(qsv 등) refcount라서 버퍼는 안전하다.
    ///
    /// `arrival_qpc`는 프레임이 파이프라인에 도착한 시각(기록용, 동작에는 쓰지 않는다).
    pub fn encode(
        &mut self,
        frame: &mut FrameRef,
        qpc: i64,
        arrival_qpc: i64,
        keyframe: bool,
        out: &mut dyn FnMut(EncodedPacket),
    ) -> Result<(), MediaError> {
        if !self.accepts(frame) {
            return Err(self.fail("입력 프레임의 크기·형식이 인코더와 다릅니다".into()));
        }
        let pts = if self.last_pts == i64::MIN {
            qpc
        } else {
            qpc.max(self.last_pts + 1)
        };
        self.last_pts = pts;
        // SAFETY: 프레임은 열린 인코더와 같은 형식이다.
        unsafe {
            let f = &mut *frame.as_mut_ptr();
            f.pts = pts;
            f.duration = QPC_PER_SEC / self.params.fps.max(1) as i64;
            if keyframe {
                f.pict_type = ff::AVPictureType::AV_PICTURE_TYPE_I;
                f.flags |= ff::AV_FRAME_FLAG_KEY;
            } else {
                f.pict_type = ff::AVPictureType::AV_PICTURE_TYPE_NONE;
                f.flags &= !ff::AV_FRAME_FLAG_KEY;
            }
        }
        let tin = self.trace.map(|_| TraceIn {
            arrival_qpc,
            enc_in_qpc: qpc_now_100ns(),
            key_req: keyframe,
        });
        self.pending.push_back(Pending { pts, qpc, tin });
        // SAFETY: 열린 인코더에 프레임을 보낸다. 프레임은 참조로 넘어간다.
        let mut r = unsafe { ff::avcodec_send_frame(self.ctx.0, frame.as_ptr()) };
        if r == EAGAIN {
            self.drain(out)?;
            // SAFETY: 위와 같다.
            r = unsafe { ff::avcodec_send_frame(self.ctx.0, frame.as_ptr()) };
        }
        if r < 0 {
            self.pending.pop_back();
            return Err(self.fail(format!("프레임 보내기 실패: {}", err_str(r))));
        }
        let mut produced = self.drain(out)?;
        if produced == 0 && self.async_output {
            // 출력이 비동기인 인코더: 한 프레임 간격까지만 짧게 기다린다.
            let deadline =
                Instant::now() + Duration::from_millis(1000 / self.params.fps.max(1) as u64);
            while produced == 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_micros(500));
                produced = self.drain(out)?;
            }
        }
        // 오래된 짝(출력되지 않고 남은 것)은 정리한다.
        while self.pending.len() > 16 {
            if let Some(e) = self.pending.pop_front() {
                self.trace_lost(&e);
            }
        }
        Ok(())
    }

    /// 나올 수 있는 패킷을 모두 꺼낸다. 꺼낸 개수를 돌려준다.
    fn drain(&mut self, out: &mut dyn FnMut(EncodedPacket)) -> Result<usize, MediaError> {
        let mut n = 0;
        loop {
            // SAFETY: 열린 인코더와 할당된 패킷.
            let r = unsafe { ff::avcodec_receive_packet(self.ctx.0, self.pkt.0) };
            if r == EAGAIN || r == EOF {
                return Ok(n);
            }
            if r < 0 {
                return Err(self.fail(format!("패킷 받기 실패: {}", err_str(r))));
            }
            let out_qpc = self.trace.map(|_| qpc_now_100ns());
            // SAFETY: receive_packet이 채운 패킷.
            let (pts, dts, key_flag) = unsafe {
                let p = &*self.pkt.0;
                (p.pts, p.dts, p.flags & ff::AV_PKT_FLAG_KEY != 0)
            };
            let taken = self.take_qpc(pts);
            let qpc = taken.qpc;
            let raw = self.pkt.data();
            let (data, keyframe) = match &mut self.fixer {
                Fixer::H264(f) => f.process(raw, key_flag),
                Fixer::Av1(f) => match f.process(raw, key_flag) {
                    Ok(d) => (d, key_flag),
                    Err(e) => {
                        tracing::warn!(encoder = %self.info.name, "AV1 OBU 파싱 실패, 원본을 보냅니다: {e}");
                        (raw.to_vec(), key_flag)
                    }
                },
            };
            if let Some(sink) = self.trace {
                let tin = taken.entry.and_then(|e| e.tin);
                let nopts = |v: i64| (v != ff::AV_NOPTS_VALUE).then_some(v);
                sink.line(
                    VideoRow {
                        codec: self.info.codec,
                        encoder: &self.info.name,
                        w: self.params.width,
                        h: self.params.height,
                        capture_qpc: taken.entry.map(|e| e.qpc),
                        arrival_qpc: tin.map(|t| t.arrival_qpc),
                        enc_in_qpc: tin.map(|t| t.enc_in_qpc),
                        pkt_out_qpc: out_qpc,
                        in_pts: taken.entry.map(|e| e.pts),
                        pkt_pts: nopts(pts),
                        pkt_dts: nopts(dts),
                        matched_qpc: Some(taken.qpc),
                        matching: taken.matching.name(),
                        key_req: tin.is_some_and(|t| t.key_req),
                        key_pkt: key_flag,
                        key_out: keyframe,
                        bytes: data.len(),
                        pending: self.pending.len(),
                    }
                    .to_line(),
                );
            }
            self.pkt.unref();
            n += 1;
            if !data.is_empty() {
                out(EncodedPacket {
                    data,
                    keyframe,
                    qpc,
                });
            }
        }
    }

    /// 출력 패킷의 pts로 입력 프레임의 캡처 시각을 찾는다.
    fn take_qpc(&mut self, pts: i64) -> Taken {
        let mut skipped = false;
        if pts != ff::AV_NOPTS_VALUE {
            while let Some(&e) = self.pending.front() {
                if e.pts < pts {
                    self.pending.pop_front();
                    self.trace_lost(&e);
                    skipped = true;
                    continue;
                }
                if e.pts == pts {
                    self.pending.pop_front();
                    return Taken {
                        qpc: e.qpc,
                        matching: if skipped {
                            Matching::Skip
                        } else {
                            Matching::Exact
                        },
                        entry: Some(e),
                    };
                }
                break;
            }
        }
        let entry = self.pending.pop_front();
        Taken {
            qpc: entry.map_or(pts, |e| e.qpc),
            matching: Matching::Fallback,
            entry,
        }
    }

    /// 출력 없이 대기열에서 버려지는 입력을 기록한다.
    fn trace_lost(&self, e: &Pending) {
        let Some(sink) = self.trace else {
            return;
        };
        let tin = e.tin;
        sink.line(
            VideoRow {
                codec: self.info.codec,
                encoder: &self.info.name,
                w: self.params.width,
                h: self.params.height,
                capture_qpc: Some(e.qpc),
                arrival_qpc: tin.map(|t| t.arrival_qpc),
                enc_in_qpc: tin.map(|t| t.enc_in_qpc),
                pkt_out_qpc: None,
                in_pts: Some(e.pts),
                pkt_pts: None,
                pkt_dts: None,
                matched_qpc: None,
                matching: "lost",
                key_req: tin.is_some_and(|t| t.key_req),
                key_pkt: false,
                key_out: false,
                bytes: 0,
                pending: self.pending.len(),
            }
            .to_line(),
        );
    }

    /// 프레임 기록을 켠다(`DONGSI_MEDIA_TRACE`가 있을 때만 효과가 있다).
    fn enable_trace(&mut self) {
        self.trace = trace::video();
        if let Some(sink) = self.trace {
            sink.mark(
                "encoder",
                &format!(
                    "codec={} name={} hw={} size={}x{} fps={} kbps={}",
                    self.info.codec,
                    self.info.name,
                    self.info.hardware,
                    self.params.width,
                    self.params.height,
                    self.params.fps,
                    self.params.bitrate_kbps
                ),
            );
        }
    }

    /// 남은 패킷을 모두 꺼낸다(인코더를 끝낼 때).
    pub fn flush(&mut self, out: &mut dyn FnMut(EncodedPacket)) -> Result<(), MediaError> {
        // SAFETY: null 프레임은 flush 신호다.
        let r = unsafe { ff::avcodec_send_frame(self.ctx.0, ptr::null()) };
        if r < 0 && r != EOF {
            return Err(self.fail(format!("flush 실패: {}", err_str(r))));
        }
        self.drain(out).map(|_| ())
    }

    /// 비트레이트만 바꾼다. 인코더를 다시 만들지 않고 바꿀 수 있으면 `true`.
    pub fn set_bitrate(&mut self, kbps: u32) -> bool {
        if !self.dynamic_bitrate {
            return false;
        }
        self.params.bitrate_kbps = kbps;
        // SAFETY: nvenc는 다음 프레임을 보낼 때 이 값의 변화를 보고 재설정한다.
        unsafe {
            let c = &mut *self.ctx.0;
            c.bit_rate = self.params.bit_rate();
            c.rc_max_rate = self.params.bit_rate();
            c.rc_buffer_size = self.params.vbv_bits() as i32;
        }
        true
    }
}

/// 인코더 하나를 실제로 열고 프레임 몇 장을 인코딩해 본다.
fn probe_one(cand: &Candidate) -> Result<(), String> {
    let prev = crate::ff::set_quiet(true);
    let res = (|| {
        let params = EncoderParams {
            width: 640,
            height: 360,
            fps: 30,
            bitrate_kbps: 1000,
        };
        let mut enc = VideoEncoder::open(cand, &params).map_err(|e| e.to_string())?;
        let mut packets = 0;
        for i in 0..3i64 {
            let mut frame = enc.new_frame().map_err(|e| e.to_string())?;
            frame.view_mut().ok_or("프레임을 쓸 수 없습니다")?.fill(Yuv(
                60 + i as u8 * 40,
                128,
                128,
            ));
            enc.encode(&mut frame, i * QPC_PER_SEC / 30, 0, i == 0, &mut |_| {
                packets += 1
            })
            .map_err(|e| e.to_string())?;
        }
        enc.flush(&mut |_| packets += 1)
            .map_err(|e| e.to_string())?;
        if packets == 0 {
            return Err("출력 패킷이 없습니다".into());
        }
        Ok(())
    })();
    crate::ff::set_quiet(prev);
    res
}

fn probe_all() -> Vec<EncoderInfo> {
    crate::ff::init();
    let start = Instant::now();
    let (tx, rx) = mpsc::channel();
    for (i, cand) in CANDIDATES.iter().enumerate() {
        let tx = tx.clone();
        let cand = *cand;
        let spawned = std::thread::Builder::new()
            .name(format!("dongsi-probe-{}", cand.name))
            .spawn(move || {
                let t = Instant::now();
                let r = probe_one(&cand);
                let _ = tx.send((i, r, t.elapsed()));
            });
        if spawned.is_err() {
            tracing::warn!(encoder = cand.name, "탐색 스레드를 시작하지 못했습니다");
        }
    }
    drop(tx);
    let deadline = start + Duration::from_secs(5);
    let mut ok = vec![false; CANDIDATES.len()];
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok((i, r, took)) => match r {
                Ok(()) => {
                    tracing::debug!(encoder = CANDIDATES[i].name, ?took, "인코더 사용 가능");
                    ok[i] = true;
                }
                Err(e) => {
                    tracing::debug!(encoder = CANDIDATES[i].name, ?took, "인코더 사용 불가: {e}")
                }
            },
            Err(_) => break,
        }
    }
    let list: Vec<EncoderInfo> = CANDIDATES
        .iter()
        .zip(ok)
        .filter(|(_, ok)| *ok)
        .map(|(c, _)| EncoderInfo {
            codec: c.codec,
            name: c.name.to_string(),
            hardware: c.hardware,
        })
        .collect();
    tracing::info!(
        took_ms = start.elapsed().as_millis() as u64,
        encoders = ?list.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        "인코더 탐색 완료"
    );
    list
}

/// 이 PC에서 실제로 쓸 수 있는 인코더 목록. 코덱마다 좋은 것(하드웨어 우선)이 먼저 온다.
///
/// 처음 부를 때 후보마다 640×360@30으로 인코더를 열고 몇 프레임을 인코딩해 본다
/// (병렬, 최대 5초). 결과는 프로세스가 끝날 때까지 캐시한다.
pub fn available_encoders() -> Vec<EncoderInfo> {
    static PROBED: OnceLock<Vec<EncoderInfo>> = OnceLock::new();
    PROBED.get_or_init(probe_all).clone()
}

/// 코덱에 쓸 수 있는 후보(우선순위 순).
///
/// 환경 변수 `DONGSI_MEDIA_ENCODER=<ffmpeg 인코더 이름>`이 있고 그 코덱의 쓸 수 있는
/// 인코더면 맨 앞에 둔다(소프트웨어 대비책 측정용).
pub(crate) fn candidates_for(codec: VideoCodec) -> Vec<Candidate> {
    let available = available_encoders();
    let mut list: Vec<Candidate> = CANDIDATES
        .iter()
        .filter(|c| c.codec == codec && available.iter().any(|a| a.name == c.name))
        .copied()
        .collect();
    if let Ok(name) = std::env::var("DONGSI_MEDIA_ENCODER")
        && let Some(i) = list.iter().position(|c| c.name == name)
    {
        let c = list.remove(i);
        list.insert(0, c);
    }
    list
}

/// 코덱의 후보를 차례로 열어 본다. 모두 실패하면 마지막 오류.
pub(crate) fn open_best(codec: VideoCodec, p: &EncoderParams) -> Result<VideoEncoder, MediaError> {
    let mut last = MediaError::NoEncoder(codec);
    for cand in candidates_for(codec) {
        match VideoEncoder::open(&cand, p) {
            Ok(mut e) => {
                e.enable_trace();
                return Ok(e);
            }
            Err(e) => {
                tracing::warn!(encoder = cand.name, "인코더 열기 실패, 다음 후보로: {e}");
                last = e;
            }
        }
    }
    Err(last)
}

/// 이름이 `name`인 후보(이 PC에서 쓸 수 있는 것) 하나만 연다(simulcast 계층은 주 인코더와 같은 인코더를 쓴다).
/// 프레임 기록은 켜지 않는다(주 인코더 기록에 섞지 않는다).
pub(crate) fn open_named(
    codec: VideoCodec,
    name: &str,
    p: &EncoderParams,
) -> Result<VideoEncoder, MediaError> {
    let cand = candidates_for(codec)
        .into_iter()
        .find(|c| c.name == name)
        .ok_or(MediaError::NoEncoder(codec))?;
    VideoEncoder::open(&cand, p)
}
