//! 네트워크에 맞춘 비트레이트 상한(D-081) 시험. 테스트 패턴을 인코딩한다(화면을 캡처하지 않는다).
//! 전체 시험에만 든다(AGENTS.md "빌드·테스트"): `cargo test -p dongsi-media --features full-tests --test bitrate_cap`

use std::sync::mpsc;
use std::time::{Duration, Instant};

use dongsi_core::VideoSettings;
use dongsi_media::{EncodedVideo, SourceId, VideoPipeline};

fn wait_for(what: &str, within: Duration, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    while !f() {
        assert!(
            Instant::now() < deadline,
            "{what}: {within:?} 안에 되지 않았습니다"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn cap_limits_the_encoder_bitrate() {
    let (tx, rx) = mpsc::channel::<bool>();
    let settings = VideoSettings {
        width: 640,
        height: 360,
        fps: 30,
        bitrate_kbps: 4_000,
    };
    let p = VideoPipeline::start(
        SourceId::TestPattern,
        settings,
        Box::new(move |f: EncodedVideo| {
            let _ = tx.send(f.keyframe);
        }),
    )
    .expect("비디오 파이프라인 시작");
    let w = Duration::from_secs(5);
    wait_for("처음 프레임", w, || rx.try_recv().is_ok());
    assert_eq!(p.encoder_bitrate_kbps(), 4_000);

    // 상한을 낮추면 인코더 비트레이트가 따라가고 키프레임이 나간다. 키프레임은 상한을 적용하자마자 나올 수 있으므로
    // 받은 프레임은 상한을 바꾸기 전에 비우고, 그 뒤에 받은 프레임에서 찾는다.
    while rx.try_recv().is_ok() {}
    p.set_bitrate_cap(Some(1_200));
    wait_for("상한 적용", w, || p.encoder_bitrate_kbps() == 1_200);
    // 재생성한 경우 force_key, nvenc는 IDR(reconfig).
    let mut saw_key = false;
    wait_for("상한 뒤 키프레임", w, || {
        saw_key |= rx.try_recv().unwrap_or(false);
        saw_key
    });
    assert_eq!(p.settings().bitrate_kbps, 4_000, "설정 비트레이트는 그대로");

    // 상한이 설정보다 높으면 설정이 이긴다.
    p.set_bitrate_cap(Some(9_000));
    wait_for("상한 올림", w, || p.encoder_bitrate_kbps() == 4_000);

    // 상한을 먼저 바꾸고 설정을 바꾸면 둘 중 작은 쪽.
    p.set_bitrate_cap(Some(2_000));
    p.update(VideoSettings {
        bitrate_kbps: 3_000,
        ..settings
    })
    .unwrap();
    assert_eq!(p.encoder_bitrate_kbps(), 2_000);
    p.set_bitrate_cap(None);
    wait_for("상한 없앰", w, || p.encoder_bitrate_kbps() == 3_000);
    assert!(p.is_running());
}
