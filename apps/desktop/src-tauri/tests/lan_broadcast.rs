//! 로컬 랜 방송 통합 시험: Tauri 없이, 앱이 쓰는 코드(`hub`, `embedded`, `session`, `viewing`, `store`)로
//! 방송자 앱이 내장 서버를 켜고 같은 네트워크의 다른 앱이 로컬 감지로 찾아 시청하는 흐름을 끝까지 확인한다.
//!
//! - 내장 서버 포트는 8200~8299(시험마다 다른 번호). 7884는 쓰지 않는다. 포트가 쓰이고 있으면 내장 서버가
//!   빈 포트로 대신하므로(`fallback_port`) 감지·접속은 항상 `embedded.port()`를 쓴다.
//! - 실제 UPnP는 부르지 않는다(`FakeMapper`). 사용자 화면·창은 캡처하지 않는다(합성 프레임, 테스트 패턴).
//! - 감지 응답은 출발지별 초당 10회(질의 단위)로 제한되고 훑기 하나가 질의를 최대 3번 보내므로, 감지는
//!   한 번에 300ms씩 차례로 한다.
//! - 이 시험을 돌리면 Windows 방화벽이 새 실행 파일의 수신을 물을 수 있다.

mod support;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::Ordering;
use std::time::Duration;

use dongsi_core::proto::{ClientMsg, ErrorCode, ServerMsg};
use dongsi_core::{Fingerprint, Host, Invite, ServerAddr, VideoCodec};
use dongsi_desktop_lib::broadcast::Broadcaster;
use dongsi_desktop_lib::embedded::{
    EmbeddedParams, HostingKind, embedded_broadcast_invite, local_addrs, start_embedded,
};
use dongsi_desktop_lib::hub::{HOST_KEY, HubEvent, Target};
use dongsi_desktop_lib::session::connect_publisher;
use dongsi_desktop_lib::viewing::{self, LocalServerView};
use dongsi_net::discovery::Via;
use dongsi_net::ifaces::{IfaceKind, ipv4_interfaces, primary_lan_ip};
use dongsi_net::publish::PendingPublisher;
use dongsi_net::signal;
use dongsi_net::tls::Identity;
use support::*;

const HOST: &str = "방송자";
const SETTLE: Duration = Duration::from_millis(3500);

fn json(e: &dongsi_desktop_lib::error::CmdError) -> serde_json::Value {
    serde_json::to_value(e).unwrap()
}

/// 방송이 보이는 서버 하나를 감지로 찾는다(앱의 기본 대상에서 Tailscale만 뺀 것).
async fn discover_one(app: &TestApp, port: u16) -> LocalServerView {
    let targets = default_targets(port).await;
    let mut views = app
        .discover_until(&targets, "방송이 있는 서버", |v| {
            v.iter().any(|s| !s.broadcasts.is_empty())
        })
        .await;
    assert_eq!(views.len(), 1, "{views:?}");
    views.remove(0)
}

/// 서버 하나가 없어질 때까지(방송이 끝나 감지에 안 나올 때까지) 기다리는 대신, 지금 감지에 안 나오는지.
async fn discover_none(app: &TestApp, targets: &[(SocketAddr, Via)]) -> bool {
    app.discover(targets, Duration::from_millis(300))
        .await
        .is_empty()
}

// ---------------------------------------------------------------------------
// 1. 기본 로컬 랜 방송

/// 방송자 하나와 시청자 `n`명: 감지 → 접속 → 시청 → 프레임·A/V 동기·키프레임 요청·시청자 수.
async fn broadcast_to_viewers(port: u16, n: usize) -> (Lan, Vec<(TestApp, Watching)>) {
    let lan = Lan::start(HOST, LanOptions::new(port), unused_mapper()).await;
    let port = lan.port();
    let host_id = lan.id();
    let fingerprint = lan.embedded.fingerprint();
    assert!(
        lan.mapper.calls.lock().unwrap().is_empty(),
        "lan은 UPnP를 쓰지 않는다"
    );

    // 방송자 자신의 감지 결과에는 자기 방송이 없다.
    let own = lan
        .app
        .discover(&default_targets(port).await, Duration::from_millis(300))
        .await;
    assert!(own.is_empty(), "내 방송이 목록에 보입니다: {own:?}");
    // 방송자 자신은 내 방송을 시청하지 못한다(D-065: 이 PC에서 소리를 틀면 루프백이 다시 담아 에코가 쌓인다).
    // 서버에 Watch를 보내기 전에 거부하므로 시청자 수에도 들어가지 않는다(아래 시청자 수 확인).
    let own_live = viewing::server_live(&lan.app.state, &lan.conn);
    assert!(
        own_live.broadcasts.iter().any(|b| b.mine),
        "내 방송 표시가 없습니다"
    );
    let err = lan
        .app
        .watch(HOST_KEY, host_id, None, false)
        .await
        .err()
        .expect("내 방송 시청이 받아들여졌습니다");
    assert_eq!(json(&err)["code"], "ownBroadcast", "{:?}", json(&err));

    let mut watchers = Vec::new();
    for i in 0..n {
        let app = TestApp::new(&format!("시청자{i}"));
        let server = discover_one(&app, port).await;
        assert!(server.key.starts_with("local-"), "{}", server.key);
        assert_eq!(server.name, HOST);
        assert!(server.host_only);
        assert!(!server.saved);
        assert!(["lan", "local"].contains(&server.via), "via {}", server.via);
        assert_eq!(server.broadcasts.len(), 1);
        let b = &server.broadcasts[0];
        assert_eq!(b.broadcaster_id, host_id.to_string());
        assert_eq!(b.broadcaster_name, HOST);
        assert_eq!(b.codec, VideoCodec::H264);
        assert_eq!((b.width, b.height, b.fps), (1280, 720, 25));
        assert!(!b.locked);

        // 지문으로 TLS를 고정한 키로 접속한다.
        let target = app
            .hub
            .registered_target(&server.key)
            .expect("대상이 등록되지 않았습니다");
        assert_eq!(target.fingerprint, Some(fingerprint));
        let conn = app.hub.connect(&server.key).await.unwrap();
        let live = viewing::server_live(&app.state, &conn);
        assert!(!live.can_publish && live.host_only);
        assert_eq!(live.fingerprint.full, fingerprint.to_hex_colon());
        assert_eq!(live.server_name, HOST);
        assert_eq!(live.broadcasts.len(), 1);
        assert_eq!(live.broadcasts[0].broadcaster_name, HOST);
        assert!(!live.broadcasts[0].mine && !live.broadcasts[0].locked);

        clear_keyframe_request(&lan.publisher);
        let mark = lan.app.events.mark();
        let w = app.watch(&server.key, host_id, None, false).await.unwrap();
        assert!(!w.result.servers_changed && w.result.answer.saved_server_id.is_none());
        let want = (i + 1) as u32;
        lan.app
            .events
            .wait(mark, "방송자 쪽 시청자 수", |e| {
                matches!(e, HubEvent::ViewerCount { key, count } if key == HOST_KEY && *count == want)
            })
            .await;
        eventually("시청자가 붙을 때마다 키프레임 요청", || {
            lan.publisher.take_keyframe_request()
        })
        .await;
        watchers.push((app, w));
    }
    assert_eq!(lan.conn.viewer_count.load(Ordering::Relaxed), n as u32);
    (lan, watchers)
}

