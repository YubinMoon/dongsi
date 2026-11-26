//! 비디오 인코더(libavcodec): 후보 목록, 저지연 설정, 탐색, 인코딩.

use std::collections::VecDeque;
use std::ptr;
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ffmpeg_sys_next as ff;
use serde::Serialize;

use crate::MediaError;
use crate::bitstream::H264Fixer;
use crate::clock::{QPC_PER_SEC, qpc_now_100ns};
use crate::ff::{
    CodecCtx, Dict, EAGAIN, EOF, FramePool, FrameRef, HwFramesRef, Packet, cstring, err_str,
};
use crate::trace::{self, Sink, VideoRow};
use crate::yuv::Yuv;

/// 이 PC에서 쓸 수 있는 인코더 하나.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncoderInfo {
    /// ffmpeg 인코더 이름(예: `h264_nvenc`)
    pub name: String,
}

/// 인코더 후보(하드웨어 H.264). 앞에 있는 것이 우선이다.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Candidate {
    pub name: &'static str,
}

impl Candidate {
    /// D3D11 NV12 텍스처를 바로 받는 인코더인지(NVENC만. AMF·QSV의 zero-copy는 나중, D-099).
    pub fn zero_copy(&self) -> bool {
        self.name.ends_with("_nvenc")
    }
}

