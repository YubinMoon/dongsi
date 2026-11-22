//! 시청 쪽 str0m 클라이언트(시험용). 받은 미디어를 채널로 넘긴다.
//!
//! 손실 주입(D-087): `TestViewer::impair`(impair.rs)는 서버에서 받은 패킷을 str0m에 넘기기 전에 버리거나 붙잡는다
//! (소켓 `recv_from` 직후). 시청 쪽 str0m이 보내는 NACK·PLI 수는 `TestViewer::ingress`(`MediaIngressStats`)로 본다
//! (`ViewerOptions::stats`). `TestViewer::request_keyframe`는 시청자 str0m이 서버로 RTCP PLI를 보내게 한다.
//!
//! 오디오 RED(D-087): 기본으로 RED(RFC 2198)를 켜고 offer에 `red/48000/2`를 싣는다(`ViewerOptions::red`, 거리
//! `AUDIO_RED_DISTANCES`). 끄면 일반 Opus로 협상된다(fallback 시험). RED로 받아도 str0m이 RED를 풀어 Opus 프레임 하나씩
//! `MediaData`로 내므로(복구된 프레임 포함) 아래 오디오 판별(`Codec::Opus`)과 `Received` 기록은 그대로다.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use dongsi_core::media::AUDIO_RED_DISTANCES;
use dongsi_net::publish::SR_INTERVAL;
use str0m::change::{SdpAnswer, SdpPendingOffer};
use str0m::format::Codec;
use str0m::media::{Direction, KeyframeRequestKind, MediaKind};
use str0m::net::{Protocol, Receive};
use str0m::rtp::RawPacket;
use str0m::rtp::rtcp::Rtcp;
use str0m::{Candidate, Event, Input, Output, Rtc};

use crate::impair::{Fate, Impair};
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
    /// 캡처 시각을 복원하는 데 쓴 SR(그 프레임 직전에 받은 마지막 SR)의 `sender_packet_count`
    pub sr_packets: Option<u32>,
    /// 그 SR의 (SSRC, `sender_packet_count`, `sender_octet_count`). SR 기록(`SrRecord`)과 맞춰 볼 때 쓴다.
    pub sr_key: Option<(u32, u32, u32)>,
    /// 시청 쪽 str0m이 이 프레임을 내놓은 시각(캡처 시각과 비교해 화면→화면 지연을 본다)
    pub arrived: SystemTime,
    /// 키프레임인지(`MediaData::is_keyframe`). 오디오는 false.
    pub keyframe: bool,
    /// 앞서 내놓은 프레임에서 이어지는지(`MediaData::contiguous`). false면 시청 쪽 str0m이 앞 프레임을 잃었다고 본다.
    pub contiguous: bool,
}

/// 시청 쪽 str0m의 비디오 수신 통계(`ViewerOptions::stats`를 켠 시청자만, 누적값).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ingress {
    pub packets: u64,
    pub bytes: u64,
    /// 시청 쪽이 서버에 보낸 NACK(재전송 요청) 수
    pub nacks: u64,
    /// 시청 쪽이 서버에 보낸 PLI 수
    pub plis: u64,
    pub firs: u64,
}

/// `PendingViewer::with_options`의 설정.
#[derive(Debug, Clone, Copy)]
pub struct ViewerOptions {
    /// 받은 모든 RTCP SR을 기록한다(`TestViewer::sender_reports`).
    pub log_sr: bool,
    /// 시청 쪽 수신 통계(NACK·PLI 수)를 0.25초마다 받는다(`TestViewer::ingress`).
    pub stats: bool,
    /// 오디오 RED를 offer에 싣는다(기본 true). false면 RED 없는 Opus만 제안한다(RED를 모르는 시청자 흉내).
    pub red: bool,
}

impl Default for ViewerOptions {
    fn default() -> Self {
        ViewerOptions {
            log_sr: false,
            stats: false,
            red: true,
        }
    }
}

/// 시청자가 받은 RTCP SR 하나(`watch_logged`로 시작한 시청자만 기록한다).
#[derive(Debug, Clone, Copy)]
pub struct SrRecord {
    pub ssrc: u32,
    /// 서버가 SR을 만든 시각(SR의 NTP)
    pub ntp: SystemTime,
    pub packets: u32,
    pub octets: u32,
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
    srs: Arc<Mutex<Vec<SrRecord>>>,
    impair: Impair,
    ingress: Arc<Mutex<Ingress>>,
    pli_wanted: Arc<AtomicU32>,
}

impl TestViewer {
    /// 서버에서 받는 패킷의 손실 주입 손잡이(impair.rs).
    pub fn impair(&self) -> &Impair {
        &self.impair
    }

    /// 시청 쪽 수신 통계의 마지막 값(`ViewerOptions::stats`가 아니면 0).
    pub fn ingress(&self) -> Ingress {
        *self.ingress.lock().unwrap()
    }

    /// 시청자 str0m이 서버로 RTCP PLI를 보내게 한다(다음 타임아웃에 나간다. 비디오가 한 번도 안 왔으면 보낼 수 없어 무시).
    pub fn request_keyframe(&self) {
        self.pli_wanted.fetch_add(1, Ordering::Relaxed);
    }

    /// 지금까지 받은 SR(`watch_logged`가 아니면 비어 있다).
    pub fn sender_reports(&self) -> Vec<SrRecord> {
        self.srs.lock().unwrap().clone()
    }

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
    impair: Impair,
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
    /// `log_sr`이면 받은 모든 RTCP SR을 기록한다(`TestViewer::sender_reports`).
    pub fn with_sr_log(server_ip: IpAddr, log_sr: bool) -> Self {
        Self::with_options(
            server_ip,
            ViewerOptions {
                log_sr,
                ..Default::default()
            },
        )
    }

