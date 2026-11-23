//! 상 계층 쉬기(D-100) 시험. 테스트 패턴으로 simulcast 파이프라인을 열고, 상(주 인코더)을 뺐다가 다시 넣는다(화면을
//! 캡처하지 않는다). 하드웨어 인코더가 없는 PC면 중·하 계층이 쉬므로 상 쉼·돌아옴만 확인한다. 전체 시험에만 든다
//! (AGENTS.md "시험"): `cargo test -p dongsi-media --features full-tests --test high_pause`

use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use dongsi_core::{Layer, VideoSettings};
use dongsi_media::{EncodedVideo, LayerSink, SourceId, VideoPipeline};

struct Frame {
    layer: Layer,
    keyframe: bool,
    width: u32,
}

fn frame(f: &EncodedVideo) -> Frame {
    Frame {
        layer: f.layer,
        keyframe: f.keyframe,
        width: f.width,
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

fn settings() -> VideoSettings {
    VideoSettings {
        width: 640,
        height: 360,
        fps: 30,
        bitrate_kbps: 4_000,
        ..VideoSettings::DEFAULT
    }
}

fn open(settings: VideoSettings) -> (VideoPipeline, mpsc::Receiver<Frame>) {
    let (tx, rx) = mpsc::channel::<Frame>();
    let layer_tx = Mutex::new(tx.clone());
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
    (p, rx)
}

#[test]
fn high_rests_while_removed_and_restarts_with_a_keyframe() {
    let (p, rx) = open(settings());
    let hw = p.stats().hardware;
    eprintln!(
        "[high_pause] 인코더 {} (하드웨어 {hw}), 경로 {}",
        p.stats().encoder,
        p.frame_path()
    );
    let got = collect(&rx, Duration::from_secs(1));
    assert!(
        count(&got, Layer::High) >= 15,
        "처음에 상 프레임이 나와야 합니다"
    );
    if hw {
        assert!(count(&got, Layer::Mid) >= 15 && count(&got, Layer::Low) >= 15);
    }

    // 상을 뺀다: 상 프레임이 멈추고 중·하는 계속 나온다.
    p.set_layer_rates([0, 2_000, 1_000]);
    let _ = collect(&rx, Duration::from_millis(300)); // 이미 인코딩한 프레임을 비운다
    let frames_before = p.stats().frames;
    // 쉬는 동안 상 키프레임 요청(SFU PLI)을 보내도 상은 나가지 않는다.
    p.request_keyframe();
    p.request_layer_keyframe(Layer::High);
    let got = collect(&rx, Duration::from_millis(1_500));
    assert_eq!(
        count(&got, Layer::High),
        0,
        "쉬는 상 계층 프레임이 나왔습니다"
    );
    if hw {
        assert!(
            count(&got, Layer::Mid) >= 20,
            "중 {}",
            count(&got, Layer::Mid)
        );
        assert!(
            count(&got, Layer::Low) >= 20,
            "하 {}",
            count(&got, Layer::Low)
        );
    }
    // 통계: 쉬는 동안 인코딩이 0이어도 오류가 아니다.
    let s = p.stats();
    assert_eq!(
        s.frames, frames_before,
        "쉬는 동안 상 인코딩 수가 늘었습니다"
    );
    assert_eq!(s.encode_fps, 0.0);
    assert!(s.capture_fps > 10.0, "캡처는 계속 돈다({})", s.capture_fps);
    assert!(p.is_running());
    assert_eq!(p.last_error(), None);

    // 쉬는 동안 미리 보기는 계속 갱신된다.
    let first = {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(f) = p.preview(0, 320, 180) {
                break f;
            }
            assert!(
                Instant::now() < deadline,
                "쉬는 동안 미리 보기가 나오지 않습니다"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let next = {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(f) = p.preview(first.seq, 320, 180) {
                break f;
            }
            assert!(
                Instant::now() < deadline,
                "쉬는 동안 미리 보기가 갱신되지 않습니다"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    assert!(next.seq > first.seq);
    assert_eq!(
        count(&collect(&rx, Duration::from_millis(100)), Layer::High),
        0
    );

    // 쉬는 동안 상한이 바뀌어도(0은 무시) 돌아올 때 그 값으로 인코딩한다. 인코더는 그대로 두고 돌아올 때 건다.
    p.set_bitrate_cap(Some(1_500));
    p.set_bitrate_cap(Some(0));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        p.encoder_bitrate_kbps(),
        4_000,
        "쉬는 동안 인코더를 건드리지 않습니다"
    );

    // 상을 다시 넣는다: 첫 프레임은 키프레임이고 이어서 나온다.
    p.set_layer_rates([1_500, 2_000, 1_000]);
    let got = collect(&rx, Duration::from_millis(1_500));
    let high: Vec<&Frame> = got.iter().filter(|f| f.layer == Layer::High).collect();
    assert!(
        high.len() >= 20,
        "다시 넣은 상 프레임이 너무 적습니다({})",
        high.len()
    );
    assert!(
        high[0].keyframe,
        "다시 넣은 상의 첫 프레임이 키프레임이 아닙니다"
    );
    assert_eq!(
        p.encoder_bitrate_kbps(),
        1_500,
        "쉬는 동안 바뀐 상한이 적용돼야 합니다"
    );
    assert_eq!(p.last_error(), None);
    if hw {
        // 중·하는 상을 뺄 때도 넣을 때도 끊기지 않는다.
        assert!(count(&got, Layer::Mid) >= 20 && count(&got, Layer::Low) >= 20);
    }
    p.stop();
}

#[test]
fn settings_change_while_removed_applies_on_return() {
    let (p, rx) = open(settings());
    let _ = collect(&rx, Duration::from_millis(500));
    p.set_layer_rates([0, 2_000, 1_000]);
    let _ = collect(&rx, Duration::from_millis(300));

    // 쉬는 동안 크기를 바꿔도 상은 나가지 않고 오류도 없다.
    p.update(VideoSettings {
        width: 480,
        height: 270,
        ..settings()
    })
    .expect("쉬는 동안 화질 변경");
    let got = collect(&rx, Duration::from_millis(800));
    assert_eq!(count(&got, Layer::High), 0);
    assert_eq!(p.last_error(), None);

    // 돌아오면 새 크기로, 키프레임부터.
    p.set_layer_rates([4_000, 2_000, 1_000]);
    let got = collect(&rx, Duration::from_millis(1_200));
    let first = got
        .iter()
        .find(|f| f.layer == Layer::High)
        .expect("돌아온 상 프레임");
    assert!(first.keyframe);
    assert_eq!(first.width, 480);
    assert!(count(&got, Layer::High) >= 15);
    assert_eq!(p.last_error(), None);
    p.stop();
}
