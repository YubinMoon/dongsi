//! str0m을 직접 구동하는 시험용 방송자. 실제 송출(`dongsi_net::publish::Publisher`)에는 없는 "네트워크
//! 끊김" 손잡이(`set_blackout`)가 있어서, 방송자가 잠시 안 들리는 상황을 만든다.
//!
//! 실제 송출과 같은 구성이다: 보내기 전용 비디오(H.264)+오디오(Opus), RTCP SR 0.5초, 비디오 RTP는
//! `캡처 - epoch`(epoch = 시작 - 10초). 연결(`Connected`)되면 20ms마다 오디오, 40ms마다 비디오를 스스로 보낸다.

use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use dongsi_core::VideoCodec;
use dongsi_net::publish::SR_INTERVAL;
use str0m::change::{SdpAnswer, SdpPendingOffer};
use str0m::format::Codec;
use str0m::media::{Direction, Frequency, MediaKind, MediaTime, Mid};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};

use crate::media::{payload, video_frame};
use crate::viewer::route_local_ip;

pub struct PendingRawPublisher {
    rtc: Rtc,
    pending: SdpPendingOffer,
    socket: UdpSocket,
    local: SocketAddr,
    video_mid: Mid,
    audio_mid: Mid,
    pub offer: String,
}

/// 끊김 동안 버린 패킷 수.
#[derive(Debug, Clone, Copy, Default)]
pub struct Dropped {
    pub sent: u64,
    pub received: u64,
}

pub struct RawPublisher {
    stop: Arc<AtomicBool>,
    blackout: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    /// RTC가 스스로 끝났는지(예: ICE 끊김 판정).
    dead: Arc<AtomicBool>,
    dropped_out: Arc<AtomicU64>,
    dropped_in: Arc<AtomicU64>,
    /// 지금까지 만든 미디어 tick 수(다음에 만들 tick 번호).
    ticks: Arc<AtomicU32>,
    thread: Option<thread::JoinHandle<()>>,
}

impl PendingRawPublisher {
    pub fn new(server_ip: std::net::IpAddr) -> Self {
        let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
        let local = SocketAddr::new(
            route_local_ip(server_ip),
            socket.local_addr().unwrap().port(),
        );
        let mut rtc = Rtc::builder()
            .clear_codecs()
            .enable_opus(true, false)
            .enable_h264(true)
            .enable_av1(true)
            .set_rtcp_report_interval_audio(SR_INTERVAL)
            .set_rtcp_report_interval_video(SR_INTERVAL)
            .set_send_buffer_video(4000)
            .build(Instant::now());
        rtc.add_local_candidate(Candidate::host(local, "udp").unwrap());
        let mut api = rtc.sdp_api();
        let video_mid = api.add_media(
            MediaKind::Video,
            Direction::SendOnly,
            Some("dongsi".into()),
            Some("video".into()),
            None,
        );
        let audio_mid = api.add_media(
            MediaKind::Audio,
            Direction::SendOnly,
            Some("dongsi".into()),
            Some("audio".into()),
            None,
        );
        let (offer, pending) = api.apply().unwrap();
        Self {
            rtc,
            pending,
            socket,
            local,
            video_mid,
            audio_mid,
            offer: offer.to_sdp_string(),
        }
    }

