//! simulcast(D-102): 방송자가 rid 둘(상·하)로 보내면 SFU가 시청자마다 트랙 하나만 보낸다.
//!
//! 방송자는 두 트랙에 같은 tick의 프레임을 보내되 방송자 표시(tag)를 트랙마다 다르게 한다(`TAG + 트랙 자리`). 모든 비디오
//! 프레임이 키프레임이라(시험 미디어, `media.rs`) 트랙을 바꾸기로 하면 그 트랙의 다음 프레임에서 바로 바뀐다. 트랙 비트레이트는
//! `PublishLayers`로 알린 값이라, 시청자 추정값과 상관없이 고를 트랙을 정해 줄 수 있다:
//! - 아주 작은 값(`[300, 100]`)이면 상(추정이 몇백 kbps만 넘으면 된다).
//! - 상을 아주 크게(`[10Gbps, 100]`) 하면 하(추정이 그만큼 될 수 없다).
//! - 하 목표를 0으로 하고(하가 쉰다: 설정 3Mbps 이하, 소프트웨어 코덱) 하 프레임도 안 보내면 상.
//!
//! 확인하는 것:
//! - 시청자는 한 번에 한 트랙만 받는다(섞이지 않는다). 같은 tick은 트랙을 바꾸는 지점에서만 한 번 겹친다(이전 트랙의
//!   그 tick을 이미 보낸 뒤 새 트랙의 같은 tick 키프레임부터 바꾸므로). 시청자가 받는 RTP 시각은 늘 늘어난다(SFU가
//!   겹친 프레임을 1클록 뒤로 민다).
//! - 시청자의 PLI는 그 시청자가 받는 트랙(rid)의 키프레임 요청이 된다.
//! - 내림은 바로, 올림은 4초(`UP_HOLD`) 뒤에 바뀐다.
//! - 하가 쉬는 방송에서는 하를 보던 시청자도, 새 시청자의 첫 트랙도 상이다. 하가 돌아오면(상이 추정에 안 맞으면) 바로 하로
//!   내려간다.
//! - 전용 서버도 REMB를 보내지 않는다(simulcast 방송에는 내장 서버도 보내지 않는다).
//!
//! 외부 서버에서도 돈다(시청자·방송자 쪽 관찰로만 단언한다).

use std::time::{Duration, Instant};

use dongsi_core::proto::ClientMsg;
use dongsi_core::{Layer, VideoCodec};
use dongsi_net::publish::Publisher;
use str0m::media::MediaKind;

use crate::client::{Client, TEST_RATE};
use crate::media::{info, payload, video_frame};
use crate::target::Target;
use crate::viewer::{Received, TestViewer};

const TAG: u32 = 0x51C0;

/// 20ms마다 오디오, 40ms마다 두 트랙의 비디오(같은 tick, 트랙마다 다른 tag).
struct LayerPump {
    tick: u32,
    origin: Option<Instant>,
    /// 하 프레임을 보내지 않는다(하 인코더가 쉰다).
    skip_low: bool,
}

impl LayerPump {
    fn new() -> Self {
        LayerPump {
            tick: 0,
            origin: None,
            skip_low: false,
        }
    }

    /// 지금 tick(다음에 보낼 것).
    fn tick(&self) -> u32 {
        self.tick
    }

    async fn run(&mut self, p: &Publisher, ticks: u32) {
        let origin = *self.origin.get_or_insert_with(Instant::now);
        let elapsed = (origin.elapsed().as_millis() / 20) as u32;
        if elapsed > self.tick {
            self.tick = elapsed + 1;
        }
        for _ in 0..ticks {
            let tick = self.tick;
            let at = origin + Duration::from_millis(20 * u64::from(tick));
            let now = Instant::now();
            if at > now {
                tokio::time::sleep(at - now).await;
            }
            p.send_audio(payload(tick, TAG, 80), at, u64::from(tick) * 960);
            if tick.is_multiple_of(2) {
                for layer in Layer::ALL {
                    if self.skip_low && layer == Layer::Low {
                        continue;
                    }
                    let tag = TAG + layer.index() as u32;
                    p.send_video_layer(
                        layer,
                        VideoCodec::H264,
                        video_frame(VideoCodec::H264, tick, tag),
                        at,
                    );
                }
            }
            self.tick += 1;
        }
    }
}