fn check_all(watchers: &[(TestApp, Watching)], min_video: usize, min_audio: usize) {
    let mut worst = 0f64;
    for (i, (_, w)) in watchers.iter().enumerate() {
        let received = w.viewer.drain();
        assert_stream_ok(&format!("시청자{i}"), &received, min_video, min_audio);
        worst = worst.max(av_summary(&av_diffs(&received)).1);
    }
    eprintln!(
        "[전체] 시청자 {}명, A/V 최대 차이 {worst:.2}ms",
        watchers.len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lan_broadcast_reaches_three_viewers_found_by_discovery() {
    let (lan, watchers) = broadcast_to_viewers(8201, 3).await;
    tokio::time::sleep(SETTLE).await;
    check_all(&watchers, 50, 100);
    drop(watchers);
    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_lan_broadcast_with_ten_viewers_and_the_eleventh_is_rejected() {
    let (lan, watchers) = broadcast_to_viewers(8202, 10).await;
    let port = lan.port();
    let host_id = lan.id();
    tokio::time::sleep(SETTLE).await;
    check_all(&watchers, 50, 100);

    // 11번째는 거부된다(서버 ServerFull).
    let eleventh = TestApp::new("시청자11");
    let server = discover_one(&eleventh, port).await;
    let err = eleventh
        .watch(&server.key, host_id, None, false)
        .await
        .err()
        .expect("11번째 시청자가 받아들여졌습니다");
    assert!(json(&err).get("code").is_none(), "{:?}", json(&err));
    assert!(err.to_string().contains("가득"), "{err}");
    // 이미 보던 시청자는 영향이 없다.
    assert_eq!(lan.embedded.server().stats().broadcasts, 1);
    tokio::time::sleep(Duration::from_millis(500)).await;
    for (i, (_, w)) in watchers.iter().enumerate() {
        assert!(
            !video(&w.viewer.drain()).is_empty(),
            "시청자{i}의 영상이 끊겼습니다"
        );
    }
    drop(watchers);
    lan.stop().await;
}

// ---------------------------------------------------------------------------
// 2. 감지 경로

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_over_loopback_reports_local_and_the_server_can_be_watched() {
    let lan = Lan::start(HOST, LanOptions::new(8210), unused_mapper()).await;
    let port = lan.port();
    let app = TestApp::new("시청자");
    let views = app
        .discover_until(&loopback_targets(port), "루프백 감지", |v| {
            !v.is_empty()
        })
        .await;
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].via, "local");
    assert_eq!(views[0].addr, format!("127.0.0.1:{port}"));
    let w = app
        .watch(&views[0].key, lan.id(), None, false)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_stream_ok("루프백 시청", &w.viewer.drain(), 30, 60);
    drop(w);
    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_over_lan_unicast_reports_lan_and_the_server_can_be_watched() {
    let Some(lan_ip) = primary_lan_ip() else {
        eprintln!("[건너뜀] LAN 인터페이스가 없습니다");
        return;
    };
    let lan = Lan::start(HOST, LanOptions::new(8211), unused_mapper()).await;
    let port = lan.port();
    let app = TestApp::new("시청자");
    let targets = [(SocketAddr::new(IpAddr::V4(lan_ip), port), Via::Lan)];
    let views = app
        .discover_until(&targets, "LAN 유니캐스트 감지", |v| !v.is_empty())
        .await;
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].via, "lan");
    assert_eq!(views[0].addr, format!("{lan_ip}:{port}"));
    let w = app
        .watch(&views[0].key, lan.id(), None, false)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_stream_ok("LAN 시청", &w.viewer.drain(), 30, 60);
    drop(w);
    lan.stop().await;
}

/// 인터페이스별 서브넷 브로드캐스트는 환경(공유기·가상 어댑터)에 따라 이 PC에서 응답이 안 올 수 있어
/// 결과를 출력만 한다. 응답이 오면 경로는 `lan`이어야 한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_over_subnet_broadcast_reports_lan_when_it_answers() {
    let lan = Lan::start(HOST, LanOptions::new(8212), unused_mapper()).await;
    let port = lan.port();
    let app = TestApp::new("시청자");
    let broadcasts: Vec<_> = default_targets(port)
        .await
        .into_iter()
        .filter(|(_, via)| *via == Via::Lan)
        .collect();
    if broadcasts.is_empty() {
        eprintln!("[건너뜀] 브로드캐스트 주소가 있는 LAN 인터페이스가 없습니다");
        return;
    }
    let mut answered = Vec::new();
    for target in &broadcasts {
        let mut found = Vec::new();
        for _ in 0..3 {
            found = app
                .discover(std::slice::from_ref(target), Duration::from_millis(300))
                .await;
            if !found.is_empty() {
                break;
            }
        }
        match found.first() {
            Some(v) => {
                eprintln!(
                    "[브로드캐스트] {} → 응답 있음(via {}, addr {})",
                    target.0, v.via, v.addr
                );
                assert_eq!(v.via, "lan", "{}", target.0);
                answered.push(target.0);
            }
            None => eprintln!("[브로드캐스트] {} → 응답 없음", target.0),
        }
    }
    eprintln!(
        "[브로드캐스트] 대상 {}개 중 응답 {}개",
        broadcasts.len(),
        answered.len()
    );
    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn own_broadcast_and_servers_without_a_broadcast_are_not_listed() {
    let lan = Lan::start(HOST, LanOptions::new(8213), unused_mapper()).await;
    let port = lan.port();
    // 방송이 없는 서버(방송자 B가 서버만 켠 상태)
    let idle_app = TestApp::new("대기자");
    let idle = start_embedded(
        &idle_app.hub,
        EmbeddedParams {
            dir: &idle_app.host_dir(),
            name: "빈 서버",
            hosting: HostingKind::Lan,
            port: 8214,
            lan_ip: primary_lan_ip(),
        },
        &*unused_mapper(),
    )
    .await
    .unwrap();
    let mut targets = loopback_targets(port);
    targets.extend(loopback_targets(idle.port()));

    // 다른 사람: 방송이 있는 서버만 보인다.
    let viewer = TestApp::new("시청자");
    let views = viewer
        .discover_until(&targets, "방송이 있는 서버", |v| !v.is_empty())
        .await;
    assert_eq!(views.len(), 1, "{views:?}");
    assert_eq!(views[0].name, HOST);
    // 방송자 자신: 내 방송은 빠진다.
    assert!(discover_none(&lan.app, &targets).await);
    // 방송이 없는 서버의 주인도 마찬가지로 내 서버를 못 본다가 아니라, 다른 사람의 방송은 본다.
    let seen = idle_app
        .discover_until(&targets, "다른 사람의 방송", |v| !v.is_empty())
        .await;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].broadcasts[0].broadcaster_id, lan.id().to_string());
    idle.teardown(&idle_app.hub).await;
    lan.stop().await;
}

