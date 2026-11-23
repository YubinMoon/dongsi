//! 캡처·인코딩 검증용 예제.
//!
//! ```text
//! cargo run -p dongsi-media --release --example record -- [옵션]
//!   --list                         캡처 대상과 인코더 목록만 출력
//!   --source <primary|test|monitor:N|window:N>   (기본 primary)
//!   --codec <h264|av1>                            (기본 h264)
//!   --size <WxH>                   출력 상한(0x0 = 원본, 기본 1920x1080)
//!   --fps <N>                      (기본 60, 0 = 원본: 대상 모니터의 주사율)
//!   --bitrate <kbps>               (기본: H.264 7000, AV1 4900)
//!   --secs <N>                     녹화 시간(기본 5)
//!   --out <경로 접두어>            (기본 out → out.h264 / out.ivf)
//!   --update-after <초> <WxH>      실행 중 해상도 변경
//!   --bitrate-after <초> <kbps>    실행 중 비트레이트만 변경
//!   --codec-switch <초> <코덱>     실행 중 코덱 변경(두 번째 파일 <out>-2.<ext>)
//!   --keyframe-every <초>          주기적으로 키프레임 요청
//!   --audio <none|system|discord|tone|pid:N>      오디오 파이프라인도 실행
//!   --decode                       받은 비디오·오디오를 바로 디코딩해서 검사(A/V 정렬 포함)
//!   --bench-convert                변환(BGRA→NV12) 시간 측정
//!   --probe-only                   인코더 탐색 시간만 측정
//!   --simulcast <나눗수,...>       시험용 simulcast 계층(같은 해상도, 비트레이트 1/N)을 같이 인코딩(예: 2,4).
//!                                  `DONGSI_SIMULCAST_TEST`와 같다. 계층 출력은 저장하지 않고 fps만 잰다
//! ```

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dongsi_core::{VideoCodec, VideoSettings};
use dongsi_media::clock::{QPC_PER_SEC, qpc_now_100ns};
use dongsi_media::{
    AUDIO_PACKET_SAMPLES, AudioPipeline, AudioSource, EncodedAudio, EncodedVideo, SourceId,
    SourceKind, VideoPipeline, available_encoders, bitstream, find_discord_root_pids, list_sources,
};
use ffmpeg_sys_next as ff;

struct Args {
    list: bool,
    source: String,
    codec: VideoCodec,
    size: (u32, u32),
    fps: u32,
    bitrate: Option<u32>,
    secs: f64,
    out: String,
    update_after: Option<(f64, (u32, u32))>,
    bitrate_after: Option<(f64, u32)>,
    codec_switch: Option<(f64, VideoCodec)>,
    keyframe_every: Option<f64>,
    audio: Option<String>,
    decode: bool,
    bench_convert: bool,
    probe_only: bool,
    simulcast: Option<String>,
}

fn parse_size(s: &str) -> (u32, u32) {
    let (w, h) = s.split_once(['x', 'X']).expect("크기는 WxH");
    (w.parse().unwrap(), h.parse().unwrap())
}

fn parse_args() -> Args {
    let mut a = Args {
        list: false,
        source: "primary".into(),
        codec: VideoCodec::H264,
        size: (1920, 1080),
        fps: 60,
        bitrate: None,
        secs: 5.0,
        out: "out".into(),
        update_after: None,
        bitrate_after: None,
        codec_switch: None,
        keyframe_every: None,
        audio: None,
        decode: false,
        bench_convert: false,
        probe_only: false,
        simulcast: None,
    };
    let v: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    let next = |i: &mut usize| {
        *i += 1;
        v[*i].clone()
    };
    while i < v.len() {
        match v[i].as_str() {
            "--list" => a.list = true,
            "--source" => a.source = next(&mut i),
            "--codec" => a.codec = next(&mut i).parse().unwrap(),
            "--size" => a.size = parse_size(&next(&mut i)),
            "--fps" => a.fps = next(&mut i).parse().unwrap(),
            "--bitrate" => a.bitrate = Some(next(&mut i).parse().unwrap()),
            "--secs" => a.secs = next(&mut i).parse().unwrap(),
            "--out" => a.out = next(&mut i),
            "--update-after" => {
                let t = next(&mut i).parse().unwrap();
                a.update_after = Some((t, parse_size(&next(&mut i))));
            }
            "--bitrate-after" => {
                let t = next(&mut i).parse().unwrap();
                a.bitrate_after = Some((t, next(&mut i).parse().unwrap()));
            }
            "--codec-switch" => {
                let t = next(&mut i).parse().unwrap();
                a.codec_switch = Some((t, next(&mut i).parse().unwrap()));
            }
            "--keyframe-every" => a.keyframe_every = Some(next(&mut i).parse().unwrap()),
            "--audio" => a.audio = Some(next(&mut i)),
            "--decode" => a.decode = true,
            "--bench-convert" => a.bench_convert = true,
            "--probe-only" => a.probe_only = true,
            "--simulcast" => a.simulcast = Some(next(&mut i)),
            other => panic!("알 수 없는 옵션: {other}"),
        }
        i += 1;
    }
    a
}

