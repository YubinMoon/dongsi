//! 주사율 "원본"(`VideoSettings.fps` 0, D-074): 파이프라인이 캡처 대상 모니터의 주사율로 풀어 쓰는지 본다.
//! 테스트 패턴(60)과 이 PC의 실제 화면을 짧게 캡처한다(인코딩 결과는 메모리에만 둔다).
//! 전체 시험에만 든다(AGENTS.md "빌드·테스트"): `cargo test -p dongsi-media --features full-tests --test source_fps`

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use dongsi_core::VideoSettings;
use dongsi_media::{
    EncodedVideo, RefreshRates, SourceId, SourceKind, TEST_PATTERN_REFRESH_HZ, VideoPipeline,
    list_sources,
};

fn source_fps() -> VideoSettings {
    VideoSettings {
        width: 1280,
        height: 720,
        fps: VideoSettings::SOURCE_FPS,
        bitrate_kbps: 2_500,
        ..VideoSettings::DEFAULT
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

#[test]
fn test_pattern_source_fps_is_60() {
    let (p, frames) = start(SourceId::TestPattern, source_fps());
    assert_eq!(p.fps(), TEST_PATTERN_REFRESH_HZ);
    assert_eq!(
        p.settings().fps,
        VideoSettings::SOURCE_FPS,
        "요청한 값은 그대로"
    );
    std::thread::sleep(Duration::from_millis(300));
    let before = frames.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_secs(1));
    let per_sec = frames.load(Ordering::Relaxed) - before;
    eprintln!(
        "[주사율 원본] 테스트 패턴: fps {} → 1초에 {per_sec}장",
        p.fps()
    );
    // 다른 빌드·시험과 CPU를 나눠 쓰면 조금 모자랄 수 있다. 30fps(또는 0 → 1fps)와 가려지면 된다.
    assert!(
        (40..=62).contains(&per_sec),
        "테스트 패턴 60fps: {per_sec}장"
    );
    // 정한 값으로 바꾸면 그 값, 다시 원본이면 60.
    p.update(VideoSettings {
        fps: 30,
        ..source_fps()
    })
    .unwrap();
    assert_eq!(p.fps(), 30);
    p.update(source_fps()).unwrap();
    assert_eq!(p.fps(), TEST_PATTERN_REFRESH_HZ);
}

#[test]
fn monitor_source_fps_follows_the_monitor_refresh_rate() {
    let rates = RefreshRates::now();
    let monitors: Vec<_> = list_sources()
        .unwrap()
        .into_iter()
        .filter(|s| s.kind == SourceKind::Monitor)
        .collect();
    let first = &monitors[0];
    let hz = rates.get(&first.id).expect("모니터 주사율");
    let (p, frames) = start(first.id.clone(), source_fps());
    eprintln!(
        "[주사율 원본] {} ({}×{}): 주사율 {hz}Hz → 파이프라인 fps {}",
        first.name,
        first.width,
        first.height,
        p.fps()
    );
    assert_eq!(p.fps(), hz.min(VideoSettings::MAX_FPS));
    std::thread::sleep(Duration::from_millis(1500));
    let stats = p.stats();
    eprintln!(
        "[주사율 원본] 1.5초 동안 {}장(화면이 멈춰 있으면 적다), 캡처 {:.1}/s, 인코딩 {:.1}/s",
        frames.load(Ordering::Relaxed),
        stats.capture_fps,
        stats.encode_fps
    );
    // 정한 fps로 바꾸면 그 값이고, 원본으로 돌아오면 다시 모니터 주사율이다.
    p.update(VideoSettings {
        fps: 30,
        ..source_fps()
    })
    .unwrap();
    assert_eq!(p.fps(), 30);
    p.update(source_fps()).unwrap();
    assert_eq!(p.fps(), hz.min(VideoSettings::MAX_FPS));
    // 대상을 바꾸면 새 모니터의 주사율로 다시 푼다(모니터가 하나면 같은 모니터를 다시 연다).
    let next = monitors.get(1).unwrap_or(first);
    p.switch_source(next.id.clone()).expect("대상 바꾸기");
    let next_hz = rates.get(&next.id).expect("모니터 주사율");
    eprintln!(
        "[주사율 원본] 대상 변경 → {}: 주사율 {next_hz}Hz → 파이프라인 fps {}",
        next.name,
        p.fps()
    );
    assert_eq!(p.fps(), next_hz.min(VideoSettings::MAX_FPS));
    assert!(p.is_running(), "{:?}", p.last_error());
}