// ---------------------------------------------------------------------------
// 3. 초대 코드

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invite_with_password_leads_to_the_pinned_server_and_a_remembered_password() {
    let lan = Lan::start(
        HOST,
        LanOptions::new(8220).password("열쇠"),
        unused_mapper(),
    )
    .await;
    let port = lan.port();
    let ifaces = ipv4_interfaces();
    let (code, without) =
        embedded_broadcast_invite(&lan.embedded, &ifaces, HOST, lan.id(), Some("열쇠")).unwrap();
    let invite = Invite::decode(&code).unwrap();
    assert_eq!(invite.fingerprint(), &lan.embedded.fingerprint());
    assert_eq!(invite.broadcaster(), Some(&lan.id()));
    assert_eq!(invite.password(), Some("열쇠"));
    assert_eq!(invite.name(), Some(HOST));
    assert_eq!(invite.addrs(), local_addrs(&ifaces, port).as_slice());
    if let Some(ip) = primary_lan_ip()
        && ifaces
            .iter()
            .any(|i| i.ip == ip && i.kind == IfaceKind::Lan)
    {
        assert!(
            invite
                .addrs()
                .contains(&ServerAddr::new(Host::Ip(IpAddr::V4(ip)), port)),
            "초대 코드에 이 PC의 LAN 주소가 없습니다: {:?}",
            invite.addrs()
        );
    }
    // 비밀번호를 뺀 코드
    let stripped =
        Invite::decode(&without.expect("비밀번호 있는 코드는 뺀 코드도 만든다")).unwrap();
    assert_eq!(stripped.password(), None);
    assert_eq!(stripped.broadcaster(), Some(&lan.id()));
    assert_eq!(stripped.addrs(), invite.addrs());

    // 시청자: 서버 목록에 넣고(저장한 서버 ID 키) 접속·시청한다. 비밀번호는 초대 코드에서 기억했다.
    let app = TestApp::new("시청자");
    let id = app
        .state
        .lock()
        .update(|s| Ok(s.servers.add_invite(&invite)))
        .unwrap()
        .id();
    let key = id.to_string();
    let conn = app.hub.connect(&key).await.unwrap();
    assert_eq!(conn.fingerprint, lan.embedded.fingerprint());
    let live = viewing::server_live(&app.state, &conn);
    assert!(live.broadcasts[0].locked && live.broadcasts[0].password_saved);
    let w = app.watch(&key, lan.id(), None, false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_stream_ok("초대 코드 시청", &w.viewer.drain(), 30, 60);

    // 비밀번호를 뺀 코드로 추가한 시청자는 비밀번호를 물어야 한다.
    let other = TestApp::new("시청자2");
    let id2 = other
        .state
        .lock()
        .update(|s| Ok(s.servers.add_invite(&stripped)))
        .unwrap()
        .id();
    let err = other
        .watch(&id2.to_string(), lan.id(), None, false)
        .await
        .err()
        .unwrap();
    assert_eq!(json(&err)["code"], "passwordRequired");
    drop(w);
    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invite_without_password_has_no_password_and_is_watchable() {
    let lan = Lan::start(HOST, LanOptions::new(8221), unused_mapper()).await;
    let (code, without) =
        embedded_broadcast_invite(&lan.embedded, &ipv4_interfaces(), HOST, lan.id(), None).unwrap();
    assert!(without.is_none());
    let invite = Invite::decode(&code).unwrap();
    assert_eq!(invite.password(), None);
    assert_eq!(invite.broadcaster(), Some(&lan.id()));
    assert_eq!(invite.fingerprint(), &lan.embedded.fingerprint());
    // 코드 본문(JSON)에도 `p`가 없다.
    let body = code.strip_prefix(dongsi_core::invite::PREFIX).unwrap();
    let json_bytes = base64url_decode(body);
    let value: serde_json::Value = serde_json::from_slice(&json_bytes).unwrap();
    assert!(value.get("p").is_none(), "{value}");
    assert_eq!(value["b"], lan.id().to_string());

    let app = TestApp::new("시청자");
    let id = app
        .state
        .lock()
        .update(|s| Ok(s.servers.add_invite(&invite)))
        .unwrap()
        .id();
    let w = app
        .watch(&id.to_string(), lan.id(), None, false)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2000)).await;
    assert!(!video(&w.viewer.drain()).is_empty());
    drop(w);
    lan.stop().await;
}

/// base64url(패딩 없음) 해독(시험용으로 짧게).
fn base64url_decode(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => panic!("base64url이 아닙니다"),
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invite_with_a_different_fingerprint_is_refused_with_detail() {
    let lan = Lan::start(HOST, LanOptions::new(8222), unused_mapper()).await;
    let port = lan.port();
    let wrong: Fingerprint = Identity::generate().unwrap().fingerprint();
    let invite = Invite::for_broadcast(
        vec![ServerAddr::new(
            Host::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            port,
        )],
        wrong,
        Some(HOST.into()),
        lan.id(),
        None,
    )
    .unwrap();
    let app = TestApp::new("시청자");
    let id = app
        .state
        .lock()
        .update(|s| Ok(s.servers.add_invite(&invite)))
        .unwrap()
        .id();
    let err = app
        .hub
        .connect(&id.to_string())
        .await
        .err()
        .expect("지문이 달라도 접속되었습니다");
    let e = json(&err);
    assert_eq!(e["code"], "fingerprintMismatch");
    assert_eq!(e["detail"]["stored"]["full"], wrong.to_hex_colon());
    assert_eq!(
        e["detail"]["actual"]["full"],
        lan.embedded.fingerprint().to_hex_colon()
    );
    assert_eq!(
        e["detail"]["actual"]["short"],
        lan.embedded.fingerprint().short()
    );
    assert!(app.hub.connected_keys().is_empty());
    lan.stop().await;
}

// ---------------------------------------------------------------------------
// 4. 방송 비밀번호

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn broadcast_without_password_is_watched_immediately() {
    let lan = Lan::start(HOST, LanOptions::new(8230), unused_mapper()).await;
    let app = TestApp::new("시청자");
    let server = discover_one(&app, lan.port()).await;
    assert!(!server.broadcasts[0].locked);
    let w = app.watch(&server.key, lan.id(), None, false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(2000)).await;
    assert!(!video(&w.viewer.drain()).is_empty());
    drop(w);
    lan.stop().await;
}

/// 저장한 서버에서: 비밀번호 없음·틀림·맞음(기억 안 함/기억함)·기억한 값으로 시청·방송자가 바꾼 뒤.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn password_flow_on_a_saved_server_required_wrong_right_remembered_and_changed() {
    let port = 8231;
    let lan = Lan::start(HOST, LanOptions::new(port).password("A"), unused_mapper()).await;
    let host_id = lan.id();
    let (_, without) =
        embedded_broadcast_invite(&lan.embedded, &ipv4_interfaces(), HOST, host_id, Some("A"))
            .unwrap();
    let app = TestApp::new("시청자");
    let invite = Invite::decode(&without.unwrap()).unwrap();
    let id = app
        .state
        .lock()
        .update(|s| Ok(s.servers.add_invite(&invite)))
        .unwrap()
        .id();
    let key = id.to_string();
    let remembered = || {
        app.state
            .lock()
            .state()
            .servers
            .password_for(id, &host_id)
            .map(str::to_owned)
    };

    // 1) 비밀번호 없이
    let err = app.watch(&key, host_id, None, false).await.err().unwrap();
    assert_eq!(json(&err)["code"], "passwordRequired");
    assert_eq!(err.to_string(), "비밀번호가 필요한 방송입니다");
    // 2) 틀림
    let err = app
        .watch(&key, host_id, Some("틀림"), true)
        .await
        .err()
        .unwrap();
    assert_eq!(json(&err)["code"], "wrongPassword");
    assert_eq!(remembered(), None, "틀린 비밀번호를 기억했습니다");
    // 3) 맞음 + 기억 안 함
    let w = app.watch(&key, host_id, Some("A"), false).await.unwrap();
    assert!(!w.result.servers_changed);
    assert_eq!(remembered(), None);
    drop(w);
    // 4) 맞음 + 기억
    let w = app.watch(&key, host_id, Some("A"), true).await.unwrap();
    assert!(w.result.servers_changed);
    assert_eq!(
        w.result.answer.saved_server_id, None,
        "저장한 서버 키에서는 새로 저장하지 않는다"
    );
    assert_eq!(remembered().as_deref(), Some("A"));
    drop(w);
    // 5) 기억한 비밀번호로 비밀번호 없이
    let w = app.watch(&key, host_id, None, false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!video(&w.viewer.drain()).is_empty());
    drop(w);

    // 6) 방송자가 비밀번호를 바꿔 다시 방송한다(같은 앱·인증서·포트).
    let host_app = lan.stop().await;
    let lan2 = Lan::start_on(
        host_app,
        LanOptions::new(port).password("B"),
        unused_mapper(),
    )
    .await;
    assert_eq!(
        lan2.port(),
        port,
        "같은 포트로 다시 열려야 저장한 주소가 맞습니다"
    );
    let err = app.watch(&key, host_id, None, false).await.err().unwrap();
    assert_eq!(json(&err)["code"], "passwordRequired");
    assert_eq!(
        err.to_string(),
        "비밀번호가 바뀌었습니다. 다시 입력해 주세요."
    );
    assert_eq!(remembered(), None, "틀린 기억은 지워야 합니다");
    // 7) 새 비밀번호
    let w = app.watch(&key, host_id, Some("B"), true).await.unwrap();
    assert_eq!(remembered().as_deref(), Some("B"));
    drop(w);
    lan2.stop().await;
}

