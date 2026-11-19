//! 시청 쪽 str0m 클라이언트(시험용). 받은 미디어를 채널로 넘긴다.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use dongsi_net::publish::SR_INTERVAL;
use str0m::change::{SdpAnswer, SdpPendingOffer};
use str0m::format::Codec;
use str0m::media::{Direction, MediaKind};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};

use crate::media::find_marker;

#[derive(Debug, Clone)]
pub struct Received {
    pub kind: MediaKind,
    pub codec: Codec,
    /// 페이로드에 넣은 tick 번호
    pub index: Option<u32>,
    /// 페이로드에 넣은 방송자 표시
    pub tag: Option<u32>,
    /// 시청 쪽에서 SR로 복원한 캡처 시각
    pub capture: Option<SystemTime>,
}

pub fn video_count(rs: &[Received]) -> usize {
    rs.iter().filter(|r| r.kind == MediaKind::Video).count()
}

pub fn audio_count(rs: &[Received]) -> usize {
    rs.iter().filter(|r| r.kind == MediaKind::Audio).count()
}

pub struct TestViewer {
    stop: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    media: std::sync::mpsc::Receiver<Received>,
}

impl TestViewer {
    /// 미디어 연결(ICE·DTLS)이 됐는지.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// 지금까지 받은 미디어를 모두 꺼낸다.
    pub fn drain(&self) -> Vec<Received> {
        self.media.try_iter().collect()
    }

    pub async fn wait_connected(&self, within: Duration) {
        let deadline = Instant::now() + within;
        while !self.is_connected() {
            assert!(
                Instant::now() < deadline,
                "시청자의 미디어 연결이 {within:?} 안에 되지 않았습니다"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
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

pub struct PendingViewer {
    rtc: Rtc,
    pending: SdpPendingOffer,
    socket: UdpSocket,
    local: SocketAddr,
    pub offer: String,
}

/// 이 PC에서 `target`으로 갈 때 쓰는 로컬 IP. 루프백이면 서버와 같은 규칙으로 LAN 주소를 쓴다.
pub fn route_local_ip(target: IpAddr) -> IpAddr {
    if target.is_loopback() {
        return dongsi_net::ifaces::primary_lan_ip()
            .map(IpAddr::V4)
            .unwrap_or(target);
    }
    let bind = if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    UdpSocket::bind(bind)
        .and_then(|probe| {
            probe.connect(SocketAddr::new(target, 9))?;
            probe.local_addr()
        })
        .map(|a| a.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

impl PendingViewer {
    pub fn new(server_ip: IpAddr) -> Self {
        let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
        let local = SocketAddr::new(
            route_local_ip(server_ip),
            socket.local_addr().unwrap().port(),
        );
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

    pub fn start(self, answer: &str) -> TestViewer {
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
        let stop = Arc::new(AtomicBool::new(false));
        let connected = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let thread_stop = stop.clone();
        let thread_connected = connected.clone();
        let thread = thread::spawn(move || {
            let mut buf = vec![0u8; 2048];
            while !thread_stop.load(Ordering::Relaxed) && rtc.is_alive() {
                // str0m이 알려 준 다음 타임아웃까지만 소켓을 기다린다(시청자가 많아도 CPU를 아낀다).
                let next = loop {
                    match rtc.poll_output() {
                        Ok(Output::Timeout(t)) => break t,
                        Ok(Output::Transmit(t)) => {
                            let _ = socket.send_to(&t.contents, t.destination);
                        }
                        Ok(Output::Event(Event::Connected)) => {
                            thread_connected.store(true, Ordering::Relaxed);
                        }
                        Ok(Output::Event(Event::MediaData(d))) => {
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
                            let marker = find_marker(&d.data);
                            let _ = tx.send(Received {
                                kind,
                                codec: d.params.spec().codec,
                                index: marker.map(|m| m.0),
                                tag: marker.map(|m| m.1),
                                capture,
                            });
                        }
                        Ok(Output::Event(_)) => {}
                        Err(_) => return,
                    }
                };
                let wait = next
                    .saturating_duration_since(Instant::now())
                    .clamp(Duration::from_millis(1), Duration::from_millis(20));
                let _ = socket.set_read_timeout(Some(wait));
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
            connected,
            thread: Some(thread),
            media: rx,
        }
    }
}