    pub fn with_options(server_ip: IpAddr, options: ViewerOptions) -> Self {
        let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
        let local = SocketAddr::new(
            route_local_ip(server_ip),
            socket.local_addr().unwrap().port(),
        );
        let mut rtc = Rtc::builder()
            .clear_codecs()
            .enable_opus(true, options.red)
            .set_red_distances(&AUDIO_RED_DISTANCES)
            .enable_h264(true)
            .enable_av1(true)
            .set_rtcp_report_interval_audio(SR_INTERVAL)
            .set_rtcp_report_interval_video(SR_INTERVAL)
            .enable_raw_packets(options.log_sr)
            .set_stats_interval(options.stats.then_some(Duration::from_millis(250)))
            .build(Instant::now());
        rtc.add_local_candidate(Candidate::host(local, "udp").unwrap());
        let mut api = rtc.sdp_api();
        api.add_media(MediaKind::Video, Direction::RecvOnly, None, None, None);
        api.add_media(MediaKind::Audio, Direction::RecvOnly, None, None, None);
        let (offer, pending) = api.apply().unwrap();
        Self {
            impair: Impair::new(),
            rtc,
            pending,
            socket,
            local,
            offer: offer.to_sdp_string(),
        }
    }

    pub fn start(self, answer: &str) -> TestViewer {
        let PendingViewer {
            impair,
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
        let srs: Arc<Mutex<Vec<SrRecord>>> = Arc::default();
        let thread_srs = srs.clone();
        impair.set_sdp(answer);
        let thread_impair = impair.clone();
        let ingress: Arc<Mutex<Ingress>> = Arc::default();
        let thread_ingress = ingress.clone();
        let pli_wanted = Arc::new(AtomicU32::new(0));
        let thread_pli = pli_wanted.clone();
        let thread = thread::spawn(move || {
            let mut buf = vec![0u8; 2048];
            let mut video_mid = None;
            let mut per_mid: std::collections::HashMap<str0m::media::Mid, Ingress> =
                Default::default();
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
                        Ok(Output::Event(Event::MediaAdded(m))) => {
                            if m.kind == MediaKind::Video {
                                video_mid = Some(m.mid);
                            }
                        }
                        Ok(Output::Event(Event::MediaIngressStats(s))) => {
                            // 미디어(mid)마다 마지막 값을 두고 합친다(오디오는 NACK·PLI가 0이라 비디오 값이 그대로다).
                            per_mid.insert(
                                s.mid,
                                Ingress {
                                    packets: s.packets,
                                    bytes: s.bytes,
                                    nacks: s.nacks,
                                    plis: s.plis,
                                    firs: s.firs,
                                },
                            );
                            *thread_ingress.lock().unwrap() =
                                per_mid.values().fold(Ingress::default(), |a, b| Ingress {
                                    packets: a.packets + b.packets,
                                    bytes: a.bytes + b.bytes,
                                    nacks: a.nacks + b.nacks,
                                    plis: a.plis + b.plis,
                                    firs: a.firs + b.firs,
                                });
                        }
                        Ok(Output::Event(Event::RawPacket(p))) => {
                            if let RawPacket::RtcpRx(Rtcp::SenderReport(sr)) = *p {
                                let i = sr.sender_info;
                                thread_srs.lock().unwrap().push(SrRecord {
                                    ssrc: *i.ssrc,
                                    ntp: i.ntp_time,
                                    packets: i.sender_packet_count,
                                    octets: i.sender_octet_count,
                                });
                            }
                        }
                        Ok(Output::Event(Event::MediaData(d))) => {
                            let kind = if d.params.spec().codec == Codec::Opus {
                                MediaKind::Audio
                            } else {
                                MediaKind::Video
                            };
                            if kind == MediaKind::Video {
                                video_mid = Some(d.mid);
                            }
                            let sr_packets = d.last_sender_info.map(|sr| sr.sender_packet_count);
                            let sr_key = d.last_sender_info.map(|sr| {
                                (*sr.ssrc, sr.sender_packet_count, sr.sender_octet_count)
                            });
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
                                sr_packets,
                                sr_key,
                                arrived: SystemTime::now(),
                                keyframe: d.is_keyframe(),
                                contiguous: d.contiguous,
                            });
                        }
                        Ok(Output::Event(_)) => {}
                        Err(_) => return,
                    }
                };
                // 시험이 요청한 키프레임(RTCP PLI)은 다음 타임아웃에 나간다.
                if thread_pli.swap(0, Ordering::Relaxed) > 0
                    && let Some(mid) = video_mid
                    && let Some(mut w) = rtc.writer(mid)
                {
                    let _ = w.request_keyframe(None, KeyframeRequestKind::Pli);
                }
                // 붙잡았던 패킷을 내보낼 때가 됐으면 str0m에 넘긴다(받은 패킷이 없어도).
                for (source, data) in thread_impair.release() {
                    if let Ok(contents) = data.as_slice().try_into() {
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
                let wait = next
                    .saturating_duration_since(Instant::now())
                    .clamp(Duration::from_millis(1), Duration::from_millis(20));
                let _ = socket.set_read_timeout(Some(wait));
                match socket.recv_from(&mut buf) {
                    Ok((n, source)) => {
                        if thread_impair.judge(source, &buf[..n]) == Fate::Pass
                            && let Ok(contents) = buf[..n].try_into()
                        {
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
            srs,
            impair,
            ingress,
            pli_wanted,
        }
    }
}
