//! NVENC zero-copy(D-099 2단계) 파이프라인 시험. 테스트 패턴(GPU 경로, `Bars`)만 인코딩한다(화면 캡처·창·소리 없음).
//! 전체 시험에만 든다(AGENTS.md "시험"):
//! `cargo test -p dongsi-media --features full-tests --test zero_copy`
//!
//! 인코딩 결과를 빌드의 h264 디코더로 풀어 `bench::read_marks`로 프레임 번호와 막대 위치를 읽는다(`record --decode`와 같은
//! 방식). 번호가 순서대로 늘고(빠짐은 허용, 되풀이·역행 없음) 막대 x가 그 번호의 `bench::bar_x`와 맞으면 텍스처가 덜 그려지거나
//! 매핑 중에 덮어써지지 않은 것이다. 흰 프레임(1초마다, 표지 없음)은 건너뛴다.
//!
//! - `h264_nvenc`가 없거나 GPU 변환을 쓸 수 없는 PC에서는 zero-copy 단언을 건너뛰고 CPU 경로로 인코딩되는지만 본다.
//!   **하이브리드 GPU처럼 NVENC가 있어도 zero-copy가 내려가는 PC에서는 실패한다**(이 시험의 목적이 zero-copy가 도는지 보는
//!   것이다).
//! - 경로 고르기 환경 변수(`DONGSI_MEDIA_NO_ZEROCOPY`·`DONGSI_MEDIA_CPU_CONVERT`)는 파이프라인을 시작할 때(워커 생성), 시험
//!   전용 훅 `DONGSI_MEDIA_ZEROCOPY_FAIL`(`full-tests` 빌드에만 있다)은 인코더를 hw 입력으로 열 때 읽는다. 환경 변수는
//!   프로세스 전체라 시험마다 [`Env`]로 직렬화한다.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dongsi_core::{VideoCodec, VideoSettings};
use dongsi_media::bench::{bar_x, live_hw_frames, read_marks};
use dongsi_media::{EncodedVideo, FramePath, SourceId, VideoPipeline, available_encoders};
use ffmpeg_sys_next as ff;

// ---------------------------------------------------------------------------------------------
// 환경 변수 직렬화

static SERIAL: Mutex<()> = Mutex::new(());

/// 시험 하나 동안 환경 변수를 정하고(끝나면 지운다) 다른 시험이 동시에 환경을 흔들지 못하게 막는다.
struct Env {
    keys: Vec<&'static str>,
    _lock: MutexGuard<'static, ()>,
}

