//! 방송자 생존 판정 시험: SFU는 연결된 방송자가 3초 넘게 아무 패킷도 보내지 않으면 끊긴 것으로 본다
//! (`sfu.rs`의 `PUBLISHER_SILENCE_LIMIT`). 일시적인 끊김과 미디어 없는 방송은 끊기지 않아야 한다.
//!
//! 외부 서버 모드 규칙(main.rs 머리 문서)을 지킨다: 방송 목록은 자기 방송자 ID로만 보고, 방송자는
//! 시험마다 새 설치 ID로 접속하며, 서버 상한 안에서 방송 1개·시청자 2명 이하만 쓴다.

use std::time::{Duration, Instant};

use dongsi_core::VideoCodec;
use dongsi_core::proto::ClientMsg;
use dongsi_core::proto::ServerMsg;
use str0m::media::MediaKind;

use crate::client::{Client, Watching, find};
use crate::media::{Pump, info};
use crate::raw_publisher::{PendingRawPublisher, RawPublisher};
use crate::target::Target;
use crate::{assert_received, ended_for, settle};

/// 시험용 방송자가 페이로드에 넣는 방송자 표시.
const RAW_TAG: u32 = 11;

/// 시험용(str0m 직접) 방송자로 방송을 시작하고 미디어 연결이 될 때까지 기다린다.
async fn publish_raw(c: &mut Client) -> RawPublisher {
    let pending = PendingRawPublisher::new(c.server_ip());
    let req = c.next_req();
    c.send(ClientMsg::Publish {
        req,
        offer: pending.offer.clone(),
        info: info(VideoCodec::H264),
        password: None,
    });
    let answer = c
        .wait_for("Published", |m| match m {
            ServerMsg::Published { req: r, answer } if *r == req => Some(answer.clone()),
            _ => None,
        })
        .await;
    let p = pending.start(&answer, RAW_TAG);
    p.wait_connected(Duration::from_secs(10)).await;
    p
}

/// 시청자가 tick 번호가 `min_tick` 이상인 비디오를 `want`개 받을 때까지 걸린 시간과 받은 가장 큰
/// tick. 제한 시간 안에 못 받으면 `None`. (끊김 동안 잃은 프레임을 재전송으로 뒤늦게 받는 것과, 끊김 뒤에
/// 새로 만든 프레임을 받는 것을 가르려고 tick을 본다.)
async fn time_to_video(
    w: &Watching,
    want: usize,
    within: Duration,
    min_tick: u32,
) -> Option<(Duration, u32)> {
    let start = Instant::now();
    let mut got = 0;
    let mut max_tick = 0;
    while start.elapsed() < within {
        for r in w.viewer.drain() {
            if r.kind == MediaKind::Video
                && let Some(i) = r.index
                && i >= min_tick
            {
                got += 1;
                max_tick = max_tick.max(i);
            }
        }
        if got >= want {
            return Some((start.elapsed(), max_tick));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    None
}

/// 방송자가 잠시(2초) 모든 패킷을 주고받지 못해도(끊김 기준 3초 미만) 방송이 끝나지 않고,
/// 다시 이어지면 시청자가 다시 미디어를 받는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn publisher_brief_silence_keeps_broadcast() {
    let t = Target::shared_with(1).await;
    let mut b = Client::connect(&t, "잠깐 끊기는 방송자").await;
    let id = b.id;
    let p = publish_raw(&mut b).await;
    let mut v = Client::connect(&t, "시청자").await;
    v.wait_list("방송 목록에 내 방송", |l| find(l, id).is_some())
        .await;
    let w = v.watch_connected(id, None).await;
    let (_, last_tick) = time_to_video(&w, 10, Duration::from_secs(10), 0)
        .await
        .expect("끊기 전에 미디어가 오지 않습니다");

    // 방송자의 모든 패킷(보내기·받기)을 2초 동안 버린다.
    let blackout = Duration::from_secs(2);
    p.set_blackout(true);
    tokio::time::sleep(blackout).await;
    p.set_blackout(false);
    let resume_tick = p.tick();
    let dropped = p.dropped();
    eprintln!(
        "[e2e] 끊김 {blackout:?}: 버린 패킷 보냄 {} 받음 {}",
        dropped.sent, dropped.received
    );
    assert!(
        dropped.sent >= 50,
        "끊김이 적용되지 않았습니다(버린 패킷 {})",
        dropped.sent
    );

    // 다시 이어진 뒤에 만든 프레임(끊김이 끝난 시점의 tick 이상)을 시청자가 받는다. 끊김 동안 잃은 프레임을
    // 재전송으로 뒤늦게 받은 것은 세지 않는다.
    let min_tick = resume_tick;
    let resumed = time_to_video(&w, 25, Duration::from_secs(10), min_tick).await;
    eprintln!(
        "[e2e] 끊기 전 마지막 tick {last_tick}, 끊김이 끝난 tick {min_tick}, 그 뒤 만든 비디오 25개 받기까지: {resumed:?}"
    );
    assert!(
        resumed.is_some(),
        "방송자가 다시 이어졌는데 시청자가 10초 안에 새 미디어를 받지 못했습니다"
    );

    // 끊김 시작부터 6초가 넘도록 방송이 끝나지 않았다(끊김 기준 3초를 넘긴 시각이다).
    let ended = v.try_wait_for(Duration::from_secs(3), ended_for(id)).await;
    assert_eq!(
        ended,
        Err("시간 초과"),
        "잠깐 끊겼을 뿐인데 방송이 끝났습니다"
    );
    assert!(
        find(&v.list, id).is_some(),
        "방송 목록에서 방송이 사라졌습니다: {:?}",
        v.list
    );
    assert!(!p.is_dead(), "시험용 방송자 연결이 끊어졌습니다");
}

/// 방송자가 끊기고 돌아오지 않으면 3초 안팎에 방송이 끝난다(`Publisher::close`처럼 닫지는 않았다).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn publisher_long_silence_ends_broadcast() {
    let t = Target::shared_with(1).await;
    let mut b = Client::connect(&t, "끊어진 방송자").await;
    let id = b.id;
    let p = publish_raw(&mut b).await;
    let mut v = Client::connect(&t, "시청자").await;
    v.wait_list("방송 목록에 내 방송", |l| find(l, id).is_some())
        .await;
    let w = v.watch_connected(id, None).await;
    assert!(
        time_to_video(&w, 10, Duration::from_secs(10), 0)
            .await
            .is_some(),
        "끊기 전에 미디어가 오지 않습니다"
    );

    let start = Instant::now();
    p.set_blackout(true);
    let ended = v.try_wait_for(Duration::from_secs(6), ended_for(id)).await;
    let secs = start.elapsed().as_secs_f64();
    eprintln!(
        "[e2e] 방송자 완전 끊김 → BroadcastEnded: {}",
        match &ended {
            Ok(()) => format!("{secs:.1}초"),
            Err(e) => format!("{e} ({secs:.1}초 기다림)"),
        }
    );
    assert!(
        ended.is_ok(),
        "방송자가 끊겼는데 6초 안에 BroadcastEnded가 오지 않았습니다"
    );
    assert!(
        secs >= 2.5,
        "끊김 기준(3초)보다 일찍 방송이 끝났습니다: {secs:.2}초"
    );
    v.wait_list("목록에서 사라짐", |l| find(l, id).is_none())
        .await;
}

