//! Opus 인코더(ffmpeg libopus): 48kHz 스테레오 float, 20ms 프레임.

use ffmpeg_sys_next as ff;

use super::timeline::{CHANNELS, FRAME_SAMPLES, SAMPLE_RATE};
use crate::MediaError;
use crate::ff::{CodecCtx, Dict, EAGAIN, EOF, Frame, Packet, cstring, err_str};

pub(crate) struct OpusEncoder {
    ctx: CodecCtx,
    frame: Frame,
    pkt: Packet,
    /// 인코더 지연(사전 건너뛰기, pre-skip) 샘플 수. libopus `application=audio`면 312.
    pub delay: u32,
}

// SAFETY: 한 스레드에서만 쓴다.
unsafe impl Send for OpusEncoder {}

fn fail(msg: String) -> MediaError {
    MediaError::AudioEncoder(msg)
}

impl OpusEncoder {
    pub fn open(bitrate_kbps: u32) -> Result<Self, MediaError> {
        crate::ff::init();
        let name = cstring("libopus");
        // SAFETY: ffmpeg 문서의 순서대로 열고, 포인터는 RAII 래퍼가 해제한다.
        unsafe {
            let codec = ff::avcodec_find_encoder_by_name(name.as_ptr());
            if codec.is_null() {
                return Err(fail("ffmpeg에 libopus 인코더가 없습니다".into()));
            }
            let ctx = CodecCtx(ff::avcodec_alloc_context3(codec));
            if ctx.0.is_null() {
                return Err(fail("컨텍스트 할당 실패".into()));
            }
            let c = &mut *ctx.0;
            c.sample_rate = SAMPLE_RATE as i32;
            ff::av_channel_layout_default(&mut c.ch_layout, CHANNELS as i32);
            c.sample_fmt = ff::AVSampleFormat::AV_SAMPLE_FMT_FLT;
            c.bit_rate = bitrate_kbps.clamp(16, 510) as i64 * 1000;
            c.time_base = ff::AVRational {
                num: 1,
                den: SAMPLE_RATE as i32,
            };
            let mut d = Dict::new();
            d.set("application", "audio");
            d.set("frame_duration", 20);
            // 제한 VBR: 평균 비트레이트를 목표에 맞춘다. 무제한 VBR(on)은 사인파·삐 소리에서 목표를 20% 넘게 넘었다.
            d.set("vbr", "constrained");
            d.set("packet_loss", 0);
            let r = ff::avcodec_open2(ctx.0, codec, d.as_mut_ptr());
            if r < 0 {
                return Err(fail(format!("libopus 열기 실패: {}", err_str(r))));
            }
            if c.frame_size != FRAME_SAMPLES as i32 {
                return Err(fail(format!("예상과 다른 프레임 크기: {}", c.frame_size)));
            }
            let frame = Frame::new().ok_or_else(|| fail("프레임 할당 실패".into()))?;
            let f = &mut *frame.0;
            f.nb_samples = FRAME_SAMPLES as i32;
            f.format = ff::AVSampleFormat::AV_SAMPLE_FMT_FLT as i32;
            f.sample_rate = SAMPLE_RATE as i32;
            let r = ff::av_channel_layout_copy(&mut f.ch_layout, &c.ch_layout);
            if r < 0 {
                return Err(fail(format!("채널 배치 복사 실패: {}", err_str(r))));
            }
            let r = ff::av_frame_get_buffer(frame.0, 0);
            if r < 0 {
                return Err(fail(format!("프레임 버퍼 할당 실패: {}", err_str(r))));
            }
            let pkt = Packet::new().ok_or_else(|| fail("패킷 할당 실패".into()))?;
            let delay = c.initial_padding.max(0) as u32;
            Ok(OpusEncoder {
                ctx,
                frame,
                pkt,
                delay,
            })
        }
    }

    /// 960샘플(스테레오 인터리브 1920개)을 인코딩한다. 보통 패킷 하나가 나온다.
    pub fn encode(
        &mut self,
        index: u64,
        pcm: &[f32],
        out: &mut Vec<Vec<u8>>,
    ) -> Result<(), MediaError> {
        debug_assert_eq!(pcm.len(), FRAME_SAMPLES * CHANNELS);
        // SAFETY: 프레임 버퍼는 FLT 인터리브 960×2이며, 쓰기 전에 쓰기 가능하게 만든다.
        unsafe {
            let r = ff::av_frame_make_writable(self.frame.0);
            if r < 0 {
                return Err(fail(format!("프레임 쓰기 준비 실패: {}", err_str(r))));
            }
            let f = &mut *self.frame.0;
            let dst =
                std::slice::from_raw_parts_mut(f.data[0] as *mut f32, FRAME_SAMPLES * CHANNELS);
            dst.copy_from_slice(pcm);
            f.pts = index as i64;
            let r = ff::avcodec_send_frame(self.ctx.0, self.frame.0);
            if r < 0 {
                return Err(fail(format!("프레임 보내기 실패: {}", err_str(r))));
            }
            loop {
                let r = ff::avcodec_receive_packet(self.ctx.0, self.pkt.0);
                if r == EAGAIN || r == EOF {
                    break;
                }
                if r < 0 {
                    return Err(fail(format!("패킷 받기 실패: {}", err_str(r))));
                }
                out.push(self.pkt.data().to_vec());
                self.pkt.unref();
            }
        }
        Ok(())
    }
}
