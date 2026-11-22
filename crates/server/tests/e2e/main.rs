//! 서버 E2E 시험: 실제 서버를 띄우고 송출(dongsi-net) → SFU → 시청(str0m)을 이어 본다.
//!
//! 같은 시험이 두 가지 대상에서 돈다. 시험 함수는 `Target`(target.rs)만 보고 서버를 구분하지 않는다.
//!
//! # 실행
//!
//! 전체 시험에만 든다(AGENTS.md "빌드·테스트"). 기본 `cargo test --workspace`는 이 시험을 빌드하지 않고,
//! `full-tests` 기능을 켜야 돈다(`cargo test-full` 또는 아래 명령).
//!
//! - 내부 서버(기본): 시험마다 `dongsi_server::start`(포트 0)로 자기 서버를 띄운다. 서로 간섭하지 않는다.
//!   `cargo test -p dongsi-server --features full-tests --test e2e -- --nocapture`
//!   시험 몇 개만: `... --test e2e -- passwords discovery`(이름 일부), 목록은 `... --test e2e -- --list`
//! - 외부 서버: 이미 떠 있는 서버(Windows·WSL·Docker 컨테이너 등)에 접속해서 시험한다.
//!   `DONGSI_E2E_INVITE=dongsi1:... cargo test -p dongsi-server --features full-tests --test e2e -- --nocapture`
//!   `DONGSI_E2E_SERVER=127.0.0.1:8370 DONGSI_E2E_FINGERPRINT=<지문> cargo test -p dongsi-server --features full-tests --test e2e`
//!
//! # 환경 변수
//!
//! | 환경 변수 | 뜻 |
//! |---|---|
//! | (없음) | 내부 서버: 시험마다 자기 서버를 띄운다 |
//! | `DONGSI_E2E_SERVER` | 외부 서버 주소 `host:port`(`ServerAddr` 형식) |
//! | `DONGSI_E2E_INVITE` | 외부 서버 초대 코드 `dongsi1:...`. 주소(`DONGSI_E2E_SERVER`가 없을 때)와 지문을 여기서 얻는다 |
//! | `DONGSI_E2E_FINGERPRINT` | 지문(base64url). 우선순위: 이 값 > 초대 코드의 지문 > TOFU(첫 접속에서 받은 지문을 기억해 그 뒤로 고정) |
//! | `DONGSI_E2E_DESTRUCTIVE=1` | 외부 서버에서도 파괴적 시험(`rate_limit`, `server_restart`)을 돌린다. 없으면 외부 모드에서 `[e2e] 건너뜀: …`을 출력하고 통과 |
//! | `DONGSI_E2E_RESTART_CMD` | 외부 서버를 재시작하는 셸 명령(Windows는 `cmd /C`, unix는 `sh -c`). 서버를 정상 종료(SIGTERM/SIGINT)시키고 같은 데이터 폴더로 다시 띄워야 한다. 명령이 서버가 다시 뜨기 전에 끝날 수 있으므로 시험은 30초까지 다시 접속을 시도한다. 없으면 외부 `server_restart`는 건너뜀 |
//! | `DONGSI_E2E_BROADCAST_SLOTS` | 외부 서버에서 시험들이 동시에 쓸 방송 수(기본 8). 서버의 동시 방송 상한(기본 16)보다 작아야 하고, 너무 크면 Windows→WSL 전달 구간에서 미디어가 새 시험이 간헐적으로 실패한다 |
//! | `DONGSI_E2E_DISCOVERY=off` | 외부 모드에서 감지 시험을 건너뜀 |
//! | `DONGSI_E2E_DISCOVERY_PORT` | 외부 모드 감지 응답에서 기대하는 시그널링 포트. 없으면 대상 포트. 서버는 실제로 듣는 포트를 알리므로, 포트를 다른 번호로 매핑했으면(예: 호스트 8340 → 컨테이너 8341) 컨테이너 포트를 준다 |
//!
//! # 외부 서버를 같이 쓰는 규칙
//!
//! - 시험은 병렬로 돈다. 방송 목록은 자기 방송자 ID로 걸러 보고, 목록이 비었다거나 전체 통계 같은
//!   전역 상태는 내부 서버에서만 단정한다.
//! - 서버는 방송 비밀번호 실패를 출발지 IP마다 1분에 8번까지만 받는다. 파괴적이지 않은 시험 전체에서
//!   WrongPassword 실패는 `passwords`의 2번뿐이다.
//! - 파괴적 시험(`rate_limit`, `server_restart`)은 다른 시험이 모두 끝난 뒤 혼자 돈다(target.rs의 문).
//!   `rate_limit`은 시작할 때 이 실행의 앞선 실패 기록(1분)이 지워지기를 기다리고(그래야 8번을 채운다),
//!   끝난 뒤에도 자기 실패 기록이 지워지도록 61초를 기다린다. 앞선 *다른* 실행의 실패는 알 수 없어서,
//!   그 경우 `WrongPassword`가 8번보다 적게 나와도(1~8번) 통과시키고 출력으로 알린다.
//! - 서버 상한(기본 동시 방송 16, 방송당 시청자 16) 안에서만 동시에 쓴다. 시험들이 병렬로 도므로, 방송을 여는
//!   시험은 `Target::shared_with(n)`으로 **동시에 열 방송 수**를 시작할 때 알린다. 외부 모드에서는 모든 시험의
//!   방송 수 합이 방송 자리(기본 8, `DONGSI_E2E_BROADCAST_SLOTS`) 안에 들도록 자리를 한꺼번에 잡는다(모자라면
//!   다른 시험이 끝나기를 기다린다. 한꺼번에 잡으므로 서로 기다리다 멈추지 않는다).
//! - 감지 질의를 보내는 시험(`discovery`, `discovery_load::*`)은 외부 모드에서 `Target::discovery_gate()`로 한 번에
//!   하나씩 돌고, 출발지 하나(0.0.0.0)에서 초당 4번 안쪽으로 보낸다. Docker 브리지(docker-proxy)나 WSL의
//!   localhost 전달 뒤에서는 모든 클라이언트가 서버에 한 출발지로 보이고, 127.0.0.x 같은 다른 출발지 주소는
//!   응답이 돌아오지 않기 때문이다. 출발지를 나눠 보내는 것은 내부 서버에서만 한다.
//! - 감지 응답은 패킷 하나가 1200바이트 이하이고, 방송이 많으면 질의 하나에 조각 여러 개로 온다(D-039).
//!   감지 시험은 조각을 모두 모아 합친 목록에서 자기 방송을 찾는다. 서버는 출발지 IP마다 질의를 초당 10번까지만
//!   받으므로(`discovery_rate_limit`은 파괴적 시험), 반복하는 시험은 그 안에서 쓴다.