async fn locked_lan(port: u16) -> (Lan, TestApp, LocalServerView) {
    let lan = Lan::start(
        HOST,
        LanOptions::new(port).password("열쇠"),
        unused_mapper(),
    )
    .await;
    let app = TestApp::new("시청자");
    let server = discover_one(&app, lan.port()).await;
    assert!(server.broadcasts[0].locked);
    assert!(
        !server.broadcasts[0].password_saved,
        "처음 감지에는 기억한 비밀번호가 없다"
    );
    assert!(server.key.starts_with("local-"));
    (lan, app, server)
}

/// 로컬 감지 키: 비밀번호 없음·틀림은 저장하지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_key_password_required_and_wrong_do_not_save_anything() {
    let (lan, app, server) = locked_lan(8232).await;
    let before = app.state.lock().state().clone();
    let err = app
        .watch(&server.key, lan.id(), None, true)
        .await
        .err()
        .unwrap();
    assert_eq!(json(&err)["code"], "passwordRequired");
    let err = app
        .watch(&server.key, lan.id(), Some("틀림"), true)
        .await
        .err()
        .unwrap();
    assert_eq!(json(&err)["code"], "wrongPassword");
    assert_eq!(*app.state.lock().state(), before);
    lan.stop().await;
}

/// D-057: local- 키에서 맞는 비밀번호 + 기억 → 서버를 저장하고 비밀번호를 기억한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_key_right_password_with_remember_saves_the_server_and_the_password() {
    let (lan, app, server) = locked_lan(8233).await;
    let host_id = lan.id();
    let fingerprint = lan.embedded.fingerprint();
    let detected: ServerAddr = server.addr.parse().unwrap();

    let w = app
        .watch(&server.key, host_id, Some("열쇠"), true)
        .await
        .unwrap();
    assert!(w.result.servers_changed);
    let saved_id = w
        .result
        .answer
        .saved_server_id
        .clone()
        .expect("savedServerId가 없습니다");
    {
        let store = app.state.lock();
        let servers: Vec<_> = store.state().servers.iter().collect();
        assert_eq!(servers.len(), 1);
        let s = servers[0];
        assert_eq!(s.id.to_string(), saved_id);
        assert_eq!(s.name, HOST, "이름은 서버 이름");
        assert_eq!(s.addrs, vec![detected.clone()], "주소는 감지된 주소");
        assert_eq!(s.fingerprint, Some(fingerprint), "지문 고정");
        assert_eq!(s.passwords.get(&host_id).map(String::as_str), Some("열쇠"));
    }
    // 디스크에도 저장됐다.
    let reopened = dongsi_desktop_lib::store::Store::open(&app.dir.0).unwrap();
    assert_eq!(reopened.state().servers.len(), 1);
    drop(reopened);
    // 열린 시청·연결은 local- 키 그대로 계속된다(키를 바꿔 끼우지 않는다).
    assert_eq!(app.hub.connected_keys(), vec![server.key.clone()]);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!video(&w.viewer.drain()).is_empty());

    // 다시 감지하면 저장한 서버 ID 키, saved: true.
    let again = discover_one(&app, lan.port()).await;
    assert_eq!(again.key, saved_id);
    assert!(again.saved);
    assert!(again.broadcasts[0].locked);
    assert!(
        again.broadcasts[0].password_saved,
        "저장한 서버에 비밀번호를 기억했으니 감지 타일도 '비밀번호 기억됨'"
    );
    // 그 키로 비밀번호 없이 시청된다(기억한 비밀번호).
    let w2 = app.watch(&again.key, host_id, None, false).await.unwrap();
    assert!(w2.result.answer.saved_server_id.is_none());
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!video(&w2.viewer.drain()).is_empty());
    let live = viewing::server_live(&app.state, &app.hub.connect(&saved_id).await.unwrap());
    assert!(live.broadcasts[0].password_saved);
    drop((w, w2));
    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_key_right_password_without_remember_changes_nothing() {
    let (lan, app, server) = locked_lan(8234).await;
    let before = app.state.lock().state().clone();
    let w = app
        .watch(&server.key, lan.id(), Some("열쇠"), false)
        .await
        .unwrap();
    assert!(!w.result.servers_changed);
    assert!(w.result.answer.saved_server_id.is_none());
    assert_eq!(*app.state.lock().state(), before, "저장하면 안 됩니다");
    drop(w);
    // 다음에도 local- 키이고 비밀번호를 다시 물어야 한다(메모리에도 기억하지 않는다).
    let again = discover_one(&app, lan.port()).await;
    assert_eq!(again.key, server.key);
    assert!(!again.saved);
    assert!(!again.broadcasts[0].password_saved);
    let err = app
        .watch(&again.key, lan.id(), None, false)
        .await
        .err()
        .unwrap();
    assert_eq!(json(&err)["code"], "passwordRequired");
    lan.stop().await;
}