/// 방송을 켜 놓고 미디어를 보내지 않아도(화면이 멈췄거나 아직 시작 전) 방송이 끝나지 않고,
/// 그 뒤에 시작한 시청·미디어가 된다. 실제 앱과 같은 송출(`dongsi_net`의 `Publisher`)을 쓴다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_publisher_without_media_stays() {
    let t = Target::shared_with(1).await;
    let mut b = Client::connect(&t, "미디어 없는 방송자").await;
    let id = b.id;
    let p = b.publish(info(VideoCodec::H264), None).await.unwrap();
    let mut v = Client::connect(&t, "시청자").await;
    v.wait_list("방송 목록에 내 방송", |l| find(l, id).is_some())
        .await;
    let w = v.watch_connected(id, None).await;

    // 미디어 없이 6초(끊김 기준 3초의 두 배).
    let ended = v.try_wait_for(Duration::from_secs(6), ended_for(id)).await;
    assert_eq!(ended, Err("시간 초과"), "미디어가 없다고 방송이 끝났습니다");
    assert!(
        find(&v.list, id).is_some(),
        "방송 목록에서 방송이 사라졌습니다: {:?}",
        v.list
    );
    assert!(
        p.stats().connected && !p.is_closed(),
        "송출이 끊어졌습니다: {:?}",
        p.stats()
    );

    // 그 뒤에 시청자가 하나 더 들어오고 미디어를 보내면 둘 다 받는다.
    let mut v2 = Client::connect(&t, "늦은 시청자").await;
    v2.wait_list("방송 목록에 내 방송", |l| find(l, id).is_some())
        .await;
    let w2 = v2.watch_connected(id, None).await;
    let sent = Pump::new(5).h264(&p, 75).await;
    settle().await;
    assert_received("처음부터 있던 시청자", &w.viewer.drain(), sent, 5);
    assert_received("늦게 들어온 시청자", &w2.viewer.drain(), sent, 5);
}

/// 측정: 서버가 사라지면(내부 서버를 닫고 다시 띄움) 송출(`Publisher`)이 언제 알아채는지. 방송자 쪽
/// full ICE 타임아웃은 고치지 않고 재기만 한다(내부 서버 전용, 22초쯤 걸려서 기본 실행에서는 뺀다).
/// `cargo test -p dongsi-server --features full-tests --test e2e publisher_notices_server_loss -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "측정용: 22초쯤 걸린다"]
async fn publisher_notices_server_loss() {
    let Some(mut t) = Target::internal_only(
        "publisher_notices_server_loss",
        crate::target::default_config(),
    )
    .await
    else {
        return;
    };
    let mut b = Client::connect(&t, "서버를 잃는 방송자").await;
    let p = b.publish(info(VideoCodec::H264), None).await.unwrap();
    let sent = Pump::new(6).h264(&p, 25).await;
    assert!(sent > 0);
    let start = Instant::now();
    t.restart().await;
    let deadline = start + Duration::from_secs(60);
    while !p.is_closed() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let secs = start.elapsed().as_secs_f64();
    eprintln!(
        "[e2e] 서버가 사라진 뒤 송출이 알아채기까지: {secs:.1}초 ({:?})",
        p.stats().closed
    );
    assert!(
        p.is_closed(),
        "서버가 사라졌는데 60초가 지나도 송출이 알아채지 못했습니다"
    );
}