/// 1080p60 기준 기본 비트레이트(kbps). AV1은 같은 화질에 H.264의 70%쯤을 쓴다.
fn default_kbps(codec: VideoCodec) -> u32 {
    match codec {
        VideoCodec::H264 => VideoSettings::DEFAULT.bitrate_kbps,
        VideoCodec::Av1 => VideoSettings::DEFAULT.bitrate_kbps * 7 / 10,
    }
}

/// 코덱별 출력 파일: H.264는 Annex B 그대로, AV1은 IVF.
enum Writer {
    AnnexB(BufWriter<File>),
    Ivf {
        file: BufWriter<File>,
        frames: u32,
        start_qpc: i64,
    },
}

impl Writer {
    fn create(prefix: &str, codec: VideoCodec, w: u32, h: u32) -> (Self, String) {
        let path = format!("{prefix}.{}", ext(codec));
        let mut file = BufWriter::new(File::create(&path).unwrap());
        let writer = match codec {
            VideoCodec::H264 => Writer::AnnexB(file),
            VideoCodec::Av1 => {
                let fourcc = b"AV01";
                let mut hdr = Vec::with_capacity(32);
                hdr.extend_from_slice(b"DKIF");
                hdr.extend_from_slice(&0u16.to_le_bytes());
                hdr.extend_from_slice(&32u16.to_le_bytes());
                hdr.extend_from_slice(fourcc);
                hdr.extend_from_slice(&(w as u16).to_le_bytes());
                hdr.extend_from_slice(&(h as u16).to_le_bytes());
                hdr.extend_from_slice(&1000u32.to_le_bytes()); // 시간 단위 분모(ms)
                hdr.extend_from_slice(&1u32.to_le_bytes()); // 분자
                hdr.extend_from_slice(&0u32.to_le_bytes()); // 프레임 수(끝에 채움)
                hdr.extend_from_slice(&0u32.to_le_bytes());
                file.write_all(&hdr).unwrap();
                Writer::Ivf {
                    file,
                    frames: 0,
                    start_qpc: i64::MIN,
                }
            }
        };
        (writer, path)
    }

    fn write(&mut self, f: &EncodedVideo) {
        match self {
            Writer::AnnexB(file) => file.write_all(&f.data).unwrap(),
            Writer::Ivf {
                file,
                frames,
                start_qpc,
            } => {
                if *start_qpc == i64::MIN {
                    *start_qpc = f.capture_qpc_100ns;
                }
                let ts_ms = ((f.capture_qpc_100ns - *start_qpc) / 10_000) as u64;
                file.write_all(&(f.data.len() as u32).to_le_bytes())
                    .unwrap();
                file.write_all(&ts_ms.to_le_bytes()).unwrap();
                file.write_all(&f.data).unwrap();
                *frames += 1;
            }
        }
    }

    fn finish(self) {
        match self {
            Writer::AnnexB(mut f) => f.flush().unwrap(),
            Writer::Ivf { file, frames, .. } => {
                let mut f = file.into_inner().unwrap();
                f.seek(SeekFrom::Start(24)).unwrap();
                f.write_all(&frames.to_le_bytes()).unwrap();
            }
        }
    }
}

/// 검사용 디코더(ffmpeg).
struct Decoder {
    ctx: *mut ff::AVCodecContext,
    pkt: *mut ff::AVPacket,
    frame: *mut ff::AVFrame,
}

