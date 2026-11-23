//! QP 기반 해상도 자동 낮추기(D-099) 파이프라인 시험. 테스트 패턴(GPU 경로)만 인코딩한다(화면 캡처·창·소리 없음).
//! 전체 시험에만 든다(AGENTS.md "시험"):
//! `cargo test -p dongsi-media --features full-tests --test auto_resolution`
//!
//! 압축하기 어려운 `Noise` 그림을 낮은 비트레이트로 보내면 QP가 임계값을 넘어 출력이 QHD에서 FHD로 내려가고, 쉬운 `Bars`로
//! 바꾸면 되돌아온다. 내림은 기본 판정 시간(연 뒤 1초 제외, 평균 3초 창, 3초 유지)으로 보고, 올림만 시험 훅
//! `DONGSI_MEDIA_QP_FAST_UP=1`(`full-tests` 빌드에만 있다)으로 2초 대기로 줄인다. 그림을 바꾸는 훅은
//! `VideoPipeline::set_test_pattern_noise`(같은 기능에만 있다).
//!
//! - `h264_nvenc`가 없는 PC에서는 건너뛴다(QP를 알려 주는 인코더가 필요하다).
//! - 환경 변수는 프로세스 전체라 시험마다 직렬화한다([`Env`]).

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use dongsi_core::{Layer, VideoCodec, VideoSettings};
use dongsi_media::{EncodedVideo, LayerSink, SourceId, VideoPipeline, available_encoders};

const QHD: (u32, u32) = (2560, 1440);
const FHD: (u32, u32) = (1920, 1080);

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
        let mut keys = vec!["DONGSI_MEDIA_QP_FAST_UP"];
        // SAFETY: 위와 같다.
        unsafe { std::env::set_var("DONGSI_MEDIA_QP_FAST_UP", "1") };
        for (k, v) in vars {
            // SAFETY: 위와 같다.
            unsafe { std::env::set_var(k, v) };
            keys.push(k);
        }
        Env { keys, _lock: lock }
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

/// 나온 프레임의 요약(데이터는 보관하지 않는다).
#[derive(Clone, Copy, Debug)]
struct Rec {
    layer: Layer,
    size: (u32, u32),
    keyframe: bool,
}

#[derive(Clone, Default)]
struct Collected(Arc<Mutex<Vec<Rec>>>);

impl Collected {
    fn push(&self, f: &EncodedVideo) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(Rec {
            layer: f.layer,
            size: (f.width, f.height),
            keyframe: f.keyframe,
        });
    }

    fn all(&self) -> Vec<Rec> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// `layer`의 크기가 바뀐 곳마다 (새 크기, 그 첫 프레임이 키프레임인가). 첫 프레임도 한 건이다.
    fn timeline(&self, layer: Layer) -> Vec<((u32, u32), bool)> {
        let mut out: Vec<((u32, u32), bool)> = Vec::new();
        for r in self.all().into_iter().filter(|r| r.layer == layer) {
            if out.last().is_none_or(|l| l.0 != r.size) {
                out.push((r.size, r.keyframe));
            }
        }
        out
    }

    fn last_size(&self, layer: Layer) -> Option<(u32, u32)> {
        self.all()
            .iter()
            .rev()
            .find(|r| r.layer == layer)
            .map(|r| r.size)
    }
}

fn settings(fps: u32, kbps: u32) -> VideoSettings {
    VideoSettings {
        codec: VideoCodec::H264,
        width: QHD.0,
        height: QHD.1,
        fps,
        bitrate_kbps: kbps,
    }
}

fn start(s: VideoSettings, simulcast: bool) -> (VideoPipeline, Collected) {
    let out = Collected::default();
    let main = out.clone();
    let on_frame = Box::new(move |f: EncodedVideo| main.push(&f));
    let p = if simulcast {
        let layers = out.clone();
        let sink: LayerSink = Arc::new(move |f: EncodedVideo| layers.push(&f));
        VideoPipeline::start_simulcast(SourceId::TestPattern, s, on_frame, sink)
    } else {
        VideoPipeline::start(SourceId::TestPattern, s, on_frame)
    }
    .expect("비디오 파이프라인 시작");
    (p, out)
}

fn has_encoder(name: &str) -> bool {
    available_encoders().iter().any(|e| e.name == name)
}