mod av_sync;
mod client;
mod discovery_load;
mod liveness;
mod media;
mod metrics;
mod raw_publisher;
mod target;
mod thumbnails;
mod viewer;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use client::{Client, Next, find};
use dongsi_core::discovery::{DiscoveryResponse, PartAssembler, decode_part, encode_query};
use dongsi_core::proto::{
    ClientMsg, ErrorCode, MAX_SDP_BYTES, PROTOCOL_VERSION, PublishInfo, ServerMsg,
};
use dongsi_core::{Fingerprint, InstallId, VideoCodec};
use dongsi_net::signal;
use dongsi_net::tls::Identity;
use dongsi_server::ServerConfig;
use media::{Pump, info};
use str0m::format::Codec;
use str0m::media::MediaKind;
use target::{Target, default_config};
use viewer::{Received, audio_count, video_count};

/// 비디오를 이 비율 이상 받아야 통과한다(오디오는 `publish_and_watch`만 본다).
pub(crate) const MIN_RATIO: f64 = 0.8;

fn assert_received(who: &str, rs: &[Received], sent_videos: usize, tag: u32) {
    let video = video_count(rs);
    let min = (sent_videos as f64 * MIN_RATIO) as usize;
    assert!(
        video >= min,
        "{who}: 비디오 {video}개만 받았습니다(보낸 {sent_videos}개 중 {min}개 이상이어야 함)"
    );
    assert!(
        rs.iter().all(|r| r.tag == Some(tag)),
        "{who}: 다른 방송자의 프레임이 섞였습니다(기대한 표시 {tag})"
    );
}

fn ended_for(id: InstallId) -> impl FnMut(&ServerMsg) -> Option<()> {
    move |m| match m {
        ServerMsg::BroadcastEnded { broadcaster } if *broadcaster == id => Some(()),
        _ => None,
    }
}

pub(crate) fn viewer_count(n: u32) -> impl FnMut(&ServerMsg) -> Option<()> {
    move |m| match m {
        ServerMsg::ViewerCount { count } if *count == n => Some(()),
        _ => None,
    }
}

pub(crate) async fn settle() {
    tokio::time::sleep(Duration::from_millis(300)).await;
}

// ---------------------------------------------------------------------------
// 1. 시그널링 핸드셰이크

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signaling_handshake() {
    let t = Target::shared().await;
    let mut c = Client::connect(&t, "핸드셰이크").await;
    assert_eq!(c.welcome.protocol, PROTOCOL_VERSION);
    assert!(c.welcome.can_publish);
    assert!(!c.welcome.host_only);
    assert!(!c.welcome.server_name.is_empty());
    if let Some(name) = t.internal_name() {
        assert_eq!(c.welcome.server_name, name);
    }
    assert_eq!(c.conn.fingerprint, t.fp, "고정한 지문으로 접속해야 합니다");

    // 다른 지문으로는 접속하지 않는다.
    let wrong = Identity::generate().unwrap().fingerprint();
    let err = signal::connect(&t.addr, Some(wrong))
        .await
        .err()
        .expect("지문이 달라도 접속되었습니다");
    match err {
        dongsi_net::NetError::FingerprintMismatch { expected, actual } => {
            assert_eq!(expected, wrong);
            assert_eq!(actual, t.fp);
        }
        other => panic!("FingerprintMismatch가 아닙니다: {other:?}"),
    }

    // TOFU: 지문을 정하지 않으면 서버의 지문이 그대로 나온다.
    let tofu = signal::connect(&t.addr, None)
        .await
        .expect("TOFU 접속 실패");
    assert_eq!(tofu.fingerprint, t.fp);
    drop(tofu);

    // 프로토콜 버전이 다르면 오류를 보내고 연결을 닫는다.
    let mut raw = signal::connect(&t.addr, Some(t.fp)).await.unwrap();
    let ip = signal::connected_ip(&raw);
    raw.sender
        .send(client::hello(
            InstallId::generate(),
            "버전 불일치",
            PROTOCOL_VERSION + 1,
            None,
            ip,
        ))
        .unwrap();
    match tokio::time::timeout(Duration::from_secs(10), raw.receiver.recv()).await {
        Ok(Some(ServerMsg::Error {
            req: None,
            code: ErrorCode::ProtocolMismatch,
            ..
        })) => {}
        other => panic!("ProtocolMismatch 오류가 아닙니다: {other:?}"),
    }
    let closed = tokio::time::timeout(Duration::from_secs(10), raw.receiver.recv())
        .await
        .expect("오류 뒤에 연결이 닫히지 않았습니다(10초)");
    assert!(
        closed.is_none(),
        "오류 뒤에 다른 메시지가 왔습니다: {closed:?}"
    );

    // 첫 메시지가 Hello가 아니면 연결을 닫는다.
    let mut raw = signal::connect(&t.addr, Some(t.fp)).await.unwrap();
    raw.sender.send(ClientMsg::Ping { nonce: 1 }).unwrap();
    let closed = tokio::time::timeout(Duration::from_secs(10), raw.receiver.recv())
        .await
        .expect("Hello가 아닌 첫 메시지 뒤에 연결이 닫히지 않았습니다(10초)");
    assert!(closed.is_none(), "예상 밖 메시지: {closed:?}");

    // 너무 큰 SDP로 방송을 시작하면 BadRequest.
    c.send(ClientMsg::Publish {
        req: 77,
        offer: "a".repeat(MAX_SDP_BYTES + 1),
        info: info(VideoCodec::H264),
        password: None,
    });
    let code = c
        .wait_for("BadRequest", |m| match m {
            ServerMsg::Error {
                req: Some(77),
                code,
                ..
            } => Some(*code),
            _ => None,
        })
        .await;
    assert_eq!(code, ErrorCode::BadRequest);
}