impl Decoder {
    fn new(names: &[&str]) -> Option<Self> {
        unsafe {
            for name in names {
                let n = std::ffi::CString::new(*name).unwrap();
                let codec = ff::avcodec_find_decoder_by_name(n.as_ptr());
                if codec.is_null() {
                    continue;
                }
                let ctx = ff::avcodec_alloc_context3(codec);
                if ff::avcodec_open2(ctx, codec, std::ptr::null_mut()) < 0 {
                    ff::avcodec_free_context(&mut { ctx });
                    continue;
                }
                return Some(Decoder {
                    ctx,
                    pkt: ff::av_packet_alloc(),
                    frame: ff::av_frame_alloc(),
                });
            }
            None
        }
    }

    fn for_video(codec: VideoCodec) -> Option<Self> {
        Self::new(match codec {
            VideoCodec::H264 => &["h264"],
            VideoCodec::Av1 => &["libdav1d", "libaom-av1", "av1"],
        })
    }

    /// 패킷 하나를 디코딩해 나온 프레임마다 `f`를 부른다. 디코딩 오류 수를 돌려준다.
    fn decode(&mut self, data: &[u8], mut f: impl FnMut(&ff::AVFrame)) -> u32 {
        unsafe {
            ff::av_new_packet(self.pkt, data.len() as i32);
            std::ptr::copy_nonoverlapping(data.as_ptr(), (*self.pkt).data, data.len());
            let r = ff::avcodec_send_packet(self.ctx, self.pkt);
            ff::av_packet_unref(self.pkt);
            if r < 0 {
                return 1;
            }
            while ff::avcodec_receive_frame(self.ctx, self.frame) >= 0 {
                f(&*self.frame);
                ff::av_frame_unref(self.frame);
            }
            0
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            ff::av_frame_free(&mut self.frame);
            ff::av_packet_free(&mut self.pkt);
            ff::avcodec_free_context(&mut self.ctx);
        }
    }
}

fn mean_luma(f: &ff::AVFrame) -> f64 {
    let (w, h) = (f.width as usize, f.height as usize);
    let mut sum = 0u64;
    for y in (0..h).step_by(4) {
        let row =
            unsafe { std::slice::from_raw_parts(f.data[0].add(y * f.linesize[0] as usize), w) };
        sum += row.iter().step_by(4).map(|&v| v as u64).sum::<u64>();
    }
    sum as f64 / ((h.div_ceil(4)) * (w.div_ceil(4))) as f64
}

enum Event {
    Video(EncodedVideo, i64),
    Audio(EncodedAudio, i64),
}

#[derive(Default)]
struct Segment {
    codec: Option<VideoCodec>,
    frames: u64,
    keyframes: Vec<f64>,
    sizes: BTreeMap<(u32, u32), u64>,
    bytes: u64,
    latencies: Vec<f32>,
    decode_errors: u32,
    decoded: u64,
}

