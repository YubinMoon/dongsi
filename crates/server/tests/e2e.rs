//! 서버 통합 시험: 실제 서버를 띄우고 송출(dongsi-net) → SFU → 시청(str0m)을 이어 본다.
//!
//! - 시그널링(TLS 지문 고정, Hello/Welcome, 방송 목록)
//! - 방송 비밀번호(없음/틀림/맞음)
//! - 비디오 프레임 전달, 방송 중 코덱 전환(VP8 → H.264)
//! - 오디오·비디오 동기: 같은 캡처 시각으로 보낸 프레임이 시청 쪽 SR 기준으로도 같은 시각인지
//! - 새 시청자 연결 시 방송자에게 키프레임 요청
//! - 로컬 감지(루프백 질의)

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use dongsi_core::proto::{
    BroadcastInfo, ClientMsg, ErrorCode, PROTOCOL_VERSION, PublishInfo, ServerMsg,
};
use dongsi_core::{InstallId, ServerAddr, VideoCodec};
use dongsi_net::publish::{PendingPublisher, SR_INTERVAL};
use dongsi_net::signal::{self, Connection};
use dongsi_net::tls::Identity;
use dongsi_server::ServerConfig;
use str0m::change::SdpAnswer;
use str0m::format::Codec;
use str0m::media::{Direction, MediaKind};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};

const MAGIC: &[u8; 3] = b"DSK";

fn server_addr(port: u16) -> ServerAddr {
    format!("127.0.0.1:{port}").parse().unwrap()
}

async fn next(conn: &mut Connection) -> ServerMsg {
    tokio::time::timeout(Duration::from_secs(10), conn.receiver.recv())
        .await
        .expect("서버 메시지 시간 초과")
        .expect("연결 닫힘")
}

/// 원하는 메시지가 올 때까지 다른 메시지(방송 목록 등)를 건너뛴다.
async fn wait_for<T>(conn: &mut Connection, mut f: impl FnMut(&ServerMsg) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            Instant::now() < deadline,
            "기다리던 메시지가 오지 않았습니다"
        );
        let msg = next(conn).await;
        if let Some(v) = f(&msg) {
            return v;
        }
    }
}

async fn connect(
    port: u16,
    fp: dongsi_core::Fingerprint,
    name: &str,
    id: InstallId,
) -> (Connection, Vec<BroadcastInfo>) {
    let mut conn = signal::connect(&server_addr(port), Some(fp))
        .await
        .expect("접속 실패");
    conn.sender
        .send(ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
            app_version: "test".into(),
            install_id: id,
            display_name: name.into(),
            connected_ip: Some(signal::connected_ip(&conn)),
            host_token: None,
        })
        .unwrap();
    match next(&mut conn).await {
        ServerMsg::Welcome {
            protocol: PROTOCOL_VERSION,
            can_publish: true,
            broadcasts,
            ..
        } => (conn, broadcasts),
        other => panic!("Welcome이 아닙니다: {other:?}"),
    }
}

/// 시청 쪽 str0m 클라이언트(시험용). 받은 미디어를 채널로 넘긴다.
struct TestViewer {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    media: std::sync::mpsc::Receiver<Received>,
}

#[derive(Debug, Clone)]
struct Received {
    kind: MediaKind,
    codec: Codec,
    index: Option<u32>,
    /// 시청 쪽에서 SR로 복원한 캡처 시각
    capture: Option<SystemTime>,
}

struct PendingViewer {
    rtc: Rtc,
    pending: str0m::change::SdpPendingOffer,
    socket: UdpSocket,
    local: SocketAddr,
    offer: String,
}

impl PendingViewer {
    fn new() -> Self {
        let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
        let ip = dongsi_net::ifaces::primary_lan_ip()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let local = SocketAddr::new(ip, socket.local_addr().unwrap().port());
        let mut rtc = Rtc::builder()
            .clear_codecs()
            .enable_opus(true, false)
            .enable_h264(true)
            .enable_h265(true)
            .enable_vp8(true)
            .enable_vp9(true)
            .enable_av1(true)
            .set_rtcp_report_interval_audio(SR_INTERVAL)
            .set_rtcp_report_interval_video(SR_INTERVAL)
            .build(Instant::now());
        rtc.add_local_candidate(Candidate::host(local, "udp").unwrap());
        let mut api = rtc.sdp_api();
        api.add_media(MediaKind::Video, Direction::RecvOnly, None, None, None);
        api.add_media(MediaKind::Audio, Direction::RecvOnly, None, None, None);
        let (offer, pending) = api.apply().unwrap();
        Self {
            rtc,
            pending,
            socket,
            local,
            offer: offer.to_sdp_string(),
        }
    }