/// `cond`가 참이 될 때까지 50ms마다 본다. 걸린 시간(시한 안에 못 되면 `None`).
fn wait_for(timeout: Duration, mut cond: impl FnMut() -> bool) -> Option<Duration> {
    let t = Instant::now();
    while t.elapsed() < timeout {
        if cond() {
            return Some(t.elapsed());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    cond().then(|| t.elapsed())
}

fn skip_without_nvenc() -> bool {
    if has_encoder("h264_nvenc") {
        return false;
    }
    eprintln!("h264_nvenc가 없어 건너뜁니다(QP를 알려 주는 인코더가 필요합니다)");
    true
}

/// 주 인코더: 어려운 그림이면 QHD → FHD(기본 판정 시간), 쉬운 그림이면 FHD → QHD(올림만 시험 훅으로 줄임), 어려운 그림이
/// 다시 오면 또 내려가고, 끄면 설정한 크기로 돌아온다. 크기가 바뀔 때마다 새 크기의 첫 프레임은 키프레임이고 되풀이 전환은 없다.
#[test]
fn main_drops_recovers_and_off_restores() {
    if skip_without_nvenc() {
        return;
    }
    let _env = Env::set(&[]);
    let (p, out) = start(settings(30, 1500), false);
    p.set_auto_resolution(true);
    p.set_test_pattern_noise(true);
    let t0 = Instant::now();

    let drop_after = wait_for(Duration::from_secs(25), || {
        out.last_size(Layer::High) == Some(FHD)
    })
    .expect("Noise 1500 kbps에서 FHD로 내려가지 않았습니다");
    eprintln!(
        "내림: 시작 후 {:.1}s (QP {:?})",
        t0.elapsed().as_secs_f32(),
        p.stats().qp
    );
    assert!(
        drop_after >= Duration::from_secs(3),
        "판정 시간보다 빨리 내려갔다: {drop_after:?}"
    );
    assert_eq!(out.timeline(Layer::High), vec![(QHD, true), (FHD, true)]);
    assert!(p.stats().auto_lowered && p.stats().auto_step == 1);
    wait_for(Duration::from_secs(5), || p.stats().qp.is_some()).expect("QP를 알려 주지 않는다");
    // 어려운 그림이 이어지는 동안은 내려간 채로 있다(되풀이 전환 없음).
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(out.timeline(Layer::High).len(), 2);

    // 쉬운 그림: 올림.
    p.set_test_pattern_noise(false);
    let up_after = wait_for(Duration::from_secs(30), || {
        out.last_size(Layer::High) == Some(QHD) && out.timeline(Layer::High).len() == 3
    })
    .expect("Bars에서 QHD로 올라오지 않았습니다");
    eprintln!(
        "올림: 그림을 바꾼 뒤 {up_after:.1?} (QP {:?})",
        p.stats().qp
    );
    assert_eq!(
        out.timeline(Layer::High),
        vec![(QHD, true), (FHD, true), (QHD, true)]
    );
    assert!(!p.stats().auto_lowered && p.stats().auto_step == 0);

    // 어려운 그림이 다시 오면 또 내려가고, 끄면 설정 크기로 돌아온다.
    p.set_test_pattern_noise(true);
    wait_for(Duration::from_secs(30), || {
        out.timeline(Layer::High).len() == 4
    })
    .expect("두 번째 내림이 없습니다");
    p.set_auto_resolution(false);
    let off_after = wait_for(Duration::from_secs(10), || {
        out.timeline(Layer::High).len() == 5
    })
    .expect("끄기가 설정 크기로 되돌리지 못했습니다");
    eprintln!("끄기: 되돌아오기까지 {off_after:.1?}");
    assert_eq!(
        out.timeline(Layer::High),
        vec![
            (QHD, true),
            (FHD, true),
            (QHD, true),
            (FHD, true),
            (QHD, true)
        ]
    );
    assert!(!p.stats().auto_lowered);
    // 꺼 둔 동안은 어려운 그림이어도 그대로다.
    std::thread::sleep(Duration::from_secs(4));
    assert_eq!(out.timeline(Layer::High).len(), 5);
    p.stop();
}

/// simulcast: 계층마다 따로 판정한다. 비트레이트가 넉넉한 상·중은 QHD를 지키고 1/4인 하만 FHD로 내려간다. 끄면 하도 돌아온다.
#[test]
fn layers_decide_independently() {
    if skip_without_nvenc() {
        return;
    }
    let _env = Env::set(&[]);
    let (p, out) = start(settings(LAYER_FPS, LAYER_HIGH_KBPS), true);
    p.set_auto_resolution(true);
    p.set_test_pattern_noise(true);
    let t0 = Instant::now();

    let mut last_log = Instant::now();
    wait_for(Duration::from_secs(40), || {
        if last_log.elapsed() >= Duration::from_secs(3) {
            last_log = Instant::now();
            eprintln!(
                "  [{:.0}s] 주 QP {:?} step {}, 계층 {:?}",
                t0.elapsed().as_secs_f32(),
                p.stats().qp,
                p.stats().auto_step,
                p.layer_stats()
                    .iter()
                    .map(|l| (
                        l.layer,
                        l.qp.map(|q| q.round()),
                        l.auto_step,
                        l.bitrate_kbps.round()
                    ))
                    .collect::<Vec<_>>()
            );
        }
        out.last_size(Layer::Low) == Some(FHD)
    })
    .expect("하 계층이 FHD로 내려가지 않았습니다");
    let stats = p.layer_stats();
    eprintln!(
        "하 계층 내림: 시작 후 {:.1}s, 주 QP {:?}, 계층 QP {:?}",
        t0.elapsed().as_secs_f32(),
        p.stats().qp,
        stats
            .iter()
            .map(|l| (l.layer, l.qp, l.auto_step))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        out.timeline(Layer::High),
        vec![(QHD, true)],
        "상은 크기를 지켜야 한다"
    );
    assert_eq!(
        out.timeline(Layer::Mid),
        vec![(QHD, true)],
        "중은 크기를 지켜야 한다"
    );
    assert_eq!(out.timeline(Layer::Low), vec![(QHD, true), (FHD, true)]);
    let low = stats.iter().find(|l| l.layer == Layer::Low).unwrap();
    let mid = stats.iter().find(|l| l.layer == Layer::Mid).unwrap();
    assert!(
        low.auto_lowered && (low.width, low.height) == FHD,
        "{low:?}"
    );
    assert!(!mid.auto_lowered, "{mid:?}");
    assert!(!p.stats().auto_lowered);

    p.set_auto_resolution(false);
    wait_for(Duration::from_secs(10), || {
        out.timeline(Layer::Low).len() == 3
    })
    .expect("끄기가 하 계층을 되돌리지 못했습니다");
    assert_eq!(
        out.timeline(Layer::Low),
        vec![(QHD, true), (FHD, true), (QHD, true)]
    );
    assert_eq!(out.timeline(Layer::High).len(), 1);
    assert!(p.layer_stats().iter().all(|l| !l.auto_lowered));
    p.stop();
}

/// 꺼 두면(기본) 어려운 그림이어도 크기를 그대로 두고 QP만 통계에 센다. QP를 알려 주지 않는 인코더(libopenh264)는 켜도 크기를
/// 그대로 두고 QP가 `None`이다.
#[test]
fn off_and_encoders_without_qp_keep_the_size() {
    // 1) 꺼 둠(nvenc)
    if !skip_without_nvenc() {
        let _env = Env::set(&[]);
        let (p, out) = start(settings(30, 1500), false);
        p.set_test_pattern_noise(true);
        assert!(!p.auto_resolution());
        let qp = wait_for(Duration::from_secs(10), || p.stats().qp.is_some())
            .expect("꺼 둬도 QP는 통계에 있어야 한다");
        std::thread::sleep(Duration::from_secs(4));
        let s = p.stats();
        eprintln!("꺼 둠: QP {:?} ({qp:.1?} 만에 표본)", s.qp);
        assert!(s.qp.is_some_and(|q| q >= 30.0), "{:?}", s.qp);
        assert_eq!(out.timeline(Layer::High), vec![(QHD, true)]);
        assert!(!s.auto_lowered);
        p.stop();
    }
    // 2) QP를 알려 주지 않는 인코더(소프트웨어 libopenh264)는 켜도 그대로.
    if has_encoder("libopenh264") {
        let _env = Env::set(&[("DONGSI_MEDIA_ENCODER", "libopenh264")]);
        let (p, out) = start(settings(15, 1500), false);
        p.set_auto_resolution(true);
        p.set_test_pattern_noise(true);
        std::thread::sleep(Duration::from_secs(9));
        let s = p.stats();
        assert_eq!(s.encoder, "libopenh264");
        assert!(s.frames > 20, "프레임이 거의 없다: {}", s.frames);
        assert_eq!(s.qp, None);
        assert!(!s.auto_lowered);
        assert_eq!(out.timeline(Layer::High), vec![(QHD, true)]);
        p.stop();
    }
}

/// 계층 시험에서 쓰는 값(상 비트레이트, fps). 중·하는 1/2·1/4. Noise(압축 불가)를 낮은 fps로 보내 상·중은 QP가 임계값(37)
/// 아래로, 1/4인 하만 위로 가게 맞춘 값이다(이 PC의 NVENC로 잰 값: 상 QP 29 · 중 32 · 하 39).
const LAYER_HIGH_KBPS: u32 = 70_000;
const LAYER_FPS: u32 = 4;
