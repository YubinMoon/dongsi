//! 지표(D-083): 실제 송출·시청을 돌리면서 내부 서버의 지표가 접속·방송·시청 수와 UDP 양을 따라가는지 본다.
//! 지표는 내부 서버에서만 읽을 수 있다(외부 모드에서는 건너뜀).

use std::time::{Duration, Instant};

use dongsi_core::InstallId;
use dongsi_core::proto::{ClientMsg, ErrorCode};
use dongsi_server::Metrics;

use crate::client::{Client, find};
use crate::media::{Pump, info};
use crate::target::{Target, default_config};
use crate::viewer::video_count;

/// 텍스트에서 `series`(이름과 라벨)의 값.
fn value(text: &str, series: &str) -> Option<u64> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .find_map(|l| l.strip_prefix(series)?.strip_prefix(' ')?.parse().ok())
}

/// 지표가 `expected`와 모두 같아질 때까지(최대 10초) 다시 읽고 마지막 텍스트를 돌려준다.
async fn wait_values(m: &Metrics, what: &str, expected: &[(&str, u64)]) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let text = m.render();
        let wrong: Vec<String> = expected
            .iter()
            .filter(|(s, v)| value(&text, s) != Some(*v))
            .map(|(s, v)| format!("{s} = {:?} (기대 {v})", value(&text, s)))
            .collect();
        if wrong.is_empty() {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: 지표가 맞지 않습니다: {}",
            wrong.join(", ")
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metrics_follow_users_and_traffic() {
    let Some(t) = Target::internal_only("metrics_follow_users_and_traffic", default_config()).await
    else {
        return;
    };
    let m = t.metrics().unwrap();
    wait_values(
        &m,
        "시작",
        &[
            ("dongsi_connections", 0),
            ("dongsi_broadcasts", 0),
            ("dongsi_connections_total", 0),
        ],
    )
    .await;

    let mut b = Client::connect(&t, "지표 방송자").await;
    let id = b.id;
    let publisher = b.publish(info(), None).await.expect("방송 시작 실패");
    let mut pump = Pump::new(31);
    pump.h264(&publisher, 10).await;

    let mut v = Client::connect(&t, "지표 시청자").await;
    v.wait_list("방송 목록에 내 방송", |l| find(l, id).is_some())
        .await;
    // 없는 방송: 오류 응답을 코드별로 센다.
    assert_eq!(
        v.watch(InstallId::from_bytes([0xEE; 16]), None).await.err(),
        Some(ErrorCode::NotFound)
    );
    let w = v.watch_connected(id, None).await;
    let sent = pump.h264(&publisher, 50).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let got = video_count(&w.viewer.drain());
    assert!(got > 0, "시청자가 비디오를 받지 못했습니다(보낸 {sent}개)");

    let text = wait_values(
        &m,
        "방송·시청 중",
        &[
            ("dongsi_connections", 2),
            ("dongsi_broadcasts", 1),
            ("dongsi_viewers", 1),
            ("dongsi_webrtc_sessions{role=\"publisher\"}", 1),
            ("dongsi_webrtc_sessions{role=\"viewer\"}", 1),
            ("dongsi_connections_total", 2),
            ("dongsi_broadcasts_started_total", 1),
            ("dongsi_watch_sessions_started_total", 1),
            ("dongsi_request_errors_total{code=\"not_found\"}", 1),
            ("dongsi_request_errors_total{code=\"wrong_password\"}", 0),
        ],
    )
    .await;
    let positive = |series: &str| {
        let n = value(&text, series).unwrap_or_else(|| panic!("{series}가 없습니다"));
        assert!(n > 0, "{series}가 0입니다");
        n
    };
    // 방송자가 올린 미디어, 시청자에게 전달한 미디어, 시청자가 보낸 RTCP·STUN
    let ingress = positive("dongsi_udp_received_bytes_total{kind=\"publisher\"}");
    let egress = positive("dongsi_udp_sent_bytes_total{kind=\"viewer\"}");
    positive("dongsi_udp_received_packets_total{kind=\"publisher\"}");
    positive("dongsi_udp_sent_packets_total{kind=\"viewer\"}");
    positive("dongsi_udp_received_bytes_total{kind=\"viewer\"}");
    positive("dongsi_udp_sent_bytes_total{kind=\"publisher\"}");
    positive("dongsi_signaling_received_bytes_total");
    positive("dongsi_signaling_sent_bytes_total");
    eprintln!("[e2e] 지표: 받은 미디어 {ingress}바이트, 시청자에게 보낸 {egress}바이트");

    // 끝나면 상태 지표는 0으로 돌아가고 누적 지표는 남는다.
    drop(w);
    b.send(ClientMsg::Unpublish);
    publisher.close();
    v.close();
    b.close();
    wait_values(
        &m,
        "모두 나감",
        &[
            ("dongsi_connections", 0),
            ("dongsi_broadcasts", 0),
            ("dongsi_viewers", 0),
            ("dongsi_webrtc_sessions{role=\"publisher\"}", 0),
            ("dongsi_webrtc_sessions{role=\"viewer\"}", 0),
            ("dongsi_connections_total", 2),
            ("dongsi_broadcasts_started_total", 1),
        ],
    )
    .await;
}