fn percentile(v: &mut [f32], p: f64) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,ffmpeg=warn".into()),
        )
        .init();
    // 창 크기 등을 물리 픽셀로 받기 위해 DPI 인식을 켠다(앱에서는 Tauri가 켠다).
    unsafe {
        let _ = windows_dpi();
    }
    let args = parse_args();
    if let Some(list) = &args.simulcast {
        // SAFETY: 다른 스레드를 시작하기 전이다.
        unsafe { std::env::set_var(dongsi_media::SIMULCAST_TEST_ENV, list) };
    }

    if args.bench_convert {
        bench_convert();
        return;
    }
    if args.probe_only {
        let t = Instant::now();
        let list = available_encoders();
        println!("인코더 탐색 {} ms", t.elapsed().as_millis());
        for e in list {
            println!("  {:?} {} (hw={})", e.codec, e.name, e.hardware);
        }
        return;
    }

    let sources = list_sources().unwrap();
    if args.list {
        println!("캡처 대상:");
        for s in &sources {
            println!(
                "  {:<22} {:?} {}x{} primary={} {:?} {:?}  {}",
                s.id.to_string(),
                s.kind,
                s.width,
                s.height,
                s.primary,
                s.process_id,
                s.process_name,
                s.name
            );
        }
        let t = Instant::now();
        let enc = available_encoders();
        println!("인코더({} ms):", t.elapsed().as_millis());
        for e in &enc {
            println!("  {:?} {} hw={}", e.codec, e.name, e.hardware);
        }
        println!("Discord 루트 PID: {:?}", find_discord_root_pids());
        return;
    }

    let source = match args.source.as_str() {
        "primary" => sources
            .iter()
            .find(|s| s.kind == SourceKind::Monitor && s.primary)
            .unwrap()
            .id
            .clone(),
        other => other.parse::<SourceId>().unwrap(),
    };
    let settings = VideoSettings {
        codec: args.codec,
        width: args.size.0,
        height: args.size.1,
        fps: args.fps,
        bitrate_kbps: args.bitrate.unwrap_or(default_kbps(args.codec)),
    };

    let (tx, rx) = mpsc::channel::<Event>();
    let vtx = tx.clone();
    let t_start = Instant::now();
    let video = VideoPipeline::start(
        source.clone(),
        settings,
        Box::new(move |f| {
            let _ = vtx.send(Event::Video(f, qpc_now_100ns()));
        }),
    )
    .expect("비디오 파이프라인 시작");
    println!(
        "비디오 시작 {} ms: {source} {settings:?} → 실제 fps {}",
        t_start.elapsed().as_millis(),
        video.fps()
    );

    let audio = args.audio.as_deref().filter(|a| *a != "none").map(|a| {
        let src = match a {
            "system" => AudioSource::System,
            "discord" => AudioSource::SystemExceptDiscord,
            "tone" => AudioSource::TestTone,
            p if p.starts_with("pid:") => AudioSource::ProcessTree(p[4..].parse().unwrap()),
            other => panic!("알 수 없는 오디오: {other}"),
        };
        let atx = tx.clone();
        AudioPipeline::start(
            src,
            128,
            Box::new(move |p| {
                let _ = atx.send(Event::Audio(p, qpc_now_100ns()));
            }),
        )
        .expect("오디오 파이프라인 시작")
    });
    drop(tx);

    let start = Instant::now();
    let mut segments = vec![Segment::default()];
    let (mut writer, mut path) = Writer::create(&args.out, args.codec, 1920, 1080);
    let mut paths = vec![path.clone()];
    let mut decoder = args
        .decode
        .then(|| Decoder::for_video(args.codec))
        .flatten();
    let mut adecoder = args
        .decode
        .then(|| Decoder::new(&["opus", "libopus"]))
        .flatten();
    let mut white_qpcs: Vec<i64> = Vec::new();
    let mut beep_qpcs: Vec<i64> = Vec::new();
    let mut white_emit_ms: Vec<f32> = Vec::new();
    let mut beep_emit_ms: Vec<f32> = Vec::new();
    let mut silent_run: u64 = 100_000;
    let mut audio_packets = 0u64;
    let mut audio_bytes = 0u64;
    let mut last_sample_index: Option<u64> = None;
    let mut audio_gaps = 0u64;
    let mut audio_arrivals: Vec<f32> = Vec::new();
    let mut last_stats = Instant::now();
    let mut did_update = false;
    let mut did_bitrate = false;
    let mut did_switch = false;
    let mut next_key = args.keyframe_every;
    let mut key_requests: Vec<(f64, Instant)> = Vec::new();
    let mut key_delays: Vec<f32> = Vec::new();
    let mut prev_white_luma = false;

    while start.elapsed().as_secs_f64() < args.secs {
        let el = start.elapsed().as_secs_f64();
        if let Some((t, size)) = args.update_after
            && !did_update
            && el >= t
        {
            did_update = true;
            let mut s = video.settings();
            s.width = size.0;
            s.height = size.1;
            let t0 = Instant::now();
            let r = video.update(s);
            println!(
                "[{el:.2}s] 해상도 변경 {size:?}: {r:?} ({} ms)",
                t0.elapsed().as_millis()
            );
            key_requests.push((el, Instant::now()));
        }
        if let Some((t, kbps)) = args.bitrate_after
            && !did_bitrate
            && el >= t
        {
            did_bitrate = true;
            let mut s = video.settings();
            s.bitrate_kbps = kbps;
            let r = video.update(s);
            println!("[{el:.2}s] 비트레이트 변경 {kbps} kbps: {r:?}");
        }
        if let Some((t, codec)) = args.codec_switch
            && !did_switch
            && el >= t
        {
            did_switch = true;
            let mut s = video.settings();
            s.codec = codec;
            s.bitrate_kbps = default_kbps(codec);
            let t0 = Instant::now();
            let r = video.update(s);
            println!(
                "[{el:.2}s] 코덱 변경 {codec}: {r:?} ({} ms)",
                t0.elapsed().as_millis()
            );
            key_requests.push((el, Instant::now()));
        }
        if let Some(t) = next_key
            && el >= t
        {
            video.request_keyframe();
            key_requests.push((el, Instant::now()));
            next_key = Some(t + args.keyframe_every.unwrap());
        }
        if last_stats.elapsed() >= Duration::from_secs(1) {
            last_stats = Instant::now();
            let s = video.stats();
            println!(
                "[{el:.1}s] {} {}x{} (원본 {}x{}) cap {:.1} enc {:.1} fps, enc {:.2} ms, conv {:.2} ms, 처리 {:.2} ms, 캡처시각→완료 {:.2} ms, {:.0} kbps, drop {}, gpu={}",
                s.encoder,
                s.width,
                s.height,
                s.source_width,
                s.source_height,
                s.capture_fps,
                s.encode_fps,
                s.avg_encode_ms,
                s.avg_convert_ms,
                s.avg_pipeline_ms,
                s.avg_latency_ms,
                s.bitrate_kbps,
                s.dropped_frames,
                s.gpu_convert
            );
            for l in video.layer_stats() {
                println!(
                    "        계층 1/{} {} {}x{} 목표 {} kbps: enc {:.1} fps, enc {:.2} ms, {:.0} kbps, drop {}, 프레임 {}{}",
                    l.divisor,
                    l.encoder,
                    l.width,
                    l.height,
                    l.target_kbps,
                    l.encode_fps,
                    l.avg_encode_ms,
                    l.bitrate_kbps,
                    l.dropped_frames,
                    l.frames,
                    l.last_error
                        .map(|e| format!(" 오류: {e}"))
                        .unwrap_or_default()
                );
            }
            if let Some(a) = &audio {
                let s = a.stats();
                println!(
                    "        audio {} excluded={} level {:.1} dBFS packets {} silence {} ms dropped {} ms err={:?}",
                    s.mode,
                    s.discord_excluded,
                    s.level_dbfs,
                    s.packets,
                    s.silence_filled_ms,
                    s.dropped_ms,
                    s.last_error
                );
            }
            if !video.is_running() {
                println!("비디오 파이프라인 멈춤: {:?}", video.last_error());
                break;
            }
        }
        let ev = match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(ev) => ev,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };
        match ev {
            Event::Video(f, recv_qpc) => {
                let seg = segments.last_mut().unwrap();
                if seg.codec.is_some_and(|c| c != f.codec) {
                    // 코덱이 바뀌었다: 새 파일
                    let old = std::mem::replace(
                        &mut writer,
                        Writer::create(&format!("{}-2", args.out), f.codec, f.width, f.height).0,
                    );
                    old.finish();
                    path = format!("{}-2.{}", args.out, ext(f.codec));
                    paths.push(path.clone());
                    segments.push(Segment::default());
                    decoder = args.decode.then(|| Decoder::for_video(f.codec)).flatten();
                }
                let seg = segments.last_mut().unwrap();
                if seg.codec.is_none() {
                    seg.codec = Some(f.codec);
                    if let Writer::Ivf { file, .. } = &mut writer {
                        // 첫 프레임 크기로 IVF 헤더를 고친다.
                        file.flush().unwrap();
                        let inner = file.get_mut();
                        inner.seek(SeekFrom::Start(12)).unwrap();
                        inner.write_all(&(f.width as u16).to_le_bytes()).unwrap();
                        inner.write_all(&(f.height as u16).to_le_bytes()).unwrap();
                        inner.seek(SeekFrom::End(0)).unwrap();
                    }
                }
                if f.keyframe {
                    seg.keyframes.push(start.elapsed().as_secs_f64());
                    if let Some((_, at)) = key_requests.first() {
                        key_delays.push(at.elapsed().as_secs_f32() * 1000.0);
                        key_requests.remove(0);
                    }
                    check_keyframe(&f);
                }
                seg.frames += 1;
                seg.bytes += f.data.len() as u64;
                *seg.sizes.entry((f.width, f.height)).or_default() += 1;
                seg.latencies
                    .push((recv_qpc - f.capture_qpc_100ns) as f32 / 10_000.0);
                writer.write(&f);
                if let Some(d) = &mut decoder {
                    let qpc = f.capture_qpc_100ns;
                    let mut white = None;
                    let mut n = 0;
                    seg.decode_errors += d.decode(&f.data, |fr| {
                        n += 1;
                        white = Some(mean_luma(fr) > 200.0);
                    });
                    seg.decoded += n;
                    if let Some(w) = white {
                        if w && !prev_white_luma {
                            white_qpcs.push(qpc);
                            white_emit_ms.push((recv_qpc - qpc) as f32 / 10_000.0);
                        }
                        prev_white_luma = w;
                    }
                }
            }
            Event::Audio(p, recv_qpc) => {
                audio_packets += 1;
                audio_bytes += p.data.len() as u64;
                if let Some(prev) = last_sample_index
                    && p.sample_index != prev + AUDIO_PACKET_SAMPLES
                {
                    audio_gaps += 1;
                }
                last_sample_index = Some(p.sample_index);
                audio_arrivals.push((recv_qpc - p.capture_qpc_100ns) as f32 / 10_000.0);
                if let Some(d) = &mut adecoder {
                    let qpc = p.capture_qpc_100ns;
                    d.decode(&p.data, |fr| unsafe {
                        // opus 디코더 출력: float planar 또는 interleaved
                        let n = fr.nb_samples as usize;
                        let ch0 = std::slice::from_raw_parts(fr.data[0] as *const f32, n);
                        for (j, &v) in ch0.iter().enumerate() {
                            if v.abs() > 0.1 {
                                if silent_run > 2400 {
                                    beep_qpcs.push(qpc + (j as i64 * QPC_PER_SEC) / 48_000);
                                    beep_emit_ms.push((recv_qpc - qpc) as f32 / 10_000.0);
                                }
                                silent_run = 0;
                            } else {
                                silent_run += 1;
                            }
                        }
                    });
                }
            }
        }
    }
    writer.finish();
    let vstats = video.stats();
    let lstats = video.layer_stats();
    video.stop();
    let astats = audio.as_ref().map(|a| a.stats());
    drop(audio);

    println!("\n=== 결과 ({:.1}s) ===", start.elapsed().as_secs_f64());
    for (seg, path) in segments.iter_mut().zip(&paths) {
        let mut lat = seg.latencies.clone();
        let avg = lat.iter().sum::<f32>() / lat.len().max(1) as f32;
        println!(
            "{path}: {:?} 프레임 {} 키프레임 {} {:?} 크기 {:?} {:.0} kbps, 지연(캡처→수신) 평균 {:.2} p95 {:.2} 최대 {:.2} ms, 디코딩 {} 오류 {}",
            seg.codec,
            seg.frames,
            seg.keyframes.len(),
            seg.keyframes
                .iter()
                .map(|t| format!("{t:.2}"))
                .collect::<Vec<_>>(),
            seg.sizes,
            seg.bytes as f64 * 8.0 / 1000.0 / args.secs,
            avg,
            percentile(&mut lat, 0.95),
            percentile(&mut lat, 1.0),
            seg.decoded,
            seg.decode_errors,
        );
    }
    println!("키프레임 요청→수신 지연(ms): {key_delays:.1?}");
    println!("비디오 통계: {vstats:?}");
    let run_secs = start.elapsed().as_secs_f64();
    for l in &lstats {
        println!(
            "simulcast 계층 1/{}: {} {}x{} 목표 {} kbps, 프레임 {} (평균 {:.1} fps), 키프레임 {}, drop {}, enc {:.2} ms{}",
            l.divisor,
            l.encoder,
            l.width,
            l.height,
            l.target_kbps,
            l.frames,
            l.frames as f64 / run_secs,
            l.keyframes,
            l.dropped_frames,
            l.avg_encode_ms,
            l.last_error
                .as_ref()
                .map(|e| format!(" 오류: {e}"))
                .unwrap_or_default()
        );
    }
    if !lstats.is_empty() {
        let main_frames: u64 = segments.iter().map(|s| s.frames).sum();
        println!(
            "주 인코더: 프레임 {main_frames} (평균 {:.1} fps)",
            main_frames as f64 / run_secs
        );
    }
    if let Some(a) = astats {
        let mut arr = audio_arrivals.clone();
        println!(
            "오디오: 패킷 {audio_packets} ({:.1}/s), {:.0} kbps, sample_index 불연속 {audio_gaps}, 캡처→수신 평균 {:.1} p95 {:.1} ms, 통계 {a:?}",
            audio_packets as f64 / args.secs,
            audio_bytes as f64 * 8.0 / 1000.0 / args.secs,
            arr.iter().sum::<f32>() / arr.len().max(1) as f32,
            percentile(&mut arr, 0.95),
        );
    }
    if args.decode && (!white_qpcs.is_empty() || !beep_qpcs.is_empty()) {
        println!(
            "흰 프레임 캡처 시각(QPC % 1초, 100ns): {:?}",
            white_qpcs
                .iter()
                .map(|q| q % QPC_PER_SEC)
                .collect::<Vec<_>>()
        );
        println!("흰 프레임 수신 지연(ms): {white_emit_ms:.1?}");
        println!(
            "삐 소리 시작 시각(QPC % 1초, 100ns): {:?}",
            beep_qpcs
                .iter()
                .map(|q| q % QPC_PER_SEC)
                .collect::<Vec<_>>()
        );
        println!("삐 소리 패킷 수신 지연(ms): {beep_emit_ms:.1?}");
        for w in &white_qpcs {
            if let Some(b) = beep_qpcs.iter().min_by_key(|b| (*b - w).abs()) {
                println!(
                    "  흰 프레임 {} 초 ↔ 삐 {} : 차이 {:+.3} ms ({:+.1} 샘플)",
                    w / QPC_PER_SEC,
                    b / QPC_PER_SEC,
                    (b - w) as f64 / 10_000.0,
                    (b - w) as f64 * 48_000.0 / QPC_PER_SEC as f64
                );
            }
        }
    }
}