/// 비디오는 하드웨어 H.264 인코더만 쓴다(D-103): NVIDIA → AMD → Intel 순서. 소프트웨어 인코더와 AV1은 없고, 이 PC에서
/// 하나도 열리지 않으면 방송할 수 없다([`MediaError::NoEncoder`]).
pub(crate) const CANDIDATES: &[Candidate] = &[
    Candidate { name: "h264_nvenc" },
    Candidate { name: "h264_amf" },
    Candidate { name: "h264_qsv" },
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

pub(crate) struct VideoEncoder {
    ctx: CodecCtx,
    /// 입력 프레임 버퍼 풀([`new_frame`](Self::new_frame)). 인코더를 다시 열면(크기·설정 변경, 복구) 이 풀도 같이 새로 만든다.
    /// 풀은 refcount라서 인코더가 없어진 뒤에도 다른 곳이 쥔 프레임은 그대로 유효하다. hw 입력 인코더는 풀이 없다(프레임은
    /// [`HwFramePool`](crate::ff::HwFramePool)에서 받는다).
    pool: Option<FramePool>,
    /// 입력 프레임 픽셀 형식(`AVPixelFormat`). hw 입력이면 `AV_PIX_FMT_D3D11`.
    pix_fmt: ff::AVPixelFormat,
    /// hw 입력(D3D11 텍스처)으로 열었으면 그때 frames ctx를 만든 풀의 세대. **인코더 하나는 평생 frames ctx 하나**라서
    /// 다른 세대의 프레임은 받지 않는다([`accepts`](Self::accepts)).
    hw_generation: Option<u64>,
    /// 후보가 hw 입력을 받을 수 있는 인코더(NVENC)인지. CPU 입력으로 열었어도 참이다.
    zero_copy_capable: bool,
    pkt: Packet,
    pub info: EncoderInfo,
    pub params: EncoderParams,
    last_pts: i64,
    /// 출력 패킷을 입력 프레임에 짝짓는 대기열(pts, 캡처 QPC).
    pending: VecDeque<Pending>,
    /// 프레임 기록(`DONGSI_MEDIA_TRACE`). 꺼져 있으면 `None`. [`open_best`]만 켠다(탐색 시험은 안 씀).
    trace: Option<&'static Sink>,
    fixer: H264Fixer,
    /// 비트레이트만 바꿀 때 인코더를 다시 만들지 않아도 되는지
    dynamic_bitrate: bool,
}

// SAFETY: ffmpeg 컨텍스트는 한 스레드에서만 쓰며, 스레드 사이 이동은 안전하다.
unsafe impl Send for VideoEncoder {}

/// 인코더별 저지연 옵션. 프로필은 모두 High다.
fn configure(cand: &Candidate, p: &EncoderParams, d: &mut Dict) {
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
        d.set("profile", "high");
    } else if name.ends_with("_amf") {
        d.set("usage", "ultralowlatency");
        d.set("quality", "speed");
        d.set("rc", "cbr");
        d.set("enforce_hrd", 1);
        d.set("filler_data", 0);
        d.set("header_insertion_mode", "idr");
        d.set("profile", "high");
    } else if name.ends_with("_qsv") {
        d.set("preset", "veryfast");
        d.set("async_depth", 1);
        d.set("look_ahead", 0);
        d.set("low_delay_brc", 1);
        d.set("forced_idr", 1);
        d.set("repeat_pps", 1);
        d.set("profile", "high");
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
    /// 인코더를 연다. `hw`가 있으면 D3D11 NV12 텍스처 입력(`AV_PIX_FMT_D3D11`, 그 frames ctx)으로 열고(NVENC만),
    /// 없으면 시스템 메모리 NV12·YUV420P 입력으로 연다.
    pub fn open(
        cand: &Candidate,
        p: &EncoderParams,
        hw: Option<&HwFramesRef>,
    ) -> Result<Self, MediaError> {
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
            let pix_fmt = if hw.is_some() {
                if !cand.zero_copy() {
                    return Err(fail("이 인코더는 D3D11 텍스처 입력을 받지 않습니다".into()));
                }
                ff::AVPixelFormat::AV_PIX_FMT_D3D11
            } else {
                pick_pix_fmt(ctx.0, codec)
            };
            if pix_fmt == ff::AVPixelFormat::AV_PIX_FMT_NONE {
                return Err(fail("NV12/YUV420P 입력을 지원하지 않습니다".into()));
            }
            if let Some(h) = hw {
                // 코덱 컨텍스트가 참조 하나를 소유한다(avcodec_free_context가 놓는다).
                c.hw_frames_ctx = h.new_ref();
                if c.hw_frames_ctx.is_null() {
                    return Err(fail("hw_frames_ctx 참조를 만들지 못했습니다".into()));
                }
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
            configure(cand, p, &mut dict);
            let r = ff::avcodec_open2(ctx.0, codec, dict.as_mut_ptr());
            if r < 0 {
                return Err(fail(format!("열기 실패: {}", err_str(r))));
            }
            let unused = dict.keys();
            if !unused.is_empty() {
                tracing::debug!(encoder = cand.name, ?unused, "인코더가 쓰지 않은 옵션");
            }
            // 다시 열 때(크기·비트레이트 변경, 자동 해상도) 실제로 건 율 제어 값을 확인하는 용도.
            tracing::debug!(
                encoder = cand.name,
                width = c.width,
                height = c.height,
                fps = p.fps,
                bit_rate = c.bit_rate,
                rc_max_rate = c.rc_max_rate,
                rc_buffer_size = c.rc_buffer_size,
                hw_input = hw.is_some(),
                "인코더 열림"
            );

            let pool = if hw.is_some() {
                None
            } else {
                Some(
                    FramePool::new(p.width, p.height, pix_fmt)
                        .ok_or_else(|| fail("프레임 풀을 만들지 못했습니다".into()))?,
                )
            };
            let pkt = Packet::new().ok_or_else(|| fail("패킷 할당 실패".into()))?;

            let extradata = if c.extradata.is_null() || c.extradata_size <= 0 {
                &[][..]
            } else {
                std::slice::from_raw_parts(c.extradata, c.extradata_size as usize)
            };
            let mut fixer = H264Fixer::default();
            fixer.learn(extradata);
            Ok(VideoEncoder {
                ctx,
                pool,
                pix_fmt,
                hw_generation: hw.map(HwFramesRef::generation),
                zero_copy_capable: cand.zero_copy(),
                pkt,
                info: EncoderInfo {
                    name: cand.name.to_string(),
                },
                params: *p,
                last_pts: i64::MIN,
                pending: VecDeque::new(),
                trace: None,
                fixer,
                dynamic_bitrate: cand.name.ends_with("_nvenc"),
            })
        }
    }

    /// 새 입력 프레임(쓰기용). 풀에서 받은 버퍼라 내용은 이전 프레임의 것일 수 있으니 전부 덮어써야 한다.
    /// 프레임마다 새로 받는다: 이전 프레임은 인코더가 아직 쥐고 있을 수 있어 다시 쓰지 않는다.
    /// hw 입력 인코더는 쓰지 않는다(프레임은 [`HwFramePool`](crate::ff::HwFramePool)에서 받는다).
    pub fn new_frame(&mut self) -> Result<FrameRef, MediaError> {
        let Some(f) = self.pool.as_mut().and_then(FramePool::get) else {
            return Err(self.fail("입력 프레임 버퍼를 받지 못했습니다".into()));
        };
        Ok(f)
    }

    /// D3D11 텍스처(hw 프레임)를 입력으로 받는 인코더인지.
    pub fn is_hw_input(&self) -> bool {
        self.hw_generation.is_some()
    }

    /// hw 입력이면 연 frames ctx의 풀 세대.
    pub fn hw_generation(&self) -> Option<u64> {
        self.hw_generation
    }

    /// hw 입력(zero-copy)을 받을 수 있는 인코더(NVENC)인지. CPU 입력으로 열었어도 참이다.
    pub fn zero_copy_capable(&self) -> bool {
        self.zero_copy_capable
    }

    /// 이 인코더가 받을 수 있는 입력 프레임인지. 크기·픽셀 형식이 같아야 하고, hw 입력이면 인코더를 연 풀과 **같은 세대**의
    /// 프레임이어야 한다(nvenc의 등록 캐시가 텍스처 포인터 비교라서, 다른 frames ctx의 프레임을 보내면 안 된다).
    pub fn accepts(&self, frame: &FrameRef) -> bool {
        frame.width() == self.params.width as usize
            && frame.height() == self.params.height as usize
            && frame.format() == self.pix_fmt as i32
            && frame.pool_generation() == self.hw_generation
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
            let c = &*self.ctx.0;
            let f = &mut *frame.as_mut_ptr();
            f.color_range = c.color_range;
            f.colorspace = c.colorspace;
            f.color_primaries = c.color_primaries;
            f.color_trc = c.color_trc;
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
        self.drain(out)?;
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
            let (data, keyframe) = self.fixer.process(raw, key_flag);
            if let Some(sink) = self.trace {
                let tin = taken.entry.and_then(|e| e.tin);
                let nopts = |v: i64| (v != ff::AV_NOPTS_VALUE).then_some(v);
                sink.line(
                    VideoRow {
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
                    "name={} size={}x{} fps={} kbps={}",
                    self.info.name,
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
        let mut enc = VideoEncoder::open(cand, &params, None).map_err(|e| e.to_string())?;
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
            name: c.name.to_string(),
        })
        .collect();
    tracing::info!(
        took_ms = start.elapsed().as_millis() as u64,
        encoders = ?list.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        "인코더 탐색 완료"
    );
    list
}

/// 이 PC에서 실제로 쓸 수 있는 하드웨어 H.264 인코더 목록(후보 순서: nvenc → amf → qsv). 비어 있으면 방송할 수 없다.
///
/// 처음 부를 때 후보마다 640×360@30으로 인코더를 열고 몇 프레임을 인코딩해 본다
/// (병렬, 최대 5초). 결과는 프로세스가 끝날 때까지 캐시한다.
pub fn available_encoders() -> Vec<EncoderInfo> {
    static PROBED: OnceLock<Vec<EncoderInfo>> = OnceLock::new();
    PROBED.get_or_init(probe_all).clone()
}

/// 쓸 수 있는 후보(우선순위 순).
///
/// 환경 변수 `DONGSI_MEDIA_ENCODER=<ffmpeg 인코더 이름>`이 있고 쓸 수 있는 인코더면 맨 앞에 둔다(측정·시험용. 예: NVENC와
/// QSV가 둘 다 있는 PC에서 QSV로 보내 보기).
pub(crate) fn candidates() -> Vec<Candidate> {
    let available = available_encoders();
    let mut list: Vec<Candidate> = CANDIDATES
        .iter()
        .filter(|c| available.iter().any(|a| a.name == c.name))
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

/// 방송에 쓸 인코더: 이 PC에서 열리는 첫 후보([`DONGSI_MEDIA_ENCODER`](candidates) 반영). 없으면 하드웨어 H.264 인코더가 없어
/// 방송할 수 없다(D-103). 처음 부르면 [`available_encoders`] 탐색(최대 몇 초)을 기다린다.
pub fn preferred_encoder() -> Option<EncoderInfo> {
    candidates().first().map(|c| EncoderInfo {
        name: c.name.to_string(),
    })
}

/// 후보 중에 D3D11 텍스처 입력(zero-copy)을 받는 인코더(NVENC)가 있는지.
pub(crate) fn has_zero_copy_candidate() -> bool {
    candidates().iter().any(Candidate::zero_copy)
}

/// [`open_best`]의 결과.
pub(crate) struct Opened {
    pub enc: VideoEncoder,
    /// hw 입력으로 열려다 실패해서 CPU 입력으로 열었다면 그 이유(예: 하이브리드 GPU에서 NVENC 세션이 우리 장치에 안 열림).
    pub hw_error: Option<String>,
}

/// 후보를 차례로 열어 본다. 후보가 없으면 [`MediaError::NoEncoder`], 모두 실패하면 마지막 오류.
///
/// `hw`가 있으면 hw 입력을 받는 후보(NVENC)는 먼저 그것으로 열어 본다. 실패하면(이유는 `Opened::hw_error`) 같은 후보를
/// CPU 입력으로 다시 연다. hw 입력을 받지 않는 후보(AMF·QSV)는 `hw`와 상관없이 CPU 입력으로 연다.
///
/// **시험 전용 훅**(`full-tests` 기능으로 빌드할 때만 있다): 환경 변수 `DONGSI_MEDIA_OPEN_FAIL_SIZE`가 `<가로>x<세로>`(예:
/// `1280x720`)이면 그 크기로는 모든 후보 열기가 실패한다. 하드웨어 인코더가 받지 않는 출력 크기(AMF·QSV의 최소 크기 등,
/// 미검증)를 흉내 내 비디오 워커의 다시 열기 실패 안전장치(D-109, `tests/zero_copy.rs`)를 돌리려고 둔 것이고, 제품 빌드에는
/// 들어가지 않는다.
pub(crate) fn open_best(p: &EncoderParams, hw: Option<&HwFramesRef>) -> Result<Opened, MediaError> {
    #[cfg(feature = "full-tests")]
    if std::env::var("DONGSI_MEDIA_OPEN_FAIL_SIZE")
        .is_ok_and(|v| v == format!("{}x{}", p.width, p.height))
    {
        return Err(MediaError::Encoder {
            encoder: "(시험)".into(),
            message: format!(
                "시험용으로 {}×{} 열기를 실패시켰습니다(DONGSI_MEDIA_OPEN_FAIL_SIZE)",
                p.width, p.height
            ),
        });
    }
    let mut last = MediaError::NoEncoder;
    let mut hw_error = None;
    for cand in candidates() {
        if let Some(h) = hw.filter(|_| cand.zero_copy()) {
            match open_hw(&cand, p, h) {
                Ok(mut e) => {
                    e.enable_trace();
                    return Ok(Opened { enc: e, hw_error });
                }
                Err(e) => {
                    tracing::debug!(encoder = cand.name, "D3D11 입력으로 열지 못했습니다: {e}");
                    hw_error.get_or_insert_with(|| e.to_string());
                }
            }
        }
        match VideoEncoder::open(&cand, p, None) {
            Ok(mut e) => {
                e.enable_trace();
                return Ok(Opened { enc: e, hw_error });
            }
            Err(e) => {
                tracing::warn!(encoder = cand.name, "인코더 열기 실패, 다음 후보로: {e}");
                last = e;
            }
        }
    }
    Err(last)
}

/// D3D11 텍스처(hw) 입력으로 인코더를 연다.
///
/// **시험 전용 훅**(`full-tests` 기능으로 빌드할 때만 있다): 환경 변수 `DONGSI_MEDIA_ZEROCOPY_FAIL`이 `1`이면 hw 입력
/// 열기가 실패한다. NVENC가 있는 PC에서도 zero-copy에서 내려가는 길(`tests/zero_copy.rs`)을 돌리려고 둔 것이고, 제품
/// 빌드에는 들어가지 않는다.
fn open_hw(
    cand: &Candidate,
    p: &EncoderParams,
    hw: &HwFramesRef,
) -> Result<VideoEncoder, MediaError> {
    #[cfg(feature = "full-tests")]
    if std::env::var("DONGSI_MEDIA_ZEROCOPY_FAIL").is_ok_and(|v| v == "1") {
        return Err(MediaError::Encoder {
            encoder: cand.name.to_string(),
            message: "시험용으로 hw 입력 열기를 실패시켰습니다(DONGSI_MEDIA_ZEROCOPY_FAIL=1)"
                .into(),
        });
    }
    VideoEncoder::open(cand, p, Some(hw))
}