fn layer_of(r: &Received) -> Option<Layer> {
    let tag = r.tag?;
    Layer::ALL
        .into_iter()
        .find(|l| TAG + l.index() as u32 == tag)
}

/// 받은 비디오의 (tick, 계층, RTP 시각).
fn videos(rs: &[Received]) -> Vec<(u32, Layer, u64)> {
    rs.iter()
        .filter(|r| r.kind == MediaKind::Video)
        .filter_map(|r| Some((r.index?, layer_of(r)?, r.rtp_time)))
        .collect()
}

/// tick은 트랙이 바뀌는 지점에서만 같을 수 있고(뒤로 가지는 않는다), RTP 시각은 늘 늘어난다.
fn assert_one_per_tick(v: &[(u32, Layer, u64)]) {
    for w in v.windows(2) {
        let switch = w[0].1 != w[1].1;
        assert!(
            w[0].0 < w[1].0 || (switch && w[0].0 == w[1].0),
            "같은 트랙에서 tick이 두 번 왔거나 순서가 바뀜: {w:?}"
        );
        assert!(w[0].2 < w[1].2, "RTP 시각이 늘지 않음: {w:?}");
    }
}

/// 방송자가 `layer`의 키프레임 요청을 받을 때까지 펌프를 돌린다. 받았으면 그때까지 다른 트랙 요청이 있었는지도 돌려준다.
async fn wait_layer_keyframe(
    pump: &mut LayerPump,
    p: &Publisher,
    layer: Layer,
) -> (bool, Vec<Layer>) {
    let mut others = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        pump.run(p, 5).await;
        for l in Layer::ALL {
            if p.take_layer_keyframe_request(l) {
                if l == layer {
                    return (true, others);
                }
                others.push(l);
            }
        }
    }
    (false, others)
}

fn clear_keyframe_requests(p: &Publisher) {
    for l in Layer::ALL {
        p.take_layer_keyframe_request(l);
    }
}