impl Env {
    fn set(vars: &[(&'static str, &str)]) -> Self {
        let lock = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        for key in [
            "DONGSI_MEDIA_NO_ZEROCOPY",
            "DONGSI_MEDIA_CPU_CONVERT",
            "DONGSI_MEDIA_ZEROCOPY_FAIL",
            "DONGSI_MEDIA_ENCODER",
        ] {
            // SAFETY: 시험 사이는 SERIAL로 직렬화했고, 이 시점에는 파이프라인 스레드가 없다.
            unsafe { std::env::remove_var(key) };
        }
        for (k, v) in vars {
            // SAFETY: 위와 같다.
            unsafe { std::env::set_var(k, v) };
        }
        Env {
            keys: vars.iter().map(|(k, _)| *k).collect(),
            _lock: lock,
        }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        for k in &self.keys {
            // SAFETY: 이 시험의 파이프라인은 이미 멈췄다(시험 본문이 끝났다).
            unsafe { std::env::remove_var(k) };
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 디코더(record.rs의 Decoder와 같은 방식)

struct Decoder {
    ctx: *mut ff::AVCodecContext,
    pkt: *mut ff::AVPacket,
    frame: *mut ff::AVFrame,
}

impl Decoder {
    fn h264() -> Self {
        unsafe {
            let name = std::ffi::CString::new("h264").unwrap();
            let codec = ff::avcodec_find_decoder_by_name(name.as_ptr());
            assert!(!codec.is_null(), "빌드에 h264 디코더가 없습니다");
            let ctx = ff::avcodec_alloc_context3(codec);
            assert!(ff::avcodec_open2(ctx, codec, std::ptr::null_mut()) >= 0);
            Decoder {
                ctx,
                pkt: ff::av_packet_alloc(),
                frame: ff::av_frame_alloc(),
            }
        }
    }

    /// 패킷 하나를 디코딩해 나온 프레임마다 `f`를 부른다. 디코딩 오류면 1.
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

// ---------------------------------------------------------------------------------------------
// 스트림 검사

#[derive(Default)]
struct Report {
    packets: usize,
    decoded: usize,
    /// 번호·막대를 읽은 프레임.
    marked: usize,
    /// 읽지 못한 프레임(1초마다 오는 흰 프레임 포함).
    unread: usize,
    decode_errors: u32,
    first: Option<u64>,
    last: Option<u64>,
    /// 번호가 건너뛴 곳(빠진 프레임이 있는 곳)의 수. 빠짐은 허용한다.
    gaps: usize,
    max_bar_dev: u32,
    /// 연속한 크기 구간: (너비, 높이, 프레임 수).
    runs: Vec<(u32, u32, usize)>,
    problems: Vec<String>,
    /// `keep_every`로 고른 프레임의 휘도 평면(너비×높이 빽빽하게).
    luma: BTreeMap<u64, Vec<u8>>,
}

impl Report {
    fn assert_ok(&self, what: &str) {
        assert!(
            self.problems.is_empty(),
            "{what}: 문제 {}건: {:#?}",
            self.problems.len(),
            &self.problems[..self.problems.len().min(10)]
        );
        assert_eq!(self.decode_errors, 0, "{what}: 디코딩 오류");
    }

    fn summary(&self) -> String {
        format!(
            "패킷 {} 디코드 {} (표지 {} · 못 읽음 {}) 오류 {} 번호 {:?}..{:?} 빠진 곳 {} 막대 최대 오차 {}px 크기 {:?}",
            self.packets,
            self.decoded,
            self.marked,
            self.unread,
            self.decode_errors,
            self.first,
            self.last,
            self.gaps,
            self.max_bar_dev,
            self.runs
        )
    }

    fn sizes(&self) -> Vec<(u32, u32)> {
        self.runs.iter().map(|r| (r.0, r.1)).collect()
    }
}

/// 스트림의 패킷을 차례로 디코딩해 검사한다. `marks`가 거짓이면 표지는 읽지 않는다(CPU 패턴).
fn analyze(packets: &[EncodedVideo], marks: bool, keep_every: Option<u64>) -> Report {
    let mut r = Report {
        packets: packets.len(),
        ..Report::default()
    };
    if packets.first().is_some_and(|p| !p.keyframe) {
        r.problems.push("첫 패킷이 키프레임이 아닙니다".into());
    }
    let mut dec = Decoder::h264();
    for p in packets {
        let report = &mut r;
        let errors = dec.decode(&p.data, |f| {
            let (w, h) = (f.width as u32, f.height as u32);
            report.decoded += 1;
            match report.runs.last_mut() {
                Some(run) if (run.0, run.1) == (w, h) => run.2 += 1,
                _ => report.runs.push((w, h, 1)),
            }
            if !marks {
                return;
            }
            let stride = f.linesize[0] as usize;
            // SAFETY: 디코더가 준 휘도 평면이다(stride × (h-1) + w 바이트가 유효하다).
            let y = unsafe {
                std::slice::from_raw_parts(f.data[0], stride * (h as usize - 1) + w as usize)
            };
            let Some((n, x)) = read_marks(y, stride, w, h) else {
                report.unread += 1;
                return;
            };
            report.marked += 1;
            if let Some(last) = report.last {
                if n <= last {
                    report.problems.push(format!(
                        "번호가 되풀이되거나 거꾸로 갔습니다: {last} 다음 {n}"
                    ));
                } else if n > last + 1 {
                    report.gaps += 1;
                }
            }
            report.first.get_or_insert(n);
            report.last = Some(n);
            let want = bar_x(n, w);
            let dev = x.abs_diff(want);
            report.max_bar_dev = report.max_bar_dev.max(dev);
            // 막대는 프레임마다 u/8px 움직인다. 반 걸음보다 어긋나면 번호와 막대가 다른 프레임에서 온 것이다.
            let step = (w / 20 / 8).max(1);
            if dev > step / 2 {
                report.problems.push(format!(
                    "프레임 {n}: 막대 x {x}, 기대 {want}(오차 {dev}px > {}px, {w}×{h})",
                    step / 2
                ));
            }
            if keep_every.is_some_and(|k| n % k == 0) {
                let mut plane = Vec::with_capacity((w * h) as usize);
                for row in 0..h as usize {
                    plane.extend_from_slice(&y[row * stride..row * stride + w as usize]);
                }
                report.luma.insert(n, plane);
            }
        });
        r.decode_errors += errors;
    }
    // 못 읽은 프레임은 1초마다 오는 흰 프레임뿐이어야 한다(60fps에서 약 1.7%).
    if marks {
        let allowed = r.decoded / 20 + 2;
        if r.unread > allowed {
            r.problems.push(format!(
                "표지를 못 읽은 프레임이 너무 많습니다({}/{}, 허용 {allowed})",
                r.unread, r.decoded
            ));
        }
    }
    r
}

// ---------------------------------------------------------------------------------------------
// 파이프라인 도우미

#[derive(Clone, Default)]
struct Collected(Arc<Mutex<Vec<EncodedVideo>>>);

impl Collected {
    fn push(&self, f: EncodedVideo) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(f);
    }

    /// 지금까지 모인 것을 가져가고 비운다.
    fn take(&self) -> Vec<EncodedVideo> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// 앞쪽의 `codec`이 아닌 프레임(바꾸기 전에 이미 나가던 것)을 뗀다. 첫 `codec` 프레임 뒤에 섞인 것은 남겨 시험이 잡는다.
fn strip_old_codec(frames: Vec<EncodedVideo>, codec: VideoCodec) -> Vec<EncodedVideo> {
    frames
        .into_iter()
        .skip_while(|f| f.codec != codec)
        .collect()
}

fn settings(codec: VideoCodec, w: u32, h: u32, fps: u32, kbps: u32) -> VideoSettings {
    VideoSettings {
        codec,
        width: w,
        height: h,
        fps,
        bitrate_kbps: kbps,
    }
}

fn start(s: VideoSettings) -> (VideoPipeline, Collected) {
    let out = Collected::default();
    let sink = out.clone();
    let on_frame = Box::new(move |f: EncodedVideo| sink.push(f));
    let p =
        VideoPipeline::start(SourceId::TestPattern, s, on_frame).expect("비디오 파이프라인 시작");
    (p, out)
}

fn has_encoder(name: &str) -> bool {
    available_encoders().iter().any(|e| e.name == name)
}

/// zero-copy를 기대하는 PC인가: `h264_nvenc`가 있고 이 파이프라인이 GPU 변환이다.
fn expects_zero_copy(p: &VideoPipeline) -> bool {
    has_encoder("h264_nvenc") && p.stats().gpu_convert
}

fn sleep(d: Duration) {
    std::thread::sleep(d);
}

/// 도는 동안 `live_hw_frames()`를 20ms마다 표본으로 본다.
struct LiveSampler {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<(usize, usize)>,
}

impl LiveSampler {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            let (mut max, mut n) = (0, 0);
            while !flag.load(Ordering::Relaxed) {
                max = max.max(live_hw_frames());
                n += 1;
                sleep(Duration::from_millis(20));
            }
            (max, n)
        });
        LiveSampler { stop, handle }
    }

    /// (최대, 표본 수).
    fn finish(self) -> (usize, usize) {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.join().unwrap()
    }
}

/// 살아 있는 hw 프레임의 상한: 워커가 쥔 1장과 인코더(NVENC)가 아직 쥔 몇 장. 표본이 8을 넘으면 어딘가가 프레임을
/// 놓지 않는 것이다.
const LIVE_LIMIT: usize = 8;

/// 파이프라인을 내린 뒤 살아 있는 hw 프레임이 0인지 본다(최대 2초 기다린다). 기다린 시간을 돌려준다.
fn assert_pool_drained(what: &str) -> Duration {
    let t = Instant::now();
    while live_hw_frames() != 0 && t.elapsed() < Duration::from_secs(2) {
        sleep(Duration::from_millis(10));
    }
    let left = live_hw_frames();
    assert_eq!(
        left, 0,
        "{what}: 파이프라인을 내린 뒤에도 hw 프레임이 남았습니다"
    );
    t.elapsed()
}

fn assert_zero_copy(p: &VideoPipeline, what: &str) {
    assert_eq!(
        p.frame_path(),
        FramePath::ZeroCopy,
        "{what}: zero-copy가 아닙니다(내려간 이유: {:?}, 오류: {:?})",
        p.zero_copy_fallback(),
        p.last_error()
    );
    assert_eq!(p.zero_copy_fallback(), None, "{what}");
}

// ---------------------------------------------------------------------------------------------
// 1. zero-copy 내용 확인 + 풀 누수

#[test]
fn zero_copy_content_is_intact() {
    let _env = Env::set(&[]);
    let (p, out) = start(settings(VideoCodec::H264, 1920, 1080, 60, 8_000));
    let zc = expects_zero_copy(&p);
    let sampler = LiveSampler::start();
    sleep(Duration::from_secs(3));
    let path = p.frame_path();
    let stats = p.stats();
    let fallback = p.zero_copy_fallback();
    let frames = out.take();
    p.stop();
    let (live_max, samples) = sampler.finish();
    let drained = assert_pool_drained("인코더");
    let rep = analyze(&frames, stats.gpu_convert, None);
    eprintln!(
        "[zero-copy 1] {} 경로 {path} 인코더 {}: {}\n    live 최대 {live_max}(표본 {samples}), 내린 뒤 0까지 {drained:?}",
        if zc {
            "zero-copy"
        } else {
            "건너뜀(NVENC 또는 GPU 변환 없음)"
        },
        stats.encoder,
        rep.summary()
    );
    if zc {
        assert_eq!(path, FramePath::ZeroCopy, "내려간 이유: {fallback:?}");
        assert_eq!(fallback, None);
        assert!(live_max >= 1, "표본이 hw 프레임을 한 번도 못 봤습니다");
        assert!(live_max <= LIVE_LIMIT, "live 최대 {live_max}");
    } else {
        assert_ne!(path, FramePath::ZeroCopy);
    }
    assert!(
        rep.decoded >= 120,
        "디코드한 프레임이 너무 적습니다: {}",
        rep.decoded
    );
    if stats.gpu_convert {
        assert!(rep.marked >= 100, "표지를 읽은 프레임이 너무 적습니다");
    }
    assert_eq!(rep.sizes(), [(1920, 1080)]);
    rep.assert_ok("스트림");
}

// ---------------------------------------------------------------------------------------------
// 2. 다른 경로와 휘도 비교

/// 스트림을 `secs`초 인코딩해 디코드 결과를 돌려준다. 4프레임마다(번호 % 4 == 0) 휘도 평면을 남긴다.
fn run_for_luma(secs: u64) -> (Report, FramePath, bool) {
    let (p, out) = start(settings(VideoCodec::H264, 1280, 720, 60, 6_000));
    sleep(Duration::from_secs(secs));
    let (path, gpu) = (p.frame_path(), p.stats().gpu_convert);
    let frames = out.take();
    p.stop();
    (analyze(&frames, gpu, Some(4)), path, gpu)
}

/// 같은 번호 프레임의 휘도 평균 절대 차이(두 쪽 모두 읽은 번호만). (프레임 수, 평균, 최대).
fn luma_diff(a: &Report, b: &Report) -> (usize, f64, f64) {
    let (mut n, mut sum, mut max) = (0usize, 0f64, 0f64);
    for (k, ya) in &a.luma {
        let Some(yb) = b.luma.get(k) else { continue };
        assert_eq!(ya.len(), yb.len(), "프레임 {k}: 크기가 다릅니다");
        let d = ya
            .iter()
            .zip(yb)
            .map(|(&p, &q)| p.abs_diff(q) as u64)
            .sum::<u64>() as f64
            / ya.len() as f64;
        n += 1;
        sum += d;
        max = max.max(d);
    }
    (n, if n == 0 { 0.0 } else { sum / n as f64 }, max)
}

/// 휘도 평균 절대 차이 임계(0~255 단위). 같은 GPU 변환 결과를 NVENC가 다시 압축하므로 압축 흔들림만 남는다.
const LUMA_LIMIT: f64 = 2.0;

#[test]
fn zero_copy_matches_other_paths() {
    let _env = Env::set(&[]);
    let (zc, zc_path, gpu) = run_for_luma(3);
    drop(_env);
    let zc_ok = has_encoder("h264_nvenc") && gpu;
    eprintln!("[zero-copy 2] zero-copy 쪽({zc_path}): {}", zc.summary());
    zc.assert_ok("zero-copy");
    if zc_ok {
        assert_eq!(zc_path, FramePath::ZeroCopy);
    }

    // GPU 변환 + 읽어 오기: 같은 GPU 패턴, 같은 변환, 인코더 입력 길만 다르다.
    let _env = Env::set(&[("DONGSI_MEDIA_NO_ZEROCOPY", "1")]);
    let (rb, rb_path, rb_gpu) = run_for_luma(3);
    drop(_env);
    eprintln!("[zero-copy 2] 읽어 오기 쪽({rb_path}): {}", rb.summary());
    rb.assert_ok("읽어 오기");
    if gpu {
        assert!(rb_gpu);
        assert_eq!(rb_path, FramePath::GpuReadback);
        let (n, mean, max) = luma_diff(&zc, &rb);
        eprintln!(
            "[zero-copy 2] zero-copy ↔ 읽어 오기: 같은 번호 {n}장, 휘도 평균 절대 차이 평균 {mean:.3} 최대 {max:.3}(임계 {LUMA_LIMIT})"
        );
        assert!(n >= 20, "같은 번호 프레임이 너무 적습니다({n})");
        assert!(max <= LUMA_LIMIT, "휘도 차이 {max:.3} > {LUMA_LIMIT}");
    }

    // CPU 변환: CPU로 그리는 패턴(다른 그림)이라 휘도를 번호별로 비교할 수 없다. 길과 인코딩·디코딩만 본다.
    let _env = Env::set(&[("DONGSI_MEDIA_CPU_CONVERT", "1")]);
    let (cpu, cpu_path, cpu_gpu) = run_for_luma(2);
    drop(_env);
    eprintln!("[zero-copy 2] CPU 변환 쪽({cpu_path}): {}", cpu.summary());
    cpu.assert_ok("CPU 변환");
    assert!(!cpu_gpu);
    assert_eq!(cpu_path, FramePath::CpuConvert);
    assert!(cpu.decoded >= 60, "디코드 {}", cpu.decoded);
    assert_eq!(cpu.sizes(), [(1280, 720)]);
}

// ---------------------------------------------------------------------------------------------
// 3. 도중 크기 변경

fn assert_phase(frames: &[EncodedVideo], size: (u32, u32), what: &str) {
    let first = frames
        .iter()
        .position(|f| (f.width, f.height) == size)
        .unwrap_or_else(|| panic!("{what}: {size:?} 프레임이 없습니다"));
    assert!(
        frames[first].keyframe,
        "{what}: 새 크기 첫 프레임이 키프레임이 아닙니다"
    );
    assert!(
        frames[first..].iter().all(|f| (f.width, f.height) == size),
        "{what}: 새 크기 뒤에 옛 크기 프레임이 섞였습니다"
    );
}

#[test]
fn resize_keeps_stream_intact() {
    let _env = Env::set(&[]);
    let big = settings(VideoCodec::H264, 1920, 1080, 60, 8_000);
    let small = settings(VideoCodec::H264, 1280, 720, 60, 6_000);
    let (p, out) = start(big);
    let zc = expects_zero_copy(&p);
    let gpu = p.stats().gpu_convert;
    let sampler = LiveSampler::start();
    let mut all: Vec<EncodedVideo> = Vec::new();
    let mut note = Vec::new();
    for (i, (s, size)) in [
        (big, (1920, 1080)),
        (small, (1280, 720)),
        (big, (1920, 1080)),
    ]
    .into_iter()
    .enumerate()
    {
        let t = Instant::now();
        if i > 0 {
            p.update(s).expect("update");
        }
        let took = t.elapsed();
        sleep(Duration::from_millis(1500));
        let frames = out.take();
        let st = p.stats();
        note.push(format!(
            "{size:?}: update {took:?}, 경로 {}, 통계 {}×{}, 프레임 {}",
            p.frame_path(),
            st.width,
            st.height,
            frames.len()
        ));
        if zc {
            assert_zero_copy(&p, &format!("단계 {i}"));
        }
        assert_eq!((st.width, st.height), size, "단계 {i}: 인코더 크기");
        if i > 0 {
            assert_phase(&frames, size, &format!("단계 {i}"));
        }
        all.extend(frames);
    }
    let (live_max, samples) = sampler.finish();
    assert!(p.is_running(), "{:?}", p.last_error());
    p.stop();
    let drained = assert_pool_drained("크기 변경");
    for line in &note {
        eprintln!("[zero-copy 3] {line}");
    }
    eprintln!("[zero-copy 3] live 최대 {live_max}(표본 {samples}), 내린 뒤 0까지 {drained:?}");
    if zc {
        assert!(live_max <= LIVE_LIMIT, "live 최대 {live_max}");
    }
    // 한 디코더가 크기가 두 번 바뀌는 스트림을 끝까지 푼다.
    let rep = analyze(&all, gpu, None);
    eprintln!("[zero-copy 3] {}", rep.summary());
    rep.assert_ok("크기 변경");
    assert_eq!(
        rep.sizes(),
        [(1920, 1080), (1280, 720), (1920, 1080)],
        "크기 구간"
    );
    assert!(rep.decoded >= 120, "디코드 {}", rep.decoded);
}

// ---------------------------------------------------------------------------------------------
// 4. 내려가기 경로(시험 전용 훅 DONGSI_MEDIA_ZEROCOPY_FAIL)

#[test]
fn encoder_failure_falls_back_to_readback() {
    let _env = Env::set(&[("DONGSI_MEDIA_ZEROCOPY_FAIL", "1")]);
    let (p, out) = start(settings(VideoCodec::H264, 1280, 720, 60, 6_000));
    let gpu = p.stats().gpu_convert;
    let zc = has_encoder("h264_nvenc") && gpu;
    let first_path = p.frame_path();
    let sampler = LiveSampler::start();
    let t = Instant::now();
    while p.frame_path() == FramePath::ZeroCopy && t.elapsed() < Duration::from_secs(5) {
        sleep(Duration::from_millis(20));
    }
    let after = t.elapsed();
    sleep(Duration::from_millis(2500));
    let path = p.frame_path();
    let fallback = p.zero_copy_fallback();
    let frames = out.take();
    let running = p.is_running();
    let err = p.last_error();
    p.stop();
    let (live_max, _) = sampler.finish();
    let drained = assert_pool_drained("내려가기");
    eprintln!(
        "[zero-copy 4] 처음 경로 {first_path}, 내려가기까지 {after:?}, 이후 경로 {path}, 이유 {fallback:?}, live 최대 {live_max}, 내린 뒤 0까지 {drained:?}"
    );
    assert!(running, "내려간 뒤 파이프라인이 멈췄습니다: {err:?}");
    if !zc {
        assert_ne!(path, FramePath::ZeroCopy);
        return;
    }
    assert_eq!(path, FramePath::GpuReadback);
    let reason = fallback.expect("내려간 이유가 있어야 합니다");
    assert!(reason.contains("시험용"), "{reason}");
    let rep = analyze(&frames, true, None);
    eprintln!("[zero-copy 4] {}", rep.summary());
    rep.assert_ok("내려가기");
    assert!(rep.decoded >= 90, "디코드 {}", rep.decoded);
    assert_eq!(rep.sizes(), [(1280, 720)]);
}

// ---------------------------------------------------------------------------------------------
// 5. 코덱 바꾸기 h264 → av1 → h264

#[test]
fn codec_switch_changes_path_without_errors() {
    let _env = Env::set(&[]);
    let h264 = settings(VideoCodec::H264, 1280, 720, 60, 6_000);
    let av1 = settings(VideoCodec::Av1, 1280, 720, 60, 4_000);
    if !has_encoder("libsvtav1") && !has_encoder("av1_nvenc") {
        eprintln!("[zero-copy 5] 건너뜀: 이 PC에는 AV1 인코더가 없습니다");
        return;
    }
    let av1_zc = has_encoder("av1_nvenc");
    let (p, out) = start(h264);
    let zc = expects_zero_copy(&p);
    let gpu = p.stats().gpu_convert;
    let sampler = LiveSampler::start();
    let mut note = Vec::new();
    let mut h264_segments: Vec<Vec<EncodedVideo>> = Vec::new();
    for (i, (s, codec)) in [
        (h264, VideoCodec::H264),
        (av1, VideoCodec::Av1),
        (h264, VideoCodec::H264),
    ]
    .into_iter()
    .enumerate()
    {
        if i > 0 {
            p.update(s).expect("update");
        }
        sleep(Duration::from_millis(1500));
        let frames = strip_old_codec(out.take(), codec);
        let st = p.stats();
        note.push(format!(
            "{codec:?}: 인코더 {} 경로 {} 프레임 {} 오류 {:?}",
            st.encoder,
            p.frame_path(),
            frames.len(),
            p.last_error()
        ));
        assert_eq!(st.codec, Some(codec), "단계 {i}");
        assert!(p.is_running(), "단계 {i}: {:?}", p.last_error());
        assert!(!frames.is_empty(), "단계 {i}: 프레임이 없습니다");
        assert!(
            frames.iter().all(|f| f.codec == codec),
            "단계 {i}: 이전 코덱 프레임이 섞였습니다"
        );
        if i > 0 {
            assert!(
                frames[0].keyframe,
                "단계 {i}: 코덱을 바꾼 뒤 첫 프레임이 키프레임이 아닙니다"
            );
        }
        if gpu {
            let want_zc = match codec {
                VideoCodec::H264 => zc,
                VideoCodec::Av1 => av1_zc,
            };
            if want_zc {
                assert_zero_copy(&p, &format!("단계 {i} {codec:?}"));
            } else {
                assert_ne!(p.frame_path(), FramePath::ZeroCopy, "단계 {i} {codec:?}");
            }
        }
        if codec == VideoCodec::H264 {
            h264_segments.push(frames);
        }
    }
    let (live_max, _) = sampler.finish();
    p.stop();
    let drained = assert_pool_drained("코덱 바꾸기");
    for line in &note {
        eprintln!("[zero-copy 5] {line}");
    }
    eprintln!("[zero-copy 5] live 최대 {live_max}, 내린 뒤 0까지 {drained:?}");
    // h264 구간(처음과 마지막)은 따로 디코드 검사한다(사이에 AV1이 끼어 있고 디코더는 h264뿐이다).
    for (i, frames) in h264_segments.iter().enumerate() {
        let rep = analyze(frames, gpu, None);
        eprintln!("[zero-copy 5] h264 구간 {i}: {}", rep.summary());
        rep.assert_ok(&format!("h264 구간 {i}"));
        assert!(rep.decoded >= 60, "h264 구간 {i} 디코드 {}", rep.decoded);
        assert_eq!(rep.sizes(), [(1280, 720)]);
    }
}
