//! simulcast 계층 인코딩(D-098) 시험. 테스트 패턴을 인코딩한다(화면을 캡처하지 않는다). 하드웨어 인코더가 없는 PC면
//! 계층이 쉬는 것만 확인한다. 전체 시험에만 든다(AGENTS.md "시험"):
//! `cargo test -p dongsi-media --features full-tests --test simulcast`

use std::collections::HashSet;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use dongsi_core::{Layer, VideoSettings};
use dongsi_media::{EncodedVideo, LayerSink, SourceId, VideoPipeline};

struct Frame {
    layer: Layer,
    keyframe: bool,
    capture: Instant,
}

fn frame(f: &EncodedVideo) -> Frame {
    Frame {
        layer: f.layer,
        keyframe: f.keyframe,
        capture: f.capture,
    }
}

/// `within` 동안 받은 프레임.
fn collect(rx: &mpsc::Receiver<Frame>, within: Duration) -> Vec<Frame> {
    let deadline = Instant::now() + within;
    let mut out = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(f) => out.push(f),
            Err(_) => break,
        }
    }
    out
}

fn count(frames: &[Frame], layer: Layer) -> usize {
    frames.iter().filter(|f| f.layer == layer).count()
}

#[test]
fn layers_encode_the_same_captures_and_follow_rates() {
    let (tx, rx) = mpsc::channel::<Frame>();
    let layer_tx = std::sync::Mutex::new(tx.clone());
    let settings = VideoSettings {
        width: 640,
        height: 360,
        fps: 30,
        bitrate_kbps: 4_000,
        ..VideoSettings::DEFAULT
    };
    let sink: LayerSink = Arc::new(move |f: EncodedVideo| {
        let _ = layer_tx.lock().unwrap().send(frame(&f));
    });
    let p = VideoPipeline::start_simulcast(
        SourceId::TestPattern,
        settings,
        Box::new(move |f: EncodedVideo| {
            let _ = tx.send(frame(&f));
        }),
        sink,
    )
    .expect("비디오 파이프라인 시작");
    assert!(p.is_simulcast());
    let got = collect(&rx, Duration::from_secs(3));
    let stats = p.stats();
    let layers = p.layer_stats();
    eprintln!(
        "[simulcast] 인코더 {} (하드웨어 {}), 3초: 상 {} · 중 {} · 하 {}",
        stats.encoder,
        stats.hardware,
        count(&got, Layer::High),
        count(&got, Layer::Mid),
        count(&got, Layer::Low)
    );
    assert!(
        count(&got, Layer::High) >= 30,
        "주 인코더 프레임이 너무 적습니다"
    );
    assert_eq!(layers.len(), 2);
    if !stats.hardware {
        // 소프트웨어 인코더면 계층은 쉰다(사용자 결정).
        assert!(layers.iter().all(|l| l.software_paused), "{layers:?}");
        assert_eq!(count(&got, Layer::Mid) + count(&got, Layer::Low), 0);
        return;
    }
    assert_eq!(
        layers
            .iter()
            .map(|l| (l.layer, l.target_kbps))
            .collect::<Vec<_>>(),
        [(Layer::Mid, 2_000), (Layer::Low, 1_000)]
    );
    for l in [Layer::Mid, Layer::Low] {
        let n = count(&got, l);
        assert!(n >= 30, "{l:?} 계층 프레임이 너무 적습니다({n})");
        // 계층 첫 프레임은 키프레임이다.
        assert!(
            got.iter()
                .find(|f| f.layer == l)
                .is_some_and(|f| f.keyframe),
            "{l:?} 첫 프레임"
        );
        // 계층은 주 인코더와 같은 캡처를 인코딩한다(RTP 시각이 같아 SFU가 계층을 바꿔도 시각이 이어진다).
        let high: HashSet<Instant> = got
            .iter()
            .filter(|f| f.layer == Layer::High)
            .map(|f| f.capture)
            .collect();
        let same = got
            .iter()
            .filter(|f| f.layer == l && high.contains(&f.capture))
            .count();
        assert!(
            same * 10 >= n * 9,
            "{l:?}: 주 인코더와 캡처 시각이 같은 프레임이 {same}/{n}뿐입니다"
        );
    }

    // 하를 빼면(0) 하 프레임이 멈춘다.
    p.set_layer_rates([4_000, 2_000, 0]);
    let _ = collect(&rx, Duration::from_millis(300));
    let got = collect(&rx, Duration::from_secs(1));
    assert_eq!(count(&got, Layer::Low), 0, "뺀 하 계층 프레임이 나왔습니다");
    assert!(count(&got, Layer::Mid) >= 15);
    assert_eq!(p.layer_stats()[1].target_kbps, 0);

    // 다시 넣으면 키프레임부터 나온다.
    p.set_layer_rates([4_000, 2_000, 1_000]);
    let got = collect(&rx, Duration::from_secs(1));
    let first_low = got
        .iter()
        .find(|f| f.layer == Layer::Low)
        .expect("다시 넣은 하 계층 프레임");
    assert!(
        first_low.keyframe,
        "다시 넣은 하 계층의 첫 프레임이 키프레임이 아닙니다"
    );

    // 중 계층 키프레임 요청: 1초 안에 중 키프레임이 나온다.
    let _ = collect(&rx, Duration::from_millis(500));
    p.request_layer_keyframe(Layer::Mid);
    let got = collect(&rx, Duration::from_secs(1));
    assert!(
        got.iter().any(|f| f.layer == Layer::Mid && f.keyframe),
        "중 계층 키프레임 요청에 키프레임이 나오지 않았습니다"
    );
    assert!(p.is_running());
}