/// 출력 파일 확장자.
fn ext(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::H264 => "h264",
        VideoCodec::Av1 => "ivf",
    }
}

/// 키프레임의 구성 요소 확인(H.264: 매개변수 집합, AV1: sequence header).
fn check_keyframe(f: &EncodedVideo) {
    match f.codec {
        VideoCodec::H264 => {
            let types = bitstream::nal_types(&f.data);
            if !(types.contains(&7) && types.contains(&8) && types.contains(&5)) {
                println!("!! 키프레임 NAL 구성 이상: {types:?}");
            }
        }
        VideoCodec::Av1 => match bitstream::obu_types(&f.data) {
            Ok(t) if t.first() == Some(&1) && !t.contains(&2) => {}
            other => println!("!! AV1 키프레임 OBU 구성 이상: {other:?}"),
        },
    }
}

fn bench_convert() {
    for (src, out) in [
        ((1920, 1080), (1920, 1080)),
        ((2560, 1440), (1920, 1080)),
        ((2560, 1440), (2560, 1440)),
        ((3840, 2160), (1920, 1080)),
        ((3840, 2160), (3840, 2160)),
    ] {
        for cpu in [false, true] {
            match dongsi_media::bench::convert(src, out, 120, cpu) {
                Ok(b) => println!(
                    "{:?} → {:?} {}: 평균 {:.2} ms, 최대 {:.2} ms",
                    src,
                    out,
                    if b.gpu {
                        "GPU(VideoProcessor)"
                    } else {
                        "CPU(swscale)"
                    },
                    b.avg_ms,
                    b.max_ms
                ),
                Err(e) => println!("{src:?} → {out:?} cpu={cpu}: 실패 {e}"),
            }
        }
    }
}

unsafe fn windows_dpi() -> bool {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetProcessDpiAwarenessContext(value: isize) -> i32;
    }
    // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = -4
    unsafe { SetProcessDpiAwarenessContext(-4) != 0 }
}