    /// 서버의 answer를 받아 시작한다. `tag`는 페이로드에 넣는 방송자 표시.
    pub fn start(self, answer: &str, tag: u32) -> RawPublisher {
        let PendingRawPublisher {
            mut rtc,
            pending,
            socket,
            local,
            video_mid,
            audio_mid,
            ..
        } = self;
        rtc.sdp_api()
            .accept_answer(pending, SdpAnswer::from_sdp_string(answer).unwrap())
            .unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let blackout = Arc::new(AtomicBool::new(false));
        let connected = Arc::new(AtomicBool::new(false));
        let dead = Arc::new(AtomicBool::new(false));
        let dropped_out = Arc::new(AtomicU64::new(0));
        let dropped_in = Arc::new(AtomicU64::new(0));
        let ticks = Arc::new(AtomicU32::new(0));
        let flags = (
            stop.clone(),
            blackout.clone(),
            connected.clone(),
            dead.clone(),
            dropped_out.clone(),
            dropped_in.clone(),
            ticks.clone(),
        );
        let thread = thread::spawn(move || {
            let (stop, blackout, connected, dead, dropped_out, dropped_in, ticks) = flags;
            let epoch = Instant::now() - Duration::from_secs(10);
            let mut media_origin: Option<Instant> = None;
            let mut tick = 0u32;
            let mut buf = vec![0u8; 2048];
            while !stop.load(Ordering::Relaxed) {
                if !rtc.is_alive() {
                    dead.store(true, Ordering::Relaxed);
                    return;
                }
                // 미디어: 연결된 뒤부터 20ms마다 오디오, 40ms마다 비디오.
                if let Some(origin) = media_origin {
                    while origin + Duration::from_millis(20 * u64::from(tick)) <= Instant::now() {
                        let at = origin + Duration::from_millis(20 * u64::from(tick));
                        write_audio(&mut rtc, audio_mid, tick, tag, at);
                        if tick.is_multiple_of(2) {
                            write_video(&mut rtc, video_mid, tick, tag, at, epoch);
                        }
                        tick += 1;
                        ticks.store(tick, Ordering::Relaxed);
                    }
                }
                let next = loop {
                    match rtc.poll_output() {
                        Ok(Output::Timeout(t)) => break t,
                        Ok(Output::Transmit(t)) => {
                            if blackout.load(Ordering::Relaxed) {
                                dropped_out.fetch_add(1, Ordering::Relaxed);
                            } else {
                                let _ = socket.send_to(&t.contents, t.destination);
                            }
                        }
                        Ok(Output::Event(Event::Connected)) => {
                            connected.store(true, Ordering::Relaxed);
                            media_origin.get_or_insert_with(Instant::now);
                        }
                        Ok(Output::Event(_)) => {}
                        Err(_) => {
                            dead.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                };
                let mut wait = next
                    .saturating_duration_since(Instant::now())
                    .clamp(Duration::from_millis(1), Duration::from_millis(10));
                if let Some(origin) = media_origin {
                    let due = (origin + Duration::from_millis(20 * u64::from(tick)))
                        .saturating_duration_since(Instant::now());
                    wait = wait.min(due.max(Duration::from_millis(1)));
                }
                let _ = socket.set_read_timeout(Some(wait));
                match socket.recv_from(&mut buf) {
                    Ok((n, source)) => {
                        if blackout.load(Ordering::Relaxed) {
                            dropped_in.fetch_add(1, Ordering::Relaxed);
                        } else if let Ok(contents) = buf[..n].try_into() {
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
        RawPublisher {
            stop,
            blackout,
            connected,
            dead,
            dropped_out,
            dropped_in,
            ticks,
            thread: Some(thread),
        }
    }
}

fn write_audio(rtc: &mut Rtc, mid: Mid, tick: u32, tag: u32, at: Instant) {
    let Some(writer) = rtc.writer(mid) else {
        return;
    };
    let Some(pt) = writer
        .payload_params()
        .find(|p| p.spec().codec == Codec::Opus)
        .map(|p| p.pt())
    else {
        return;
    };
    let rtp = MediaTime::new(u64::from(tick) * 960, Frequency::FORTY_EIGHT_KHZ);
    let _ = writer.write(pt, at, rtp, payload(tick, tag, 80));
}

fn write_video(rtc: &mut Rtc, mid: Mid, tick: u32, tag: u32, at: Instant, epoch: Instant) {
    let Some(writer) = rtc.writer(mid) else {
        return;
    };
    let Some(pt) = writer
        .payload_params()
        .find(|p| p.spec().codec == Codec::H264)
        .map(|p| p.pt())
    else {
        return;
    };
    let since = at.saturating_duration_since(epoch);
    let rtp = MediaTime::from_90khz((since.as_micros() as u64) * 9 / 100);
    let _ = writer.write(pt, at, rtp, video_frame(VideoCodec::H264, tick, tag));
}

impl RawPublisher {
    /// true면 보내는 패킷도 받는 패킷도 모두 버린다(네트워크가 완전히 끊긴 것처럼).
    pub fn set_blackout(&self, on: bool) {
        self.blackout.store(on, Ordering::Relaxed);
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// 이쪽 RTC가 스스로 끝났는지.
    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Relaxed)
    }

    /// 다음에 만들 미디어 tick 번호. 이 값 이상의 tick을 가진 프레임은 이 호출 뒤에 만들어진다.
    pub fn tick(&self) -> u32 {
        self.ticks.load(Ordering::Relaxed)
    }

    pub fn dropped(&self) -> Dropped {
        Dropped {
            sent: self.dropped_out.load(Ordering::Relaxed),
            received: self.dropped_in.load(Ordering::Relaxed),
        }
    }

    pub async fn wait_connected(&self, within: Duration) {
        let deadline = Instant::now() + within;
        while !self.is_connected() {
            assert!(
                Instant::now() < deadline,
                "시험용 방송자의 미디어 연결이 {within:?} 안에 되지 않았습니다"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

impl Drop for RawPublisher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
