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
    /// 캡처 시각(QPC 100ns)
    qpc: i64,
}

fn frame(f: &EncodedVideo) -> Frame {
    Frame {
        layer: f.layer,
        keyframe: f.keyframe,
        capture: f.capture,
        qpc: f.capture_qpc_100ns,
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
        // 주 fps가 60 이하(30)면 계층도 주와 같은 fps다: 모든 프레임을 인코딩한다(D-100).
        let high_n = count(&got, Layer::High);
        assert!(
            n * 10 >= high_n * 9,
            "{l:?}: 주 fps 30에서 계층 프레임 {n}장은 주 {high_n}장보다 눈에 띄게 적습니다"
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

/// 주 fps가 60보다 크면 중·하는 캡처 시각 기준으로 프레임을 골라 평균 60fps로 인코딩한다(D-100). 테스트 패턴을 `fps`로
/// 돌려 `secs`초 동안 받은 프레임으로 본다. 하드웨어 인코더가 없는 PC면 계층이 쉬는 것만 확인한다.
///
/// 허용 범위의 근거: 테스트 패턴은 정초에 맞춘 `fps` 격자의 틱마다 프레임을 만든다(`pattern_next_tick`). 인코더가 멈추지
/// 않으면 `secs`초에 `fps·secs`장이고, 이 PC가 바쁘면 틱을 건너뛸 수 있어 주 프레임은 85% 이상만 본다. 계층은 평균 60fps라
/// `60·secs` ± 10%(시작 직후 인코더 여는 시간 몫)로 본다.
fn layers_hold_60(fps: u32, secs: u64) {
    let (tx, rx) = mpsc::channel::<Frame>();
    let layer_tx = std::sync::Mutex::new(tx.clone());
    let settings = VideoSettings {
        width: 640,
        height: 360,
        fps,
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
    assert_eq!(p.fps(), fps);
    // 인코더들이 열리고 안정될 때까지 건너뛴다.
    let _ = collect(&rx, Duration::from_millis(1000));
    let got = collect(&rx, Duration::from_secs(secs));
    let stats = p.stats();
    let layers = p.layer_stats();
    let (high, mid, low) = (
        count(&got, Layer::High),
        count(&got, Layer::Mid),
        count(&got, Layer::Low),
    );
    eprintln!(
        "[simulcast {fps}fps] 인코더 {} (하드웨어 {}), {secs}초: 상 {high}장({:.0}fps) · 중 {mid}장({:.0}fps) · 하 {low}장({:.0}fps), 통계 encode_fps 상 {:.0} · 중 {:.0} · 하 {:.0}, 계층 버린 수 {:?}",
        stats.encoder,
        stats.hardware,
        high as f64 / secs as f64,
        mid as f64 / secs as f64,
        low as f64 / secs as f64,
        stats.encode_fps,
        layers[0].encode_fps,
        layers[1].encode_fps,
        layers.iter().map(|l| l.dropped_frames).collect::<Vec<_>>()
    );
    assert!(
        high as u64 * 100 >= fps as u64 * secs * 85,
        "주 인코더가 {fps}fps에 못 미칩니다: {high}장"
    );
    if !stats.hardware {
        assert_eq!(mid + low, 0);
        return;
    }
    let expect = 60 * secs as usize;
    let eq = |l: Layer| {
        let n = count(&got, l);
        assert!(
            (expect * 90 / 100..=expect * 110 / 100).contains(&n),
            "{l:?}: {secs}초에 {n}장(기대 {expect} ± 10%)"
        );
        let times: Vec<i64> = got.iter().filter(|f| f.layer == l).map(|f| f.qpc).collect();
        // 프레임 간격(ms): 평균 16.7ms. 격자를 따라 고르므로 원본 프레임 간격(1000/fps)의 절반·한 장 몫만 흔들린다.
        let gaps: Vec<f64> = times
            .windows(2)
            .map(|w| (w[1] - w[0]) as f64 / 1e4)
            .collect();
        let (lo, hi) = (
            1000.0 / 60.0 - 1000.0 / fps as f64 * 1.01,
            1000.0 / 60.0 + 1000.0 / fps as f64 * 1.01,
        );
        let even = gaps.iter().filter(|g| (lo..=hi).contains(*g)).count();
        assert!(
            even * 100 >= gaps.len() * 90,
            "{l:?}: 간격 {lo:.1}~{hi:.1}ms 안인 것이 {even}/{}뿐입니다(최소 {:.1}, 최대 {:.1})",
            gaps.len(),
            gaps.iter().cloned().fold(f64::MAX, f64::min),
            gaps.iter().cloned().fold(0.0, f64::max)
        );
        times
    };
    let mid_times: HashSet<i64> = eq(Layer::Mid).into_iter().collect();
    let low_times: HashSet<i64> = eq(Layer::Low).into_iter().collect();
    // 중·하는 같은 프레임을 고른다.
    let both = mid_times.intersection(&low_times).count();
    assert!(
        both * 10 >= mid_times.len().min(low_times.len()) * 9,
        "중·하가 같은 캡처를 고른 것이 {both}장뿐입니다(중 {}, 하 {})",
        mid_times.len(),
        low_times.len()
    );
    // 고른 프레임은 모두 주 인코더가 인코딩한 캡처다.
    let high_times: HashSet<i64> = got
        .iter()
        .filter(|f| f.layer == Layer::High)
        .map(|f| f.qpc)
        .collect();
    assert!(
        mid_times.iter().filter(|t| high_times.contains(t)).count() * 10 >= mid_times.len() * 9
    );
    // 계층의 인코딩 fps 통계와 고르지 않은 프레임이 버린 수에 들지 않는 것.
    for l in &layers {
        assert!(
            (50.0..=70.0).contains(&l.encode_fps),
            "{:?} encode_fps {}",
            l.layer,
            l.encode_fps
        );
    }
    assert!(
        stats.encode_fps > 100.0,
        "주 encode_fps {}",
        stats.encode_fps
    );
    for l in &layers {
        // 계층이 바빠서 버린 프레임만 센다. 60fps 작은 해상도에서는 거의 없다.
        assert!(
            l.dropped_frames <= 10,
            "{:?} 버린 수 {}",
            l.layer,
            l.dropped_frames
        );
    }
    // 고른 프레임에서 키프레임 요청이 처리된다: 중 요청 → 1초 안에 중 키프레임, 하도 마찬가지.
    for l in [Layer::Mid, Layer::Low] {
        let _ = collect(&rx, Duration::from_millis(200));
        p.request_layer_keyframe(l);
        let got = collect(&rx, Duration::from_secs(1));
        assert!(
            got.iter().any(|f| f.layer == l && f.keyframe),
            "{fps}fps: {l:?} 키프레임 요청에 키프레임이 나오지 않았습니다"
        );
    }
    assert!(p.is_running());
}

#[test]
fn layers_cap_at_60_when_main_runs_120() {
    layers_hold_60(120, 3);
}

#[test]
fn layers_cap_at_60_when_main_runs_144() {
    layers_hold_60(144, 3);
}