/// 같은 지문의 저장한 서버가 이미 있으면 새로 만들지 않고 그 서버에 기억한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_key_remember_with_an_existing_saved_server_reuses_it() {
    let (lan, app, server) = locked_lan(8235).await;
    let host_id = lan.id();
    // 감지한 뒤(local- 키가 등록된 뒤) 같은 서버를 초대 코드로 저장해 둔다(이름은 사용자가 정한 것).
    let invite = Invite::for_server(
        vec![ServerAddr::new(
            Host::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            lan.port(),
        )],
        lan.embedded.fingerprint(),
        Some("내가 붙인 이름".into()),
    )
    .unwrap();
    let existing = app
        .state
        .lock()
        .update(|s| Ok(s.servers.add_invite(&invite)))
        .unwrap()
        .id();

    let w = app
        .watch(&server.key, host_id, Some("열쇠"), true)
        .await
        .unwrap();
    assert!(w.result.servers_changed);
    assert_eq!(w.result.answer.saved_server_id, Some(existing.to_string()));
    {
        let store = app.state.lock();
        assert_eq!(store.state().servers.len(), 1, "새 항목을 만들면 안 됩니다");
        let s = store.state().servers.get(existing).unwrap();
        assert_eq!(s.name, "내가 붙인 이름");
        assert_eq!(s.passwords.get(&host_id).map(String::as_str), Some("열쇠"));
    }
    drop(w);
    lan.stop().await;
}

/// (a) 저장한 서버를 감지로 찾으면, 저장된 주소가 낡았어도 감지된 주소로 접속한다. 저장 목록은 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saved_server_found_by_discovery_connects_to_the_detected_address() {
    let lan = Lan::start(HOST, LanOptions::new(8236), unused_mapper()).await;
    let port = lan.port();
    let stale = ServerAddr::new(Host::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST)), 9); // 아무도 안 듣는 포트
    let invite = Invite::for_server(
        vec![stale.clone()],
        lan.embedded.fingerprint(),
        Some("저장".into()),
    )
    .unwrap();
    let app = TestApp::new("시청자");
    let id = app
        .state
        .lock()
        .update(|s| Ok(s.servers.add_invite(&invite)))
        .unwrap()
        .id();
    let key = id.to_string();
    // 감지 전에는 낡은 주소라 접속하지 못한다.
    assert!(app.hub.connect(&key).await.is_err());

    let views = app
        .discover_until(&loopback_targets(port), "저장한 서버 감지", |v| {
            !v.is_empty()
        })
        .await;
    assert_eq!(views[0].key, key);
    assert!(views[0].saved);
    let state_before = app.state.lock().state().clone();
    // 감지된 주소(루프백:port)가 먼저, 저장된 낡은 주소는 뒤라서 접속된다.
    let conn = app
        .hub
        .connect(&key)
        .await
        .expect("감지된 주소로 접속하지 못했습니다");
    assert_eq!(conn.fingerprint, lan.embedded.fingerprint());
    // state.json(저장 목록)은 바뀌지 않았다. TOFU·지문 고정·saved 정보도 그대로다.
    assert_eq!(*app.state.lock().state(), state_before);
    let s = app.state.lock().state().servers.get(id).unwrap().clone();
    assert_eq!(s.addrs, vec![stale]);
    assert_eq!(s.fingerprint, Some(lan.embedded.fingerprint()));
    let w = app.watch(&key, lan.id(), None, false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!video(&w.viewer.drain()).is_empty());
    drop(w);

    // 서버를 삭제하면 메모리에 둔 감지 주소도 지운다(앱의 remove_server와 같은 순서).
    app.hub
        .disconnect(&key, dongsi_desktop_lib::hub::REASON_REMOVED);
    app.state
        .lock()
        .update(|s| Ok(s.servers.remove(id).map(drop)?))
        .unwrap();
    app.hub.remove_target(&key);
    assert!(
        app.hub.connect(&key).await.is_err(),
        "삭제한 서버에 접속되면 안 됩니다"
    );
    lan.stop().await;
}

// ---------------------------------------------------------------------------
// 5. 호스트가 아닌 접속자의 방송 거부

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connections_without_the_host_token_cannot_publish() {
    let lan = Lan::start(HOST, LanOptions::new(8240), unused_mapper()).await;
    let port = lan.port();
    let app = TestApp::new("시청자");
    let server = discover_one(&app, port).await;
    let info = publish_info(VideoCodec::H264, 1280, 720, 25);
    // 토큰 없음(감지 키)과 틀린 토큰
    app.hub.set_target(
        "wrong-token",
        Target {
            addrs: vec![ServerAddr::new(
                Host::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                port,
            )],
            fingerprint: Some(lan.embedded.fingerprint()),
            saved: None,
            host_token: Some("틀린 토큰".into()),
        },
    );
    for key in [server.key.as_str(), "wrong-token"] {
        let conn = app.hub.connect(key).await.unwrap();
        assert!(!conn.can_publish, "{key}");
        assert!(conn.host_only);
        // 송출 함수는 Publish를 보내지 않고 실패한다.
        let err = connect_publisher(&conn, info, None).await.err().unwrap();
        assert_eq!(err.to_string(), "이 서버에서는 방송할 수 없습니다");
        // 억지로 Publish를 보내도 서버가 거부한다.
        let offer = PendingPublisher::new(conn.connected_ip, false)
            .unwrap()
            .offer;
        let reply = conn
            .request(|req| ClientMsg::Publish {
                req,
                offer,
                info,
                password: None,
            })
            .await
            .unwrap();
        match reply {
            ServerMsg::Error { code, .. } => assert_eq!(code, ErrorCode::NotAllowed, "{key}"),
            other => panic!("거부되지 않았습니다: {other:?}"),
        }
    }
    // 방송은 호스트의 것 하나뿐이다.
    assert_eq!(lan.embedded.server().stats().broadcasts, 1);
    let live = viewing::server_live(&app.state, &app.hub.connect(&server.key).await.unwrap());
    assert_eq!(live.broadcasts.len(), 1);
    assert_eq!(live.broadcasts[0].broadcaster_id, lan.id().to_string());
    lan.stop().await;
}

// ---------------------------------------------------------------------------
// 6. 방송 종료

