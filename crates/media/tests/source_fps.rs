//! 방송 fps는 30·60뿐이다(D-107): 파이프라인이 설정 fps 그대로 프레임 수를 제한하고, 캡처 대상의 주사율을 따르지 않는지 본다.
//! 테스트 패턴과 이 PC의 실제 화면을 짧게 캡처한다(인코딩 결과는 메모리에만 둔다).
//! 전체 시험에만 든다(AGENTS.md "빌드·테스트"): `cargo test -p dongsi-media --features full-tests --test source_fps`

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use dongsi_core::VideoSettings;
use dongsi_media::{EncodedVideo, RefreshRates, SourceId, SourceKind, VideoPipeline, list_sources};

fn settings(fps: u32) -> VideoSettings {
    VideoSettings {
        width: 1280,
        height: 720,
        fps,
        bitrate_kbps: 2_500,
    }
}

fn start(source: SourceId, settings: VideoSettings) -> (VideoPipeline, Arc<AtomicU32>) {
    let frames = Arc::new(AtomicU32::new(0));
    let seen = frames.clone();
    let p = VideoPipeline::start(
        source,
        settings,
        Box::new(move |_: EncodedVideo| {
            seen.fetch_add(1, Ordering::Relaxed);
        }),
    )
    .expect("비디오 파이프라인 시작");
    (p, frames)
}

/// 0.3초 쉬고 1초 동안 나온 프레임 수.
fn frames_per_sec(frames: &AtomicU32) -> u32 {
    std::thread::sleep(Duration::from_millis(300));
    let before = frames.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_secs(1));
    frames.load(Ordering::Relaxed) - before
}

#[test]
fn test_pattern_runs_at_the_fps_setting() {
    // 30·60이 아닌 값(옛 "원본" 0, 144 등)은 60이다.
    for other in [0, 144] {
        let (p, _) = start(SourceId::TestPattern, settings(other));
        assert_eq!(p.settings().fps, 60, "fps {other}");
    }
    let (p, frames) = start(SourceId::TestPattern, settings(60));
    let sixty = frames_per_sec(&frames);
    p.update(settings(30)).unwrap();
    assert_eq!(p.settings().fps, 30);
    let thirty = frames_per_sec(&frames);
    eprintln!("[fps] 테스트 패턴: 60fps → 1초에 {sixty}장, 30fps → {thirty}장");
    // 다른 빌드·시험과 CPU를 나눠 쓰면 조금 모자랄 수 있다. 서로 가려지면 된다.
    assert!((40..=62).contains(&sixty), "테스트 패턴 60fps: {sixty}장");
    assert!((20..=31).contains(&thirty), "테스트 패턴 30fps: {thirty}장");
    // 방송 중에도 다른 값은 60이다.
    p.update(settings(144)).unwrap();
    assert_eq!(p.settings().fps, 60);
    assert!(p.is_running(), "{:?}", p.last_error());
}

#[test]
fn monitor_keeps_the_fps_setting_across_source_switch() {
    let rates = RefreshRates::now();
    let monitors: Vec<_> = list_sources()
        .unwrap()
        .into_iter()
        .filter(|s| s.kind == SourceKind::Monitor)
        .collect();
    let first = &monitors[0];
    let (p, _) = start(first.id.clone(), settings(30));
    std::thread::sleep(Duration::from_millis(1500));
    let stats = p.stats();
    eprintln!(
        "[fps] {} ({}×{}, 주사율 {:?}Hz): fps {} · 캡처 {:.1}/s(화면이 멈춰 있으면 적다), 인코딩 {:.1}/s",
        first.name,
        first.width,
        first.height,
        rates.get(&first.id),
        p.settings().fps,
        stats.capture_fps,
        stats.encode_fps
    );
    assert_eq!(p.settings().fps, 30);
    // fps 제한기가 설정을 넘지 않는다(모니터 주사율이 더 높아도).
    assert!(stats.capture_fps <= 32.0, "캡처 {:.1}/s", stats.capture_fps);
    // 대상을 바꿔도 fps는 설정 그대로다(모니터가 하나면 같은 모니터를 다시 연다).
    let next = monitors.get(1).unwrap_or(first);
    p.switch_source(next.id.clone()).expect("대상 바꾸기");
    std::thread::sleep(Duration::from_millis(1500));
    let stats = p.stats();
    eprintln!(
        "[fps] 대상 변경 → {} (주사율 {:?}Hz): fps {} · 캡처 {:.1}/s",
        next.name,
        rates.get(&next.id),
        p.settings().fps,
        stats.capture_fps
    );
    assert_eq!(p.settings().fps, 30);
    assert!(stats.capture_fps <= 32.0, "캡처 {:.1}/s", stats.capture_fps);
    assert!(p.is_running(), "{:?}", p.last_error());
}