    fn start(self, answer: &str) -> TestViewer {
        let PendingViewer {
            mut rtc,
            pending,
            socket,
            local,
            ..
        } = self;
        rtc.sdp_api()
            .accept_answer(pending, SdpAnswer::from_sdp_string(answer).unwrap())
            .unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(5)))
            .unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let thread_stop = stop.clone();
        let thread = thread::spawn(move || {
            let mut buf = vec![0u8; 2048];
            while !thread_stop.load(Ordering::Relaxed) && rtc.is_alive() {
                loop {
                    match rtc.poll_output().unwrap() {
                        Output::Timeout(_) => break,
                        Output::Transmit(t) => {
                            let _ = socket.send_to(&t.contents, t.destination);
                        }
                        Output::Event(Event::MediaData(d)) => {
                            let kind = if d.params.spec().codec == Codec::Opus {
                                MediaKind::Audio
                            } else {
                                MediaKind::Video
                            };
                            let capture = d.last_sender_info.map(|sr| {
                                let delta = d.time.as_seconds()
                                    - sr.rtp_time.rebase(d.time.frequency()).as_seconds();
                                if delta >= 0.0 {
                                    sr.ntp_time + Duration::from_secs_f64(delta)
                                } else {
                                    sr.ntp_time - Duration::from_secs_f64(-delta)
                                }
                            });
                            let _ = tx.send(Received {
                                kind,
                                codec: d.params.spec().codec,
                                index: find_index(&d.data),
                                capture,
                            });
                        }
                        Output::Event(_) => {}
                    }
                }
                match socket.recv_from(&mut buf) {
                    Ok((n, source)) => {
                        if let Ok(contents) = buf[..n].try_into() {
                            let _ = rtc.handle_input(Input::Receive(
                                Instant::now(),
                                Receive {
                                    proto: Protocol::Udp,
                                    source,
                                    destination: local,
                                    contents,
                                },
                            ));
                        }
                    }
                    Err(_) => {
                        let _ = rtc.handle_input(Input::Timeout(Instant::now()));
                    }
                }
            }
        });
        TestViewer {
            stop,
            thread: Some(thread),
            media: rx,
        }
    }
}

impl Drop for TestViewer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn find_index(data: &[u8]) -> Option<u32> {
    let pos = data.windows(3).position(|w| w == MAGIC)?;
    let b = data.get(pos + 3..pos + 7)?;
    Some(u32::from_le_bytes(b.try_into().ok()?))
}

fn payload(index: u32, len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len + 7);
    v.extend_from_slice(MAGIC);
    v.extend_from_slice(&index.to_le_bytes());
    v.resize(len, 0xAA);
    v
}

/// 시험용 비디오 프레임. H.264는 Annex B(SPS, PPS, IDR).
fn video_frame(codec: VideoCodec, index: u32) -> Vec<u8> {
    match codec {
        VideoCodec::H264 => {
            let mut v = vec![0, 0, 0, 1, 0x67, 0x64, 0x00, 0x1f, 0xac, 0xd9, 0x40];
            v.extend_from_slice(&[0, 0, 0, 1, 0x68, 0xeb, 0xe3, 0xcb]);
            v.extend_from_slice(&[0, 0, 0, 1, 0x65]);
            v.extend_from_slice(&payload(index, 3000));
            v
        }
        // VP8: 첫 바이트의 0번 비트가 0이면 키프레임
        _ => {
            let mut v = vec![0x10, 0x02, 0x00, 0x9d, 0x01, 0x2a];
            v.extend_from_slice(&payload(index, 3000));
            v
        }
    }
}