async fn end_broadcast(hosting: HostingKind, port: u16, router: Router) {
    let mapper = FakeMapper::new(router);
    let lan = Lan::start(HOST, LanOptions::new(port).hosting(hosting), mapper.clone()).await;
    let port = lan.port();
    let host_id = lan.id();
    let mut watchers = Vec::new();
    for i in 0..2 {
        let app = TestApp::new(&format!("시청자{i}"));
        let server = discover_one(&app, port).await;
        let w = app.watch(&server.key, host_id, None, false).await.unwrap();
        watchers.push((app, server.key, w));
    }
    tokio::time::sleep(Duration::from_millis(800)).await;
    let marks: Vec<usize> = watchers.iter().map(|(a, ..)| a.events.mark()).collect();
    let host_mark = lan.app.events.mark();
    assert_eq!(mapper.removed.load(Ordering::SeqCst), 0);

    let host_app = lan.stop().await;

    for ((app, key, _), mark) in watchers.iter().zip(&marks) {
        // 방송 종료 → 서버가 닫힘 순서로 온다.
        let ended = app
            .events
            .wait(*mark, "BroadcastEnded", |e| {
                matches!(e, HubEvent::BroadcastEnded { key: k, broadcaster_id } if k == key && *broadcaster_id == host_id.to_string())
            })
            .await;
        let closed = app
            .events
            .wait(*mark, "ServerClosed", |e| {
                matches!(e, HubEvent::ServerClosed { .. })
            })
            .await;
        assert!(
            ended < closed,
            "BroadcastEnded가 ServerClosed보다 먼저여야 합니다"
        );
        assert_eq!(
            app.events.all()[closed],
            HubEvent::ServerClosed {
                key: key.clone(),
                reason: "서버가 닫혔습니다".into()
            }
        );
        let n = app
            .events
            .count_after(*mark, Duration::from_millis(800), |e| {
                matches!(e, HubEvent::ServerClosed { .. })
            })
            .await;
        assert_eq!(n, 1, "ServerClosed는 한 번만 와야 합니다");
        assert!(app.hub.connected_keys().is_empty());
    }
    // 방송자 쪽 host 연결: 방송을 끝냈다는 이유로 한 번.
    let host_closed: Vec<_> = host_app
        .events
        .since(host_mark)
        .into_iter()
        .filter(|e| matches!(e, HubEvent::ServerClosed { .. }))
        .collect();
    assert_eq!(
        host_closed,
        vec![HubEvent::ServerClosed {
            key: HOST_KEY.into(),
            reason: "방송을 끝냈습니다".into()
        }]
    );
    assert!(
        host_app.hub.registered_target(HOST_KEY).is_none(),
        "host 대상이 남았습니다"
    );
    assert!(host_app.hub.connect(HOST_KEY).await.is_err());
    // 내장 서버가 닫혔다: 새 접속이 안 되고 감지에도 응답하지 않는다.
    let addr: ServerAddr = format!("127.0.0.1:{port}").parse().unwrap();
    assert!(
        signal::connect(&addr, None).await.is_err(),
        "닫힌 서버에 접속되었습니다"
    );
    let probe = TestApp::new("확인");
    assert!(discover_none(&probe, &loopback_targets(port)).await);
    // UPnP 매핑은 direct일 때만 지운다.
    let removed = mapper.removed.load(Ordering::SeqCst);
    assert_eq!(removed, usize::from(hosting == HostingKind::Direct));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ending_a_lan_broadcast_closes_viewers_host_connection_and_server() {
    end_broadcast(
        HostingKind::Lan,
        8250,
        Router::Missing("쓰이지 않음".into()),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ending_a_direct_broadcast_also_removes_the_upnp_mapping() {
    let router = Router::Maps {
        ip: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)),
        port: None,
    };
    end_broadcast(HostingKind::Direct, 8251, router).await;
}

// ---------------------------------------------------------------------------
// 7. 방송 중 화질·코덱 변경

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changing_quality_and_codec_updates_viewers_without_reconnecting() {
    let lan = Lan::start(HOST, LanOptions::new(8260), unused_mapper()).await;
    let app = TestApp::new("시청자");
    let server = discover_one(&app, lan.port()).await;
    let w = app.watch(&server.key, lan.id(), None, false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let conn = app
        .hub
        .get(&server.key)
        .await
        .expect("연결이 열려 있어야 합니다");

    let mark = app.events.mark();
    let new_info = publish_info(VideoCodec::Av1, 1920, 1080, 60);
    assert!(lan.change(new_info));
    assert!(
        !lan.change(new_info),
        "같은 값이면 서버에 다시 알리지 않는다"
    );
    app.events
        .wait(
            mark,
            "ServerChanged",
            |e| matches!(e, HubEvent::ServerChanged { key } if *key == server.key),
        )
        .await;
    eventually("방송 목록이 새 값", || {
        conn.broadcasts()
            .first()
            .is_some_and(|b| b.codec == VideoCodec::Av1)
    })
    .await;
    let b = conn.broadcasts()[0].clone();
    assert_eq!(
        (b.codec, b.width, b.height, b.fps),
        (VideoCodec::Av1, 1920, 1080, 60)
    );
    let list = viewing::server_live(&app.state, &conn);
    assert_eq!(
        (list.broadcasts[0].width, list.broadcasts[0].fps),
        (1920, 60)
    );

    tokio::time::sleep(Duration::from_millis(2500)).await;
    let received = w.viewer.drain();
    let v = video(&received);
    let first_av1 = v
        .iter()
        .position(|r| r.codec == str0m::format::Codec::Av1)
        .expect("새 코덱 프레임이 오지 않았습니다");
    assert!(first_av1 > 0, "바꾸기 전 프레임(H.264)이 있어야 합니다");
    assert!(
        v[..first_av1]
            .iter()
            .all(|r| r.codec == str0m::format::Codec::H264)
    );
    assert!(
        v[first_av1..]
            .iter()
            .all(|r| r.codec == str0m::format::Codec::Av1),
        "바뀐 뒤에 옛 코덱이 섞였습니다"
    );
    assert!(
        v.len() - first_av1 >= 25,
        "새 코덱 프레임이 적습니다: {}",
        v.len() - first_av1
    );
    assert!(received.iter().all(|r| r.index.is_some()));
    eprintln!(
        "[코덱 변경] H.264 {}개 → AV1 {}개",
        first_av1,
        v.len() - first_av1
    );
    // 시청 연결은 그대로다.
    assert!(!conn.is_closed());
    assert!(
        app.events
            .all()
            .iter()
            .all(|e| !matches!(e, HubEvent::ServerClosed { .. }))
    );
    drop(w);
    lan.stop().await;
}

// ---------------------------------------------------------------------------
// 8. 시청자 나감·다시 들어옴

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn viewer_can_leave_and_come_back_and_hub_disconnect_removes_the_viewer() {
    let lan = Lan::start(HOST, LanOptions::new(8270), unused_mapper()).await;
    let port = lan.port();
    let host_id = lan.id();
    let a = TestApp::new("시청자A");
    let b = TestApp::new("시청자B");
    let key_a = discover_one(&a, port).await.key;
    let key_b = discover_one(&b, port).await.key;
    let wa = a.watch(&key_a, host_id, None, false).await.unwrap();
    let wb = b.watch(&key_b, host_id, None, false).await.unwrap();
    eventually("시청자 2명", || {
        lan.conn.viewer_count.load(Ordering::Relaxed) == 2
    })
    .await;
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // StopWatch → 시청자 수 감소, 그 수신자에 프레임이 더 오지 않는다.
    let mark = lan.app.events.mark();
    a.hub
        .get(&key_a)
        .await
        .unwrap()
        .send(ClientMsg::StopWatch {
            session: wa.session,
        })
        .unwrap();
    lan.app
        .events
        .wait(mark, "시청자 수 1", |e| {
            matches!(e, HubEvent::ViewerCount { count: 1, .. })
        })
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = wa.viewer.drain();
    let _ = wb.viewer.drain();
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(
        video(&wa.viewer.drain()).is_empty(),
        "나간 시청자에게 프레임이 옵니다"
    );
    assert!(
        video(&wb.viewer.drain()).len() >= 15,
        "남은 시청자의 프레임이 끊겼습니다"
    );
    drop(wa);

    // 같은 시청자가 새 offer로 다시 시청 → 프레임이 다시 오고 키프레임 요청도 다시 켜진다.
    clear_keyframe_request(&lan.publisher);
    let mark = lan.app.events.mark();
    let wa = a.watch(&key_a, host_id, None, false).await.unwrap();
    lan.app
        .events
        .wait(mark, "시청자 수 2", |e| {
            matches!(e, HubEvent::ViewerCount { count: 2, .. })
        })
        .await;
    eventually("다시 붙을 때 키프레임 요청", || {
        lan.publisher.take_keyframe_request()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_stream_ok("다시 들어온 시청자", &wa.viewer.drain(), 30, 60);

    // Hub 끊기(disconnect_server)로 나간 시청자: 시청자 수 감소, 다시 접속·시청 가능.
    let mark_b = b.events.mark();
    let mark = lan.app.events.mark();
    Broadcaster::default()
        .disconnect_unless_broadcasting(&b.hub, &key_b)
        .unwrap();
    b.events
        .wait(mark_b, "ServerClosed(끊기)", |e| {
            *e == HubEvent::ServerClosed {
                key: key_b.clone(),
                reason: "연결을 끊었습니다".into(),
            }
        })
        .await;
    lan.app
        .events
        .wait(mark, "시청자 수 감소", |e| {
            matches!(e, HubEvent::ViewerCount { count: 1, .. })
        })
        .await;
    drop(wb);
    let mark = lan.app.events.mark();
    let wb = b.watch(&key_b, host_id, None, false).await.unwrap();
    lan.app
        .events
        .wait(mark, "시청자 수 2", |e| {
            matches!(e, HubEvent::ViewerCount { count: 2, .. })
        })
        .await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_stream_ok("끊고 다시 들어온 시청자", &wb.viewer.drain(), 30, 60);
    drop((wa, wb));
    lan.stop().await;
}

// ---------------------------------------------------------------------------
// 9. PC 직접 호스팅(UPnP 실패·이중 NAT)

async fn viewer_gets_frames(lan: &Lan) {
    let app = TestApp::new("시청자");
    let server = discover_one(&app, lan.port()).await;
    let w = app.watch(&server.key, lan.id(), None, false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(2000)).await;
    assert!(
        video(&w.viewer.drain()).len() >= 20,
        "LAN으로 시청되지 않았습니다"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_hosting_without_a_router_still_broadcasts_and_explains_the_failure() {
    let mapper = FakeMapper::new(Router::Missing(
        "UPnP 공유기를 찾지 못했습니다: 시험".into(),
    ));
    let lan_ip = Ipv4Addr::new(192, 168, 77, 5);
    let mut options = LanOptions::new(8280).hosting(HostingKind::Direct);
    options.lan_ip = Some(lan_ip);
    let lan = Lan::start(HOST, options, mapper.clone()).await;
    let port = lan.port();
    assert_eq!(*mapper.calls.lock().unwrap(), vec![(lan_ip, port)]);
    let view = lan
        .embedded
        .upnp_view()
        .expect("direct는 UPnP 상태가 있어야 합니다");
    let error = view.error.clone().expect("안내 문구가 없습니다");
    assert!(
        error.contains("UPnP 공유기를 찾지 못했습니다: 시험"),
        "{error}"
    );
    assert!(
        error.contains(&port.to_string()) && error.contains("192.168.77.5"),
        "{error}"
    );
    assert_eq!(view.external, None);
    assert!(!view.double_nat);
    // 초대 코드에는 외부 주소가 없다(LAN 주소만).
    let (code, _) =
        embedded_broadcast_invite(&lan.embedded, &ipv4_interfaces(), HOST, lan.id(), None).unwrap();
    let invite = Invite::decode(&code).unwrap();
    assert_eq!(
        invite.addrs(),
        local_addrs(&ipv4_interfaces(), port).as_slice()
    );
    viewer_gets_frames(&lan).await;
    lan.stop().await;
    assert_eq!(
        mapper.removed.load(Ordering::SeqCst),
        0,
        "열지 못한 매핑은 지울 것이 없다"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_hosting_without_a_lan_address_says_so() {
    let mapper = unused_mapper();
    let mut options = LanOptions::new(8281).hosting(HostingKind::Direct);
    options.lan_ip = None;
    let lan = Lan::start(HOST, options, mapper.clone()).await;
    assert!(
        mapper.calls.lock().unwrap().is_empty(),
        "LAN 주소가 없으면 UPnP를 시도하지 않는다"
    );
    let view = lan.embedded.upnp_view().unwrap();
    assert_eq!(view.error.as_deref(), Some("LAN 주소를 찾지 못했습니다"));
    assert_eq!(view.external, None);
    viewer_gets_frames(&lan).await;
    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_hosting_behind_double_nat_is_flagged_and_the_invite_leads_with_the_external_address()
 {
    // 매핑은 됐지만 외부 IP가 사설 주소이고 외부 포트가 다르다.
    let mapper = FakeMapper::new(Router::Maps {
        ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
        port: Some(48884),
    });
    let lan_ip = Ipv4Addr::new(192, 168, 77, 5);
    let mut options = LanOptions::new(8282).hosting(HostingKind::Direct);
    options.lan_ip = Some(lan_ip);
    let lan = Lan::start(HOST, options, mapper.clone()).await;
    let port = lan.port();
    assert_eq!(*mapper.calls.lock().unwrap(), vec![(lan_ip, port)]);
    let view = lan.embedded.upnp_view().unwrap();
    assert_eq!(view.external.as_deref(), Some("10.0.0.2:48884"));
    assert!(view.double_nat, "사설 외부 IP는 이중 NAT로 표시해야 합니다");
    assert_eq!(view.error, None);
    let json = serde_json::to_value(&view).unwrap();
    assert_eq!(json["doubleNat"], true);
    assert_eq!(json["external"], "10.0.0.2:48884");
    // 초대 코드의 첫 주소가 외부 주소다.
    let (code, _) =
        embedded_broadcast_invite(&lan.embedded, &ipv4_interfaces(), HOST, lan.id(), None).unwrap();
    let invite = Invite::decode(&code).unwrap();
    assert_eq!(
        invite.addrs()[0],
        ServerAddr::new(Host::Ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))), 48884)
    );
    assert!(invite.addrs().len() <= 6);
    // LAN으로는 그대로 시청된다.
    viewer_gets_frames(&lan).await;
    assert_eq!(mapper.removed.load(Ordering::SeqCst), 0);
    lan.stop().await;
    assert_eq!(
        mapper.removed.load(Ordering::SeqCst),
        1,
        "끝낼 때 매핑을 지워야 합니다"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_hosting_with_a_public_address_is_not_double_nat() {
    let mapper = FakeMapper::new(Router::Maps {
        ip: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)),
        port: None,
    });
    let lan = Lan::start(
        HOST,
        LanOptions::new(8283).hosting(HostingKind::Direct),
        mapper.clone(),
    )
    .await;
    let port = lan.port();
    let view = lan.embedded.upnp_view().unwrap();
    assert_eq!(view.external, Some(format!("203.0.113.7:{port}")));
    assert!(!view.double_nat);
    assert_eq!(view.error, None);
    lan.stop().await;
    assert_eq!(mapper.removed.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lan_hosting_never_touches_the_router_and_has_no_upnp_status() {
    let mapper = unused_mapper();
    let lan = Lan::start(HOST, LanOptions::new(8284), mapper.clone()).await;
    assert!(mapper.calls.lock().unwrap().is_empty());
    assert!(lan.embedded.upnp_view().is_none());
    lan.stop().await;
    assert_eq!(mapper.removed.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------
// 9-1. 포트 대체 표시(T4-F): 요청한 포트를 못 써서 다른 포트로 열렸는지. 포트는 8480~8489(T4 대역).

/// 내장 서버만 열고(방송 없이) 요청 포트와 실제 포트를 돌려준다.
async fn open_embedded_on(port: u16) -> (TestApp, dongsi_desktop_lib::embedded::Embedded) {
    let app = TestApp::new(HOST);
    let embedded = start_embedded(
        &app.hub,
        EmbeddedParams {
            dir: &app.host_dir(),
            name: &app.name,
            hosting: HostingKind::Lan,
            port,
            lan_ip: None,
        },
        &*unused_mapper(),
    )
    .await
    .expect("내장 서버를 열지 못했습니다");
    (app, embedded)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn free_requested_port_is_used_as_is_and_not_a_fallback() {
    let (app, embedded) = open_embedded_on(8480).await;
    assert_eq!(embedded.requested_port(), 8480);
    assert_eq!(embedded.port(), 8480);
    assert!(!embedded.port_fallback());
    embedded.teardown(&app.hub).await;
}

/// TCP 포트를 다른 소켓이 잡고 있으면 빈 포트로 대신 열고 `port_fallback`이 true다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requested_port_with_tcp_in_use_falls_back_to_another_port() {
    let blocker = std::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 8481)).unwrap();
    let (app, embedded) = open_embedded_on(8481).await;
    assert_eq!(embedded.requested_port(), 8481);
    assert_ne!(embedded.port(), 8481);
    assert!(embedded.port_fallback());
    embedded.teardown(&app.hub).await;
    drop(blocker);
}

/// UDP만 쓰이고 있어도(TCP는 비어 있어도) 대신 연다. 서버는 두 포트가 같은 번호여야 하기 때문이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requested_port_with_only_udp_in_use_also_falls_back() {
    let blocker = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 8482)).unwrap();
    let (app, embedded) = open_embedded_on(8482).await;
    assert_ne!(embedded.port(), 8482);
    assert!(embedded.port_fallback());
    embedded.teardown(&app.hub).await;
    drop(blocker);
}

// ---------------------------------------------------------------------------
// 10. 실제 미디어 파이프라인(테스트 패턴 + 테스트 톤, 앱과 같은 방식)

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_real_pipeline_test_pattern_and_tone_reach_two_viewers() {
    use std::sync::{Arc, Mutex};

    use dongsi_core::VideoSettings;
    use dongsi_media::{
        AudioPipeline, AudioSource, EncodedAudio, EncodedVideo, SourceId, VideoPipeline,
    };

    let mut options = LanOptions::new(8290);
    options.feed = false;
    // 방송 정보는 앱과 같이 소스 크기와 설정에서 구한다.
    let settings = VideoSettings {
        width: 1280,
        height: 720,
        fps: 30,
        bitrate_kbps: 2_500,
        ..VideoSettings::DEFAULT
    }
    .clamped();
    let sources = tokio::task::spawn_blocking(dongsi_media::list_sources)
        .await
        .unwrap()
        .unwrap();
    let source = sources
        .into_iter()
        .find(|s| s.id == SourceId::TestPattern)
        .expect("테스트 패턴이 없습니다");
    let (width, height) = settings.output_size(source.width, source.height);
    options.info = publish_info(settings.codec, width, height, settings.fps);
    let lan = Lan::start(HOST, options, unused_mapper()).await;

    // 캡처·인코딩 → 송출 연결(콜백이 송출 연결을 보고 보낸다).
    let slot: Arc<Mutex<Option<Arc<dongsi_net::publish::Publisher>>>> = Arc::default();
    *slot.lock().unwrap() = Some(lan.publisher.clone());
    let video = {
        let slot = slot.clone();
        tokio::task::spawn_blocking(move || {
            VideoPipeline::start(
                SourceId::TestPattern,
                settings,
                Box::new(move |frame: EncodedVideo| {
                    if let Some(p) = slot.lock().unwrap().as_ref() {
                        p.send_video(frame.codec, frame.data, frame.capture);
                    }
                }),
            )
        })
        .await
        .unwrap()
        .expect("비디오 파이프라인을 시작하지 못했습니다")
    };
    let video = Arc::new(video);
    let audio = {
        let slot = slot.clone();
        tokio::task::spawn_blocking(move || {
            AudioPipeline::start(
                AudioSource::TestTone,
                128,
                Box::new(move |packet: EncodedAudio| {
                    if let Some(p) = slot.lock().unwrap().as_ref() {
                        p.send_audio(packet.data, packet.capture, packet.sample_index);
                    }
                }),
            )
        })
        .await
        .unwrap()
        .expect("오디오 파이프라인을 시작하지 못했습니다")
    };
    // 감시 스레드가 하는 일 중 키프레임 요청 전달.
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let monitor = {
        let (publisher, video, stop) = (lan.publisher.clone(), video.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if publisher.take_keyframe_request() {
                    video.request_keyframe();
                }
                std::thread::sleep(Duration::from_millis(15));
            }
        })
    };

    let mut watchers = Vec::new();
    for i in 0..2 {
        let app = TestApp::new(&format!("시청자{i}"));
        let server = discover_one(&app, lan.port()).await;
        assert_eq!(
            (server.broadcasts[0].width, server.broadcasts[0].height),
            (width, height)
        );
        let w = app.watch(&server.key, lan.id(), None, false).await.unwrap();
        watchers.push((app, w));
    }
    tokio::time::sleep(Duration::from_millis(4000)).await;
    for (i, (_, w)) in watchers.iter().enumerate() {
        let received = w.viewer.drain();
        let v = video_of(&received);
        let a = audio_of(&received);
        eprintln!(
            "[실제 파이프라인] 시청자{i}: 비디오 {}개, 오디오 {}개",
            v.len(),
            a.len()
        );
        assert!(
            v.len() >= 40,
            "시청자{i}: 비디오 프레임이 적습니다: {}",
            v.len()
        );
        assert!(
            a.len() >= 100,
            "시청자{i}: 오디오 패킷이 적습니다: {}",
            a.len()
        );
        assert!(v.iter().all(|r| r.codec == str0m::format::Codec::H264));
    }

    // 정리: 앱과 같은 순서(감시 스레드·오디오·비디오 정지 → 송출 연결 닫기 → 서버 닫기).
    stop.store(true, Ordering::Relaxed);
    let _ = monitor.join();
    let _ = tokio::task::spawn_blocking(move || {
        audio.stop();
        drop(video);
    })
    .await;
    *slot.lock().unwrap() = None;
    drop(watchers);
    lan.stop().await;
}

fn video_of(r: &[Received]) -> Vec<&Received> {
    video(r)
}

fn audio_of(r: &[Received]) -> Vec<&Received> {
    audio(r)
}