// ---------------------------------------------------------------------------
// 2. 방송 → 시청 → A/V 동기

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn publish_and_watch() {
    let t = Target::shared_with(1).await;
    let password = "비밀";

    let mut b = Client::connect(&t, "방송자").await;
    let id = b.id;
    if t.internal_name().is_some() {
        assert!(
            b.list.is_empty(),
            "새 서버의 방송 목록이 비어 있지 않습니다"
        );
    }
    let publisher = b
        .publish(info(VideoCodec::H264), Some(password))
        .await
        .expect("방송 시작 실패");

    // 같은 사람이 두 번 송출할 수 없다.
    assert_eq!(
        b.publish_err(info(VideoCodec::H264), None).await,
        ErrorCode::AlreadyPublishing
    );

    // 방송자가 미디어를 조금 보낸 뒤에 시청자가 들어온다. 미디어가 아직 없을 때 들어온 시청자의 키프레임
    // 요청은 서버가 기억해 두었다가 방송자의 첫 비디오가 오면 한 번 더 보내고, 키프레임이 올 때까지 1초마다
    // 되풀이한다(`av_sync.rs`의 `keyframe_request_before_first_media`·`keyframe_request_retried_until_keyframe`가
    // 시험한다). 이 시험은 미디어가 이미 흐르는 중에 들어오는 경우다.
    let mut pump = Pump::new(7);
    pump.h264(&publisher, 10).await;

    // 시청자: 방송 목록에 잠긴 방송이 보인다.
    let mut v = Client::connect(&t, "시청자").await;
    v.wait_list("방송 목록에 내 방송", |l| find(l, id).is_some())
        .await;
    let item = find(&v.list, id).unwrap().clone();
    assert_eq!(item.name, "방송자");
    assert!(item.locked);
    assert_eq!(item.codec, VideoCodec::H264);

    // 비밀번호 없이는 볼 수 없다. 틀린 비밀번호 시험은 `passwords`에서 한다(횟수 제한 때문).
    assert_eq!(
        v.watch(id, None).await.err(),
        Some(ErrorCode::PasswordRequired)
    );

    // 맞는 비밀번호
    let w = v.watch(id, Some(password)).await.expect("시청 실패");
    b.wait_for("ViewerCount 1", viewer_count(1)).await;
    w.viewer.wait_connected(Duration::from_secs(10)).await;

    // 시청자가 연결되면 SFU가 방송자에게 키프레임을 요청한다.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !publisher.take_keyframe_request() {
        assert!(Instant::now() < deadline, "키프레임 요청이 오지 않았습니다");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // 미디어: 20ms마다 오디오, 40ms마다 비디오(같은 캡처 시각). 절반은 H.264, 나머지는 AV1.
    let total_ticks: u32 = 200; // 4초
    w.viewer.drain();
    pump.run(
        &publisher,
        total_ticks,
        |tick| {
            if tick < total_ticks / 2 {
                VideoCodec::H264
            } else {
                VideoCodec::Av1
            }
        },
        |tick| {
            if tick == total_ticks / 2 {
                b.send(ClientMsg::UpdatePublish {
                    info: info(VideoCodec::Av1),
                });
            }
        },
    )
    .await;
    settle().await;

    let received = w.viewer.drain();
    let video: Vec<&Received> = received
        .iter()
        .filter(|r| r.kind == MediaKind::Video)
        .collect();
    let audio: Vec<&Received> = received
        .iter()
        .filter(|r| r.kind == MediaKind::Audio)
        .collect();
    eprintln!(
        "[e2e] 받음: 비디오 {}개(SR {}), 오디오 {}개(SR {})",
        video.len(),
        video.iter().filter(|r| r.capture.is_some()).count(),
        audio.len(),
        audio.iter().filter(|r| r.capture.is_some()).count(),
    );
    assert_eq!(video_count(&received), video.len());
    assert_eq!(audio_count(&received), audio.len());
    assert!(
        video.len() >= 80,
        "비디오 프레임이 너무 적습니다: {}",
        video.len()
    );
    assert!(
        audio.len() >= 160,
        "오디오 패킷이 너무 적습니다: {}",
        audio.len()
    );

    // 코덱 전환: H.264 다음에 AV1이 온다.
    assert!(video.iter().any(|r| r.codec == Codec::H264));
    assert!(video.iter().any(|r| r.codec == Codec::Av1));
    let first_av1 = video.iter().position(|r| r.codec == Codec::Av1).unwrap();
    // 외부 서버 모드에서는 시그널링(TCP)의 방송 정보 갱신과 미디어(UDP)가 시청자에게 닿는 순서가 부하에 따라
    // 전환 직전후로 어긋날 수 있다. 전환 뒤 첫 AV1에서 10프레임 안에서만 H.264가 끼는 것을 허용하고(내부 서버는
    // 허용하지 않는다), 그 뒤에는 모두 AV1이어야 한다.
    let grace = if t.is_external() { 10 } else { 0 };
    let stragglers: Vec<usize> = video[first_av1..]
        .iter()
        .enumerate()
        .filter(|(_, r)| r.codec != Codec::Av1)
        .map(|(i, _)| i)
        .collect();
    if !stragglers.is_empty() {
        eprintln!("[e2e] 첫 AV1 뒤에 온 H.264 프레임 위치(첫 AV1 기준): {stragglers:?}");
    }
    assert!(
        stragglers.iter().all(|i| *i < grace),
        "코덱 전환 뒤에도 H.264 프레임이 왔습니다: {stragglers:?}"
    );
    assert!(
        video.iter().all(|r| r.index.is_some()),
        "프레임 내용이 바뀌었습니다"
    );

    // A/V 동기: 같은 tick의 비디오와 오디오 캡처 시각(시청 쪽 SR 기준) 차이
    let mut diffs = Vec::new();
    let mut diff_ticks = Vec::new();
    for v in &video {
        let (Some(vi), Some(vc)) = (v.index, v.capture) else {
            continue;
        };
        if let Some(a) = audio.iter().find(|a| a.index == Some(vi))
            && let Some(ac) = a.capture
        {
            let d = match vc.duration_since(ac) {
                Ok(d) => d.as_secs_f64(),
                Err(e) => -e.duration().as_secs_f64(),
            };
            diffs.push(d * 1000.0);
            diff_ticks.push(vi);
        }
    }
    assert!(
        diffs.len() >= 20,
        "SR 이후 짝지은 프레임이 너무 적습니다: {}",
        diffs.len()
    );
    let max = diffs.iter().fold(0f64, |m, d| m.max(d.abs()));
    let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
    eprintln!(
        "[e2e] A/V 차이: 평균 {mean:.2}ms, 최대 {max:.2}ms ({}쌍)",
        diffs.len()
    );
    if max >= 5.0 {
        // 진단: 큰 쌍이 어느 tick에 몰려 있는지
        let mut bad: Vec<(u32, f64)> = Vec::new();
        for v in &video {
            if let (Some(vi), Some(vc)) = (v.index, v.capture)
                && let Some(a) = audio.iter().find(|a| a.index == Some(vi))
                && let Some(ac) = a.capture
            {
                let d = match vc.duration_since(ac) {
                    Ok(d) => d.as_secs_f64(),
                    Err(e) => -e.duration().as_secs_f64(),
                };
                if d.abs() * 1000.0 >= 5.0 {
                    bad.push((vi, d * 1000.0));
                }
            }
        }
        eprintln!("[e2e] 큰 쌍(tick, ms): {bad:?}");
    }
    // 방송자는 트랙이 시작하고 약 0.5초 뒤에야 첫 실제 SR을 보내고, 그 전 프레임의 캡처 시각은 서버가 도착 시각으로
    // 대신한다(`sfu.rs` `wallclock`). 시청자용 SR이 그 프레임에서 만들어지면 SR 한 주기 동안 전송 지연만큼(측정
    // 5~13ms) 한두 쌍이 어긋난다(부하가 클 때 보인다). `av_sync.rs`와 같은 기준: 5ms 이상은 2쌍까지, 모두 50ms 미만,
    // 평균 |A/V| 1ms 미만. 지속되는 편향이나 초 단위 어긋남은 걸린다.
    //
    // 외부 서버 모드(다른 프로세스·Docker·WSL 전달을 거쳐 전송 지연이 5~15ms이고 PC 부하에 따라 흔들린다)에서는
    // 위 어긋남이 첫 SR 한 주기(0.5초, 25 tick) 동안 이어져 10여 쌍이 같은 값으로 어긋난다(측정: 11~12쌍, 일정한
    // -10.2~-14.9ms). 그래서 외부 모드에서는 첫 짝에서 30 tick 안의 쌍을 5ms·평균 검사에서 빼고(50ms 미만은 그대로
    // 확인) 그 뒤의 쌍에는 같은 기준을 적용한다. 지속되는 편향은 그 뒤의 쌍에서 걸린다.
    let skip_until = if t.is_external() {
        diff_ticks.first().map_or(0, |t| t + 30)
    } else {
        0
    };
    let steady: Vec<f64> = diffs
        .iter()
        .zip(&diff_ticks)
        .filter(|(_, t)| **t >= skip_until)
        .map(|(d, _)| *d)
        .collect();
    assert!(
        steady.len() >= 20,
        "첫 SR 구간을 뺀 짝이 너무 적습니다: {}",
        steady.len()
    );
    let over5 = steady.iter().filter(|d| d.abs() >= 5.0).count();
    let mean_abs = steady.iter().map(|d| d.abs()).sum::<f64>() / steady.len() as f64;
    eprintln!(
        "[e2e] A/V 5ms 이상 {over5}쌍, 평균 |A/V| {mean_abs:.3}ms (검사 {}쌍)",
        steady.len()
    );
    assert!(
        over5 <= 2 && max < 50.0 && mean_abs < 1.0,
        "A/V 차이가 큽니다: 5ms 이상 {over5}쌍, 최대 {max:.2}ms, 평균 {mean_abs:.3}ms"
    );

    // 방송 정보 갱신이 목록에 반영된다.
    v.wait_list("목록에 H.264", |l| {
        find(l, id).is_some_and(|x| x.codec == VideoCodec::H264)
    })
    .await;

    // 방송 종료 → 시청자에게 BroadcastEnded, 목록에서 사라짐
    b.send(ClientMsg::Unpublish);
    v.wait_for("BroadcastEnded", ended_for(id)).await;
    v.wait_list("목록에서 사라짐", |l| find(l, id).is_none())
        .await;
    drop(w);
    publisher.close();

    // 같은 사람의 두 번째 방송은 끝난 뒤에는 다시 된다(자세한 건 broadcast_end_paths).
    if let Some(stats) = t.stats() {
        // 내부 서버: 전역 상태를 단정할 수 있다. 통계 갱신은 알림보다 아주 조금 늦을 수 있다.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let s = t.stats().unwrap();
            if s.broadcasts == 0 && s.connections == 2 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "서버 통계가 맞지 않습니다: {s:?} (처음 {stats:?})"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// 3. 방송자 3명 동시

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multiple_broadcasters() {
    let t = Target::shared_with(3).await;
    let names = ["동시 방송 A", "동시 방송 B", "동시 방송 C"];
    let passwords = [None, None, Some("셋째-잠금")];

    let mut obs = Client::connect(&t, "관찰자").await;
    let mut pubs = Vec::new();
    for (name, pw) in names.iter().zip(passwords) {
        let mut c = Client::connect(&t, name).await;
        let p = c
            .publish(info(VideoCodec::H264), pw)
            .await
            .expect("방송 시작 실패");
        pubs.push((c, p));
    }
    let ids: Vec<InstallId> = pubs.iter().map(|(c, _)| c.id).collect();

    // 관찰자의 목록에 3개가 이름·잠금 표시와 함께 보인다.
    obs.wait_list("방송 3개", |l| {
        ids.iter().all(|id| find(l, *id).is_some())
    })
    .await;
    for (i, id) in ids.iter().enumerate() {
        let item = find(&obs.list, *id).unwrap();
        assert_eq!(item.name, names[i]);
        assert_eq!(item.locked, passwords[i].is_some(), "{}", names[i]);
    }

    // 방송마다 시청자 2명
    let mut viewers = Vec::new(); // (방송 번호, 클라이언트, 시청)
    for (i, id) in ids.iter().enumerate() {
        for n in 0..2 {
            let mut c = Client::connect(&t, &format!("시청자 {i}-{n}")).await;
            let w = c.watch_connected(*id, passwords[i]).await;
            viewers.push((i, c, w));
        }
    }

    // 세 방송이 동시에 미디어를 보낸다. 페이로드에 방송자 표시(1~3)를 넣는다.
    let sent: Vec<usize> = futures_util::future::join_all(
        pubs.iter()
            .enumerate()
            .map(|(i, (_, p))| async move { Pump::new(i as u32 + 1).h264(p, 150).await }),
    )
    .await;
    settle().await;
    for (i, c, w) in &viewers {
        let rs = w.viewer.drain();
        eprintln!(
            "[e2e] {}: 비디오 {}/{}개, 오디오 {}개",
            c.name,
            video_count(&rs),
            sent[*i],
            audio_count(&rs)
        );
        assert_received(&c.name, &rs, sent[*i], *i as u32 + 1);
    }

    // 하나씩 끝내면 목록이 줄어든다.
    for i in 0..3 {
        pubs[i].0.send(ClientMsg::Unpublish);
        for (j, c, _) in viewers.iter_mut() {
            if *j == i {
                c.wait_for("BroadcastEnded", ended_for(ids[i])).await;
            }
        }
        let gone = ids[..=i].to_vec();
        let left = ids[i + 1..].to_vec();
        obs.wait_list("방송이 줄어듦", |l| {
            gone.iter().all(|id| find(l, *id).is_none())
                && left.iter().all(|id| find(l, *id).is_some())
        })
        .await;
    }
}

// ---------------------------------------------------------------------------
// 4. 시청자 10명

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_viewers() {
    let t = Target::shared_with(1).await;
    let mut b = Client::connect(&t, "시청자 많은 방송").await;
    let id = b.id;
    let publisher = b.publish(info(VideoCodec::H264), None).await.unwrap();
    let mut obs = Client::connect(&t, "관찰자").await;

    // 시청자 10명: 시그널링 연결과 설치 ID가 각각 따로
    let mut viewers = Vec::new();
    for n in 0..10 {
        let mut c = Client::connect(&t, &format!("시청자 {n}")).await;
        let w = c.watch(id, None).await.expect("시청 실패");
        viewers.push((c, w));
    }
    for (_, w) in &viewers {
        w.viewer.wait_connected(Duration::from_secs(15)).await;
    }
    b.wait_for("ViewerCount 10", viewer_count(10)).await;
    obs.wait_list("목록의 시청자 10명", |l| {
        find(l, id).is_some_and(|x| x.viewers == 10)
    })
    .await;

    let mut pump = Pump::new(1);
    let sent = pump.h264(&publisher, 150).await;
    settle().await;
    for (c, w) in &viewers {
        let rs = w.viewer.drain();
        assert_received(&c.name, &rs, sent, 1);
    }
    eprintln!("[e2e] 시청자 10명 모두 비디오 {sent}개 중 80% 이상 받음");

    // 5명이 StopWatch → 5
    let mut rest = viewers.split_off(5);
    let mut stopped = Vec::new(); // 연결은 그대로 둔다(끝에서 같이 닫힌다).
    for (c, w) in viewers {
        c.send(ClientMsg::StopWatch { session: w.session });
        drop(w);
        stopped.push(c);
    }
    b.wait_for("ViewerCount 5", viewer_count(5)).await;
    obs.wait_list("목록의 시청자 5명", |l| {
        find(l, id).is_some_and(|x| x.viewers == 5)
    })
    .await;

    // 2명이 연결 자체를 끊음 → 3
    for _ in 0..2 {
        let (c, w) = rest.remove(0);
        drop(w);
        c.close();
    }
    b.wait_for("ViewerCount 3", viewer_count(3)).await;
    obs.wait_list("목록의 시청자 3명", |l| {
        find(l, id).is_some_and(|x| x.viewers == 3)
    })
    .await;

    // 남은 시청자는 계속 받는다.
    for (_, w) in &rest {
        w.viewer.drain();
    }
    let sent = pump.h264(&publisher, 75).await;
    settle().await;
    for (c, w) in &rest {
        let rs = w.viewer.drain();
        assert_received(&c.name, &rs, sent, 1);
    }
}

// ---------------------------------------------------------------------------
// 5. 비밀번호

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn passwords() {
    let t = Target::shared_with(2).await;
    let secret = "pw-비밀";

    let mut open = Client::connect(&t, "열린 방송").await;
    let _p_open = open.publish(info(VideoCodec::H264), None).await.unwrap();
    let mut locked = Client::connect(&t, "잠긴 방송").await;
    let _p_locked = locked
        .publish(info(VideoCodec::H264), Some(secret))
        .await
        .unwrap();
    let mut v = Client::connect(&t, "시청자").await;

    // 잠기지 않은 방송: 비밀번호 None이나 아무 값이나 된다.
    for pw in [None, Some("아무 값")] {
        let w = v.watch(open.id, pw).await.expect("열린 방송 시청 실패");
        v.send(ClientMsg::StopWatch { session: w.session });
    }

    // 잠긴 방송
    assert_eq!(
        v.watch(locked.id, None).await.err(),
        Some(ErrorCode::PasswordRequired)
    );
    assert_eq!(
        v.watch(locked.id, Some("")).await.err(),
        Some(ErrorCode::PasswordRequired)
    );
    // WrongPassword는 이 시험에서 2번만(서버가 출발지 IP마다 1분에 8번까지만 받는다).
    assert_eq!(
        v.watch(locked.id, Some("틀린 비밀번호")).await.err(),
        Some(ErrorCode::WrongPassword)
    );
    assert_eq!(
        v.watch(locked.id, Some("pw-비")).await.err(),
        Some(ErrorCode::WrongPassword)
    );
    let w = v
        .watch(locked.id, Some(secret))
        .await
        .expect("맞는 비밀번호인데 시청하지 못했습니다");
    v.send(ClientMsg::StopWatch { session: w.session });

    // 방송자 본인도 비밀번호 없이 자기 잠긴 방송을 보면 PasswordRequired.
    let own = locked.id;
    assert_eq!(
        locked.watch(own, None).await.err(),
        Some(ErrorCode::PasswordRequired)
    );
}

// ---------------------------------------------------------------------------
// 6. 비밀번호 횟수 제한(파괴적)

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rate_limit() {
    let Some(t) = Target::exclusive("rate_limit", false).await else {
        return;
    };
    let last_wrong = *client::LAST_WRONG_PASSWORD.lock().unwrap();
    if t.is_external()
        && let Some(last) = last_wrong
    {
        // 이 실행의 다른 시험이 낸 실패가 1분 안에 남아 있으면 8번을 채우기 전에 막힌다. 지워질 때까지 기다린다.
        let wait = Duration::from_secs(61).saturating_sub(last.elapsed());
        if !wait.is_zero() {
            eprintln!(
                "[e2e] 앞선 비밀번호 실패 기록이 지워지도록 {:.0}초 기다립니다",
                wait.as_secs_f64()
            );
            tokio::time::sleep(wait).await;
        }
    }
    let mut b = Client::connect(&t, "제한 방송").await;
    let _p = b
        .publish(info(VideoCodec::H264), Some("정답"))
        .await
        .unwrap();
    let mut v = Client::connect(&t, "제한 시청자").await;

    let mut wrong = 0;
    let mut limited = false;
    for i in 0..10 {
        match v.watch(b.id, Some("틀림")).await.err() {
            Some(ErrorCode::WrongPassword) => wrong += 1,
            Some(ErrorCode::RateLimited) => {
                limited = true;
                eprintln!("[e2e] {}번째 시도에서 RateLimited", i + 1);
                break;
            }
            other => panic!("{}번째 시도: 예상 밖 결과 {other:?}", i + 1),
        }
    }
    assert!(limited, "10번 틀려도 RateLimited가 오지 않았습니다");
    if t.is_external() {
        // 같은 1분 안에 이 서버에 낸 다른 실패(앞선 시험 등)가 있었을 수 있다.
        assert!((1..=8).contains(&wrong), "WrongPassword {wrong}번");
        if wrong != 8 {
            eprintln!(
                "[e2e] 주의: RateLimited 전에 WrongPassword가 {wrong}번(앞선 실패가 1분 안에 있었음)"
            );
        }
    } else {
        assert_eq!(wrong, 8, "횟수 제한 전 WrongPassword 횟수");
    }
    // 맞는 비밀번호도 막힌다.
    assert_eq!(
        v.watch(b.id, Some("정답")).await.err(),
        Some(ErrorCode::RateLimited)
    );

    if t.is_external() {
        // 다른 시험이 같은 서버에서 맞는 비밀번호로 시청하다가 막히지 않도록 실패 기록(1분)이 지워질 때까지 문을 잡고 있는다.
        eprintln!("[e2e] 실패 기록이 지워지도록 61초 기다립니다");
        tokio::time::sleep(Duration::from_secs(61)).await;
        let w = v
            .watch(b.id, Some("정답"))
            .await
            .expect("61초 뒤에도 막혀 있습니다");
        v.send(ClientMsg::StopWatch { session: w.session });
    }
}

// ---------------------------------------------------------------------------
// 7. 방송이 끝나는 경로

struct Setup {
    b: Client,
    p: dongsi_net::publish::Publisher,
    v: Client,
    w: client::Watching,
}

async fn setup_broadcast(t: &Target, label: &str) -> Setup {
    let mut b = Client::connect(t, &format!("{label} 방송자")).await;
    let p = b.publish(info(VideoCodec::H264), None).await.unwrap();
    let mut v = Client::connect(t, &format!("{label} 시청자")).await;
    v.wait_list("방송 목록에 내 방송", |l| find(l, b.id).is_some())
        .await;
    let w = v.watch_connected(b.id, None).await;
    let sent = Pump::new(1).h264(&p, 25).await;
    settle().await;
    assert!(
        video_count(&w.viewer.drain()) * 2 >= sent,
        "{label}: 방송을 시작했는데 미디어가 오지 않습니다"
    );
    Setup { b, p, v, w }
}

/// (a) Unpublish → BroadcastEnded + 목록에서 사라짐, (d) 같은 방송자가 다시 방송하고 다시 볼 수 있다.
async fn end_by_unpublish(t: &Target) {
    let Setup { mut b, p, mut v, w } = setup_broadcast(t, "끝(a)").await;
    let id = b.id;
    b.send(ClientMsg::Unpublish);
    v.wait_for("BroadcastEnded", ended_for(id)).await;
    v.wait_list("목록에서 사라짐", |l| find(l, id).is_none())
        .await;
    b.wait_list("방송자 목록에서도 사라짐", |l| {
        find(l, id).is_none()
    })
    .await;
    drop(w);
    p.close();
    eprintln!("[e2e] (a) Unpublish: BroadcastEnded 확인");

    // (d) 같은 방송자가 다시 방송하고 시청자가 다시 본다.
    let p2 = b
        .publish(info(VideoCodec::H264), None)
        .await
        .expect("끝난 뒤 같은 방송자가 다시 방송하지 못했습니다");
    v.wait_list("목록에 다시 나타남", |l| find(l, id).is_some())
        .await;
    let w2 = v.watch_connected(id, None).await;
    let sent = Pump::new(2).h264(&p2, 75).await;
    settle().await;
    assert_received("(d) 다시 시청", &w2.viewer.drain(), sent, 2);
    eprintln!("[e2e] (d) 같은 방송자가 다시 방송하고 시청자가 다시 봄");
}

/// (b) 방송자의 시그널링 연결이 닫힘 → 시청자 BroadcastEnded
async fn end_by_signal_close(t: &Target) {
    let Setup { b, p, mut v, w } = setup_broadcast(t, "끝(b)").await;
    let id = b.id;
    let start = Instant::now();
    b.close();
    v.wait_for("BroadcastEnded", ended_for(id)).await;
    eprintln!(
        "[e2e] (b) 시그널링 연결 닫음 → BroadcastEnded: {:.2}초",
        start.elapsed().as_secs_f64()
    );
    v.wait_list("목록에서 사라짐", |l| find(l, id).is_none())
        .await;
    drop(w);
    p.close();
}

/// (c) 시그널링은 두고 송출 WebRTC만 닫음 → SFU가 세션 끊김을 알아채고 방송을 끝낸다.
async fn end_by_media_close(t: &Target) {
    let Setup { b, p, mut v, w } = setup_broadcast(t, "끝(c)").await;
    let id = b.id;
    let start = Instant::now();
    p.close();
    let limit = Duration::from_secs(6);
    let ended = v.try_wait_for(limit, ended_for(id)).await;
    let secs = start.elapsed().as_secs_f64();
    eprintln!(
        "[e2e] (c) 송출 WebRTC만 닫음 → BroadcastEnded: {}",
        match &ended {
            Ok(()) => format!("{secs:.1}초"),
            Err(e) => format!("{e} ({secs:.1}초 기다림)"),
        }
    );
    assert!(
        ended.is_ok(),
        "송출 WebRTC만 닫았는데 {limit:?} 안에 BroadcastEnded가 오지 않았습니다"
    );
    v.wait_list("목록에서 사라짐", |l| find(l, id).is_none())
        .await;
    drop(w);
    drop(b);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn broadcast_end_paths() {
    let t = Target::shared_with(3).await;
    tokio::join!(
        end_by_unpublish(&t),
        end_by_signal_close(&t),
        end_by_media_close(&t)
    );
}

// ---------------------------------------------------------------------------
// 8. 재접속

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reconnect() {
    let t = Target::shared_with(2).await;
    let mut b = Client::connect(&t, "재접속 방송자").await;
    let id = b.id;
    let p = b.publish(info(VideoCodec::H264), None).await.unwrap();

    // 클라이언트가 끊었다가 같은 설치 ID로 다시 접속하면 Welcome에 지금 방송이 보인다.
    let c = Client::connect(&t, "재접속 시청자").await;
    let cid = c.id;
    assert!(
        find(&c.list, id).is_some(),
        "처음 Welcome에 방송이 없습니다"
    );
    c.close();
    let c2 = Client::open(&t, "재접속 시청자", cid, None).await;
    assert!(
        find(&c2.list, id).is_some(),
        "다시 접속한 Welcome에 방송이 없습니다: {:?}",
        c2.list
    );
    c2.close();

    // 시청 중이던 시청자도 끊었다가 다시 접속해서 다시 볼 수 있다.
    let mut v = Client::connect(&t, "재접속 시청자 2").await;
    let vid = v.id;
    let w = v.watch_connected(id, None).await;
    let sent = Pump::new(1).h264(&p, 25).await;
    settle().await;
    assert_received("재접속 전", &w.viewer.drain(), sent, 1);
    drop(w);
    v.close();
    let mut v = Client::open(&t, "재접속 시청자 2", vid, None).await;
    assert!(find(&v.list, id).is_some());
    let w = v.watch_connected(id, None).await;
    let sent = Pump::new(1).h264(&p, 25).await;
    settle().await;
    assert_received("시청자 재접속 뒤", &w.viewer.drain(), sent, 1);
    drop(w);

    // 방송자가 끊긴 뒤 다시 접속해 같은 ID로 다시 방송한다.
    b.close();
    p.close();
    let (mut b2, took) = Client::open_retry(&t, "재접속 방송자", id, Duration::from_secs(10)).await;
    eprintln!("[e2e] 방송자 재접속: {:.2}초", took.as_secs_f64());
    // 옛 연결을 서버가 정리하기 전에 다시 방송하면 AlreadyPublishing일 수 있다. 몇 번째에 되는지 센다.
    let mut attempts = 0;
    let p2 = loop {
        attempts += 1;
        match b2.publish(info(VideoCodec::H264), None).await {
            Ok(p) => break p,
            Err(ErrorCode::AlreadyPublishing) if attempts < 50 => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(code) => panic!("재방송 실패({attempts}번째): {code:?}"),
        }
    };
    eprintln!("[e2e] 같은 ID로 다시 방송: {attempts}번째 시도에 성공");
    v.wait_for("옛 방송의 BroadcastEnded", ended_for(id)).await;
    v.wait_list("새 방송이 목록에 있음", |l| find(l, id).is_some())
        .await;
    let w = v.watch_connected(id, None).await;
    let sent = Pump::new(3).h264(&p2, 75).await;
    settle().await;
    assert_received("방송자 재접속 뒤", &w.viewer.drain(), sent, 3);
}

// ---------------------------------------------------------------------------
// 9. 서버 재시작(파괴적)

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_restart() {
    let Some(mut t) = Target::exclusive("server_restart", true).await else {
        return;
    };
    if !t.can_restart() {
        eprintln!("[e2e] 건너뜀: server_restart는 DONGSI_E2E_RESTART_CMD가 필요합니다");
        return;
    }
    let mut b = Client::connect(&t, "재시작 방송자").await;
    let id = b.id;
    let p = b.publish(info(VideoCodec::H264), None).await.unwrap();
    let mut v = Client::connect(&t, "재시작 시청자").await;
    let vid = v.id;
    let w = v.watch_connected(id, None).await;
    let sent = Pump::new(1).h264(&p, 25).await;
    settle().await;
    assert_received("재시작 전", &w.viewer.drain(), sent, 1);

    t.restart().await;

    // 두 연결 모두 Closing을 받고 닫힌다.
    for c in [&mut b, &mut v] {
        let seen = c.expect_closed(Duration::from_secs(20)).await;
        let reason = seen
            .iter()
            .find_map(|m| match m {
                ServerMsg::Closing { reason } => Some(reason.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{}: Closing을 받지 못했습니다: {seen:?}", c.name));
        if t.is_external() {
            assert!(!reason.is_empty(), "Closing 이유가 비어 있습니다");
        } else {
            assert_eq!(reason, "서버가 닫혔습니다");
        }
        eprintln!("[e2e] {}: Closing \"{reason}\" 뒤 연결 닫힘", c.name);
    }
    drop(w);
    p.close();

    // 고정한 지문으로 다시 접속된다(재시작 뒤에도 지문이 같다).
    let (mut b2, took) = Client::open_retry(&t, "재시작 방송자", id, Duration::from_secs(30)).await;
    assert_eq!(b2.conn.fingerprint, t.fp);
    eprintln!(
        "[e2e] 재시작 뒤 다시 접속까지 {:.1}초(지문 같음)",
        took.as_secs_f64()
    );
    assert!(
        find(&b2.list, id).is_none(),
        "재시작한 서버에 옛 방송이 남아 있습니다"
    );

    // 다시 방송하고 시청해서 미디어가 온다.
    let p2 = b2.publish(info(VideoCodec::H264), None).await.unwrap();
    let mut v2 = Client::open(&t, "재시작 시청자", vid, None).await;
    v2.wait_list("새 방송이 목록에 있음", |l| find(l, id).is_some())
        .await;
    let w2 = v2.watch_connected(id, None).await;
    let sent = Pump::new(2).h264(&p2, 75).await;
    settle().await;
    assert_received("재시작 뒤", &w2.viewer.drain(), sent, 2);
}

// ---------------------------------------------------------------------------
// 10. 로컬 감지

/// 서버에 UDP 유니캐스트로 감지 질의를 보낸다(0.5초마다 다시 보낸다). 응답 조각을 모아 합치고, 조건에 맞는
/// 응답(지문이 같고 방송이 들어 있음)이 `within` 안에 완성되면 돌려준다. 못 찾으면 마지막으로 합친 응답을 준다.
async fn query_discovery(
    target: SocketAddr,
    want_fp: Fingerprint,
    want: InstallId,
    within: Duration,
) -> (
    Option<DiscoveryResponse>,
    Option<DiscoveryResponse>,
    Duration,
) {
    let bind = if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let sock = tokio::net::UdpSocket::bind(bind).await.unwrap();
    let nonce: u64 = rand::random();
    let query = encode_query(nonce);
    let started = Instant::now();
    let deadline = started + within;
    let mut next_send = started;
    let mut last = None;
    let mut asm = PartAssembler::new();
    let mut buf = vec![0u8; 4096];
    while Instant::now() < deadline {
        if Instant::now() >= next_send {
            let _ = sock.send_to(&query, target).await;
            next_send += Duration::from_millis(500);
        }
        let until = next_send.min(deadline);
        let left = until.saturating_duration_since(Instant::now());
        let Ok(Ok((n, _))) = tokio::time::timeout(left, sock.recv_from(&mut buf)).await else {
            continue;
        };
        let Some(part) = decode_part(&buf[..n]) else {
            continue;
        };
        if part.response.nonce != nonce {
            continue;
        }
        if !asm.push(part) {
            continue;
        }
        let resp = asm.merge().unwrap();
        if resp.fingerprint == want_fp && resp.broadcasts.iter().any(|b| b.broadcaster == want) {
            return (Some(resp.clone()), Some(resp), started.elapsed());
        }
        last = Some(resp);
    }
    (None, last, started.elapsed())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery() {
    let t = Target::shared_with(1).await;
    let _gate = t.discovery_gate().await;
    if t.is_external() && !t.discovery_enabled() {
        eprintln!("[e2e] 건너뜀: discovery(DONGSI_E2E_DISCOVERY=off)");
        return;
    }
    let id = InstallId::from_bytes(rand::random());
    let mut b = Client::open(&t, "감지 방송자", id, None).await;
    let _p = b
        .publish(info(VideoCodec::H264), Some("잠금"))
        .await
        .unwrap();

    if !t.is_external() {
        // 내부: 브로드캐스트·루프백 질의로 이 서버를 찾는다.
        let found = dongsi_net::discovery::scan(t.port(), Duration::from_millis(800)).await;
        let me = found
            .iter()
            .find(|d| d.fingerprint() == t.fp)
            .expect("감지 응답이 없습니다");
        assert_eq!(me.response.port, t.port());
        assert_eq!(me.response.name, target::SERVER_NAME);
        assert_eq!(me.response.broadcasts.len(), 1);
        assert_eq!(me.response.broadcasts[0].broadcaster, id);
        assert!(me.response.broadcasts[0].locked);
        return;
    }

    // 외부: 서버 IP:포트로 유니캐스트 질의
    let (hit, last, took) = query_discovery(t.udp_addr(), t.fp, id, Duration::from_secs(2)).await;
    let resp = hit.unwrap_or_else(|| {
        panic!(
            "2초 안에 지문이 같고 내 방송이 든 감지 응답을 받지 못했습니다(마지막 응답: {last:?})"
        )
    });
    assert_eq!(
        resp.port,
        t.expected_discovery_port(),
        "감지 응답의 포트가 다릅니다: 응답 {}, 기대 {}(대상 포트 {}). 서버는 실제로 듣는 포트를 알립니다. \
         다른 번호로 매핑했으면 DONGSI_E2E_DISCOVERY_PORT=<컨테이너 포트>",
        resp.port,
        t.expected_discovery_port(),
        t.port()
    );
    assert!(!resp.name.is_empty());
    assert!(!resp.host_only);
    let mine = resp
        .broadcasts
        .iter()
        .find(|x| x.broadcaster == id)
        .unwrap();
    assert_eq!(mine.name, "감지 방송자");
    assert!(mine.locked);
    assert_eq!(mine.codec, VideoCodec::H264);
    eprintln!(
        "[e2e] 감지 유니캐스트 응답 {:.0}ms, 방송 {}개",
        took.as_secs_f64() * 1000.0,
        resp.broadcasts.len()
    );

    // 브로드캐스트·루프백 훑기: 브리지와 host에서 다를 수 있어 결과는 출력만 한다.
    let found = dongsi_net::discovery::scan(t.port(), Duration::from_millis(800)).await;
    match found.iter().find(|d| d.fingerprint() == t.fp) {
        Some(d) => eprintln!("[e2e] scan(): 이 서버를 찾음({:?}, {})", d.via, d.ip),
        None => eprintln!(
            "[e2e] scan(): 이 서버를 찾지 못함(찾은 서버 {}개)",
            found.len()
        ),
    }
}

// ---------------------------------------------------------------------------
// 11. 상한과 호스트 전용(내부 서버 전용)

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn limits() {
    let Some(t) = Target::internal_only(
        "limits",
        ServerConfig {
            max_broadcasts: 1,
            max_viewers_per_broadcast: 1,
            ..default_config()
        },
    )
    .await
    else {
        return;
    };
    let mut a = Client::connect(&t, "첫 방송자").await;
    let _pa = a.publish(info(VideoCodec::H264), None).await.unwrap();
    let mut b = Client::connect(&t, "둘째 방송자").await;
    assert_eq!(
        b.publish_err(info(VideoCodec::H264), None).await,
        ErrorCode::ServerFull
    );
    let mut v1 = Client::connect(&t, "첫 시청자").await;
    let _w1 = v1.watch(a.id, None).await.expect("첫 시청 실패");
    let mut v2 = Client::connect(&t, "둘째 시청자").await;
    assert_eq!(
        v2.watch(a.id, None).await.err(),
        Some(ErrorCode::ServerFull)
    );

    // 호스트 토큰이 있는 서버(내장 서버): 토큰 없는 접속은 방송할 수 없다.
    let t2 = Target::internal_only(
        "limits",
        ServerConfig {
            host_token: Some("호스트-토큰".into()),
            ..default_config()
        },
    )
    .await
    .unwrap();
    let mut guest = Client::open(&t2, "손님", InstallId::generate(), None).await;
    assert!(!guest.welcome.can_publish);
    assert!(guest.welcome.host_only);
    assert_eq!(
        guest.publish_err(info(VideoCodec::H264), None).await,
        ErrorCode::NotAllowed
    );
    let mut wrong = Client::open(&t2, "틀린 토큰", InstallId::generate(), Some("아님")).await;
    assert!(!wrong.welcome.can_publish);
    assert_eq!(
        wrong.publish_err(info(VideoCodec::H264), None).await,
        ErrorCode::NotAllowed
    );
    let mut host = Client::open(&t2, "호스트", InstallId::generate(), Some("호스트-토큰")).await;
    assert!(host.welcome.can_publish);
    assert!(host.welcome.host_only);
    let _p = host
        .publish(info(VideoCodec::H264), None)
        .await
        .expect("호스트가 방송하지 못했습니다");
    // 손님은 시청할 수 있다.
    let w = guest.watch_connected(host.id, None).await;
    drop(w);
}

// ---------------------------------------------------------------------------
// 12. 방송 정보 갱신

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_publish() {
    let t = Target::shared_with(1).await;
    let mut b = Client::connect(&t, "갱신 방송자").await;
    let id = b.id;
    let _p = b.publish(info(VideoCodec::H264), None).await.unwrap();
    let mut obs = Client::connect(&t, "관찰자").await;
    obs.wait_list("방송이 보임", |l| find(l, id).is_some())
        .await;

    let new = PublishInfo {
        codec: VideoCodec::Av1,
        width: 2560,
        height: 1440,
        fps: 60,
        has_audio: false,
    };
    b.send(ClientMsg::UpdatePublish { info: new });
    obs.wait_list("바뀐 정보가 목록에 반영됨", |l| {
        find(l, id).is_some_and(|x| {
            x.codec == new.codec
                && x.width == new.width
                && x.height == new.height
                && x.fps == new.fps
                && x.has_audio == new.has_audio
        })
    })
    .await;

    // 방송자가 아닌 연결, 같은 설치 ID로 따로 접속한 연결이 보낸 UpdatePublish는 무시된다.
    let bogus = PublishInfo {
        codec: VideoCodec::H264,
        width: 640,
        height: 360,
        fps: 15,
        has_audio: true,
    };
    let mut impostor = Client::open(&t, "가짜 방송자", id, None).await;
    obs.send(ClientMsg::UpdatePublish { info: bogus });
    impostor.send(ClientMsg::UpdatePublish { info: bogus });
    // 서버는 연결마다 메시지를 차례로 처리한다. 없는 방송을 보려는 요청의 NotFound가 오면 앞선
    // UpdatePublish는 처리가 끝난 것이다(받아들였다면 목록 갱신이 NotFound보다 먼저 와 있다).
    for c in [&mut impostor, &mut obs] {
        assert_eq!(
            c.watch(InstallId::generate(), None).await.err(),
            Some(ErrorCode::NotFound)
        );
    }
    let item = find(&obs.list, id).expect("방송이 사라졌습니다");
    assert_eq!(
        (
            item.codec,
            item.width,
            item.height,
            item.fps,
            item.has_audio
        ),
        (new.codec, new.width, new.height, new.fps, new.has_audio),
        "방송자가 아닌 연결의 UpdatePublish가 반영되었습니다"
    );
    // 서버가 아직 다른 메시지를 보내는지 확인할 겸, 시험이 쓴 `Next`를 쓴다.
    assert!(matches!(
        obs.poll_msg(Duration::from_millis(50)).await,
        Next::Timeout | Next::Msg(_)
    ));
}