fn info(codec: VideoCodec) -> PublishInfo {
    PublishInfo {
        codec,
        width: 1920,
        height: 1080,
        fps: 30,
        has_audio: true,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn publish_and_watch() {
    let identity = Identity::generate().unwrap();
    let server = dongsi_server::start(
        ServerConfig {
            name: "시험 서버".into(),
            port: 0,
            ..Default::default()
        },
        &identity,
    )
    .await
    .unwrap();
    let port = server.port();
    let fp = server.fingerprint();

    // 지문이 다르면 접속하지 않는다.
    let wrong = Identity::generate().unwrap().fingerprint();
    let err = signal::connect(&server_addr(port), Some(wrong))
        .await
        .err()
        .expect("지문이 달라도 접속되었습니다");
    assert!(
        matches!(err, dongsi_net::NetError::FingerprintMismatch { .. }),
        "{err:?}"
    );

    // 방송자
    let broadcaster_id = InstallId::generate();
    let (mut pub_conn, list) = connect(port, fp, "방송자", broadcaster_id).await;
    assert!(list.is_empty());
    let server_ip = signal::connected_ip(&pub_conn);
    let pending = PendingPublisher::new(server_ip, true).unwrap();
    pub_conn
        .sender
        .send(ClientMsg::Publish {
            req: 1,
            offer: pending.offer.clone(),
            info: info(VideoCodec::Vp8),
            password: Some("비밀".into()),
        })
        .unwrap();
    let answer = wait_for(&mut pub_conn, |m| match m {
        ServerMsg::Published { req: 1, answer } => Some(answer.clone()),
        ServerMsg::Error { message, .. } => panic!("송출 실패: {message}"),
        _ => None,
    })
    .await;
    let publisher = pending.start(&answer).unwrap();

    // 같은 사람이 두 번 송출할 수 없다.
    pub_conn
        .sender
        .send(ClientMsg::Publish {
            req: 2,
            offer: PendingPublisher::new(server_ip, false).unwrap().offer,
            info: info(VideoCodec::Vp8),
            password: None,
        })
        .unwrap();
    let code = wait_for(&mut pub_conn, |m| match m {
        ServerMsg::Error {
            req: Some(2), code, ..
        } => Some(*code),
        _ => None,
    })
    .await;
    assert_eq!(code, ErrorCode::AlreadyPublishing);

    // 송출 연결이 될 때까지 기다린다.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !publisher.stats().connected {
        assert!(
            Instant::now() < deadline,
            "송출 연결 실패: {:?}",
            publisher.stats()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // 첫 연결 시 키프레임 요청 표시가 켜진다. 비워 둔다.
    publisher.take_keyframe_request();

    // 시청자: 방송 목록에 잠긴 방송이 보인다.
    let (mut view_conn, list) = connect(port, fp, "시청자", InstallId::generate()).await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].broadcaster, broadcaster_id);
    assert_eq!(list[0].name, "방송자");
    assert!(list[0].locked);
    assert_eq!(list[0].codec, VideoCodec::Vp8);

    // 비밀번호 없음 → PasswordRequired, 틀림 → WrongPassword
    for (req, password, expected) in [
        (10, None, ErrorCode::PasswordRequired),
        (11, Some("아님".to_string()), ErrorCode::WrongPassword),
    ] {
        let v = PendingViewer::new();
        view_conn
            .sender
            .send(ClientMsg::Watch {
                req,
                broadcaster: broadcaster_id,
                password,
                offer: v.offer,
            })
            .unwrap();
        let code = wait_for(&mut view_conn, |m| match m {
            ServerMsg::Error {
                req: Some(r), code, ..
            } if *r == req => Some(*code),
            _ => None,
        })
        .await;
        assert_eq!(code, expected);
    }

    // 맞는 비밀번호
    let pending_viewer = PendingViewer::new();
    view_conn
        .sender
        .send(ClientMsg::Watch {
            req: 12,
            broadcaster: broadcaster_id,
            password: Some("비밀".into()),
            offer: pending_viewer.offer.clone(),
        })
        .unwrap();
    let (session, answer) = wait_for(&mut view_conn, |m| match m {
        ServerMsg::Watching {
            req: 12,
            session,
            answer,
        } => Some((*session, answer.clone())),
        ServerMsg::Error {
            req: Some(12),
            message,
            ..
        } => panic!("시청 실패: {message}"),
        _ => None,
    })
    .await;
    let viewer = pending_viewer.start(&answer);

    // 방송자에게 시청자 수가 간다.
    let count = wait_for(&mut pub_conn, |m| match m {
        ServerMsg::ViewerCount { count } if *count > 0 => Some(*count),
        _ => None,
    })
    .await;
    assert_eq!(count, 1);

    // 시청자가 연결되면 SFU가 방송자에게 키프레임을 요청한다.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !publisher.take_keyframe_request() {
        assert!(Instant::now() < deadline, "키프레임 요청이 오지 않았습니다");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // 미디어: 20ms마다 오디오, 40ms마다 비디오(같은 캡처 시각). 절반은 VP8, 나머지는 H.264.
    let total_ticks: u32 = 200; // 4초
    let start = Instant::now();
    let mut sample_index = 0u64;
    for tick in 0..total_ticks {
        let at = start + Duration::from_millis(20 * u64::from(tick));
        let now = Instant::now();
        if at > now {
            tokio::time::sleep(at - now).await;
        }
        let capture = at;
        publisher.send_audio(payload(tick, 80), capture, sample_index);
        sample_index += 960;
        if tick % 2 == 0 {
            let codec = if tick < total_ticks / 2 {
                VideoCodec::Vp8
            } else {
                VideoCodec::H264
            };
            if tick == total_ticks / 2 {
                pub_conn
                    .sender
                    .send(ClientMsg::UpdatePublish {
                        info: info(VideoCodec::H264),
                    })
                    .unwrap();
            }
            publisher.send_video(codec, video_frame(codec, tick), capture);
        }
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    let received: Vec<Received> = viewer.media.try_iter().collect();
    let video: Vec<&Received> = received
        .iter()
        .filter(|r| r.kind == MediaKind::Video)
        .collect();
    let audio: Vec<&Received> = received
        .iter()
        .filter(|r| r.kind == MediaKind::Audio)
        .collect();
    eprintln!(
        "받음: 비디오 {}개(SR {}), 오디오 {}개(SR {})",
        video.len(),
        video.iter().filter(|r| r.capture.is_some()).count(),
        audio.len(),
        audio.iter().filter(|r| r.capture.is_some()).count(),
    );
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

    // 코덱 전환: VP8 다음에 H.264가 온다.
    assert!(video.iter().any(|r| r.codec == Codec::Vp8));
    assert!(video.iter().any(|r| r.codec == Codec::H264));
    let first_h264 = video.iter().position(|r| r.codec == Codec::H264).unwrap();
    assert!(video[first_h264..].iter().all(|r| r.codec == Codec::H264));
    assert!(
        video.iter().all(|r| r.index.is_some()),
        "프레임 내용이 바뀌었습니다"
    );

    // A/V 동기: 같은 tick의 비디오와 오디오 캡처 시각(시청 쪽 SR 기준) 차이
    let mut diffs = Vec::new();
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
        "A/V 차이: 평균 {mean:.2}ms, 최대 {max:.2}ms ({}쌍)",
        diffs.len()
    );
    assert!(max < 5.0, "A/V 차이가 큽니다: 최대 {max:.2}ms");

    // 방송 정보 갱신이 목록에 반영된다.
    let codec = wait_for(&mut view_conn, |m| match m {
        ServerMsg::Broadcasts { broadcasts } => broadcasts
            .iter()
            .find(|b| b.broadcaster == broadcaster_id && b.codec == VideoCodec::H264)
            .map(|b| b.codec),
        _ => None,
    })
    .await;
    assert_eq!(codec, VideoCodec::H264);

    // 감지: 루프백 질의에 이 서버가 응답한다.
    let found = dongsi_net::discovery::scan(port, Duration::from_millis(800)).await;
    let me = found
        .iter()
        .find(|d| d.fingerprint() == fp)
        .expect("감지 응답이 없습니다");
    assert_eq!(me.response.port, port);
    assert_eq!(me.response.broadcasts.len(), 1);
    assert!(me.response.broadcasts[0].locked);

    // 방송 종료 → 시청자에게 BroadcastEnded
    let _ = session;
    pub_conn.sender.send(ClientMsg::Unpublish).unwrap();
    let ended = wait_for(&mut view_conn, |m| match m {
        ServerMsg::BroadcastEnded { broadcaster } => Some(*broadcaster),
        _ => None,
    })
    .await;
    assert_eq!(ended, broadcaster_id);
    drop(viewer);
    publisher.close();

    let stats = server.stats();
    assert_eq!(stats.broadcasts, 0);
    assert_eq!(stats.connections, 2);
    server.shutdown().await;

    // 서버가 닫히면 접속자에게 Closing이 간다.
    wait_for(&mut view_conn, |m| match m {
        ServerMsg::Closing { .. } => Some(()),
        _ => None,
    })
    .await;
}