fn layers_msg(kbps: [u32; 2]) -> ClientMsg {
    ClientMsg::PublishLayers { kbps }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simulcast_viewer_gets_one_layer_and_switches() {
    let t = Target::shared_with(1).await;
    let mut a = Client::connect(&t, "simulcast 방송자").await;
    let rate = dongsi_net::rate::RateConfig {
        simulcast: true,
        ..TEST_RATE
    };
    let p = a
        .publish_with(info(VideoCodec::H264), None, rate)
        .await
        .unwrap();
    assert!(p.is_simulcast());
    // 설정 7000: 상 7000 + 하 3000.
    assert_eq!(p.layer_rates(), [7000, 3000]);
    a.send(layers_msg([300, 100]));
    let mut pump = LayerPump::new();
    pump.run(&p, 10).await;

    let mut v = Client::connect(&t, "simulcast 시청자").await;
    let w = v.watch_connected(a.id, None).await;
    let viewer: &TestViewer = &w.viewer;
    let mut all = Vec::new();

    // (1) 상만 온다.
    pump.run(&p, 100).await;
    let got = videos(&viewer.drain());
    all.extend_from_slice(&got);
    eprintln!(
        "[e2e] simulcast 처음 2초: {} 프레임, 트랙 {:?}",
        got.len(),
        got.iter()
            .map(|x| x.1.label())
            .collect::<std::collections::BTreeSet<_>>()
    );
    assert!(got.len() >= 30, "비디오가 거의 오지 않았습니다: {got:?}");
    assert!(
        got.iter().all(|x| x.1 == Layer::High),
        "상이 아닌 트랙이 섞였습니다: {got:?}"
    );
    assert_one_per_tick(&got);

    // (2) 시청자 PLI는 상 트랙의 키프레임 요청이 된다.
    clear_keyframe_requests(&p);
    viewer.request_keyframe();
    let (ok, others) = wait_layer_keyframe(&mut pump, &p, Layer::High).await;
    assert!(
        ok,
        "시청자 PLI가 상 트랙 요청으로 가지 않았습니다(다른 트랙: {others:?})"
    );
    assert!(
        others.is_empty(),
        "다른 트랙의 키프레임도 요청됨: {others:?}"
    );

    // (3) 상이 추정보다 크면 바로 하로 내린다.
    all.extend(videos(&viewer.drain()));
    a.send(layers_msg([10_000_000, 100]));
    let down_from = pump.tick();
    pump.run(&p, 75).await;
    let got = videos(&viewer.drain());
    all.extend_from_slice(&got);
    assert_one_per_tick(&got);
    let first_low = got
        .iter()
        .position(|x| x.1 == Layer::Low)
        .unwrap_or_else(|| panic!("하로 내려가지 않았습니다: {got:?}"));
    assert!(
        got[first_low..].iter().all(|x| x.1 == Layer::Low),
        "내린 뒤 다른 트랙이 섞였습니다: {got:?}"
    );
    assert!(
        got[..first_low].iter().all(|x| x.1 == Layer::High),
        "{got:?}"
    );
    let down_ms = (got[first_low].0 - down_from) * 20;
    eprintln!("[e2e] simulcast 내림: {down_ms} ms 뒤 하");
    assert!(down_ms <= 1000, "내림이 늦었습니다({down_ms} ms)");

    // (4) 이제 시청자 PLI는 하 트랙 요청이 된다(시청자 한 명의 요청은 2초에 한 번만 이어진다).
    pump.run(&p, 105).await;
    clear_keyframe_requests(&p);
    all.extend(videos(&viewer.drain()));
    viewer.request_keyframe();
    let (ok, others) = wait_layer_keyframe(&mut pump, &p, Layer::Low).await;
    assert!(
        ok,
        "시청자 PLI가 하 트랙 요청으로 가지 않았습니다(다른 트랙: {others:?})"
    );
    assert!(
        others.is_empty(),
        "다른 트랙의 키프레임도 요청됨: {others:?}"
    );

    // (5) 다시 작아지면 4초 뒤에 상으로 올린다.
    all.extend(videos(&viewer.drain()));
    a.send(layers_msg([300, 100]));
    let up_from = pump.tick();
    pump.run(&p, 350).await;
    let got = videos(&viewer.drain());
    all.extend_from_slice(&got);
    assert_one_per_tick(&got);
    let first_high = got
        .iter()
        .position(|x| x.1 == Layer::High)
        .unwrap_or_else(|| panic!("상으로 올라가지 않았습니다: {got:?}"));
    assert!(
        got[first_high..].iter().all(|x| x.1 == Layer::High),
        "{got:?}"
    );
    let up_ms = (got[first_high].0 - up_from) * 20;
    eprintln!("[e2e] simulcast 올림: {up_ms} ms 뒤 상");
    assert!(
        (3500..=6000).contains(&up_ms),
        "올림은 4초쯤 뒤여야 합니다({up_ms} ms)"
    );
    // 처음부터 끝까지 한 스트림으로 이어진다.
    assert_one_per_tick(&all);
    let switches = all.windows(2).filter(|w| w[0].1 != w[1].1).count();
    assert_eq!(switches, 2, "트랙은 상→하→상 두 번만 바뀌어야 합니다");
    // 전용 서버는 REMB를 보내지 않는다.
    assert_eq!(p.stats().remb_kbps, None, "{:?}", p.stats());
}

/// D-102: 하가 쉰다(목표 0, 프레임도 없음: 설정 3Mbps 이하, 방송 중 소프트웨어 코덱). 하를 보던 시청자는 바로 상으로
/// 가고(지금 트랙이 사라지면 올림이어도 기다리지 않는다), 새 시청자의 첫 트랙도 상이다. 하가 돌아오면 상이 추정에 맞지
/// 않으니 바로 하로 내려간다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simulcast_viewers_get_high_while_low_is_paused() {
    let t = Target::shared_with(1).await;
    let mut a = Client::connect(&t, "simulcast 방송자(하 쉼)").await;
    let rate = dongsi_net::rate::RateConfig {
        simulcast: true,
        ..TEST_RATE
    };
    let p = a
        .publish_with(info(VideoCodec::H264), None, rate)
        .await
        .unwrap();
    // 상은 추정에 맞지 않는다: 시청자는 하.
    a.send(layers_msg([10_000_000, 100]));
    let mut pump = LayerPump::new();
    pump.run(&p, 10).await;

    let mut v1 = Client::connect(&t, "시청자 1").await;
    let w1 = v1.watch_connected(a.id, None).await;
    let viewer1: &TestViewer = &w1.viewer;

    // (1) 하 시청자(첫 트랙은 추정이 아직 없어 상일 수 있고, 곧 하로 내려간다).
    pump.run(&p, 100).await;
    let got = videos(&viewer1.drain());
    assert!(got.len() >= 30, "비디오가 거의 오지 않았습니다: {got:?}");
    assert_one_per_tick(&got);
    let first_low = got
        .iter()
        .position(|x| x.1 == Layer::Low)
        .unwrap_or_else(|| panic!("하로 내려가지 않았습니다: {got:?}"));
    assert!(
        got[first_low..].iter().all(|x| x.1 == Layer::Low),
        "{got:?}"
    );
    let mut all1 = got[first_low..].to_vec();

    // (2) 하를 쉬게 한다: 하 프레임이 멎고 목표가 0이다. 하를 보던 시청자는 바로 상으로 간다.
    a.send(layers_msg([10_000_000, 0]));
    pump.skip_low = true;
    let up_from = pump.tick();
    pump.run(&p, 75).await;
    let got = videos(&viewer1.drain());
    all1.extend_from_slice(&got);
    assert_one_per_tick(&got);
    let first_high = got
        .iter()
        .position(|x| x.1 == Layer::High)
        .unwrap_or_else(|| panic!("상으로 가지 않았습니다: {got:?}"));
    assert!(
        got[first_high..].iter().all(|x| x.1 == Layer::High),
        "바꾼 뒤 다른 트랙이 섞였습니다: {got:?}"
    );
    let up_ms = (got[first_high].0 - up_from) * 20;
    eprintln!("[e2e] simulcast 하 쉼: {up_ms} ms 뒤 상");
    assert!(
        up_ms <= 1000,
        "하가 쉰 뒤 상으로 바꾸기가 늦었습니다({up_ms} ms)"
    );

    // (3) 새 시청자의 첫 트랙은 상이다(하 프레임은 없다).
    let mut v2 = Client::connect(&t, "시청자 2").await;
    let w2 = v2.watch_connected(a.id, None).await;
    let viewer2: &TestViewer = &w2.viewer;
    pump.run(&p, 100).await;
    let got2 = videos(&viewer2.drain());
    assert!(
        got2.len() >= 20,
        "새 시청자에게 비디오가 오지 않았습니다: {got2:?}"
    );
    assert!(
        got2.iter().all(|x| x.1 == Layer::High),
        "새 시청자가 상이 아닌 트랙을 받았습니다: {got2:?}"
    );
    assert_one_per_tick(&got2);
    all1.extend(videos(&viewer1.drain()));

    // (4) 하가 돌아온다(목표를 다시 알리고 프레임도 보낸다): 상은 추정에 맞지 않으니 바로 하로 내려간다.
    a.send(layers_msg([10_000_000, 100]));
    pump.skip_low = false;
    let down_from = pump.tick();
    pump.run(&p, 75).await;
    let got = videos(&viewer1.drain());
    all1.extend_from_slice(&got);
    assert_one_per_tick(&got);
    let first_low = got
        .iter()
        .position(|x| x.1 == Layer::Low)
        .unwrap_or_else(|| panic!("하로 돌아오지 않았습니다: {got:?}"));
    let down_ms = (got[first_low].0 - down_from) * 20;
    eprintln!("[e2e] simulcast 하 복귀: {down_ms} ms 뒤 하");
    assert!(
        down_ms <= 1000,
        "하 복귀 뒤 내림이 늦었습니다({down_ms} ms)"
    );
    assert_one_per_tick(&all1);
    let switches = all1.windows(2).filter(|w| w[0].1 != w[1].1).count();
    assert_eq!(switches, 2, "트랙은 하→상→하 두 번만 바뀌어야 합니다");
}
