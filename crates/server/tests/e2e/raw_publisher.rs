//! str0m을 직접 구동하는 시험용 방송자. 실제 송출(`dongsi_net::publish::Publisher`)에는 없는 손잡이가 있다.
//!
//! - "네트워크 끊김"(`set_blackout`): 방송자가 잠시 안 들리는 상황.
//! - 손실 주입(`impair`, impair.rs, D-087): 보내는 패킷을 소켓 앞에서 버리거나 붙잡는다(비디오 원본·RTX를 따로 골라
//!   복구 불가능한 손실을 만든다). 받는 패킷은 건드리지 않는다(`set_blackout`만 받기를 막는다).
//! - 인코더 흉내(`RawOptions::encoder`, D-087): 기본은 비디오가 모두 키프레임이다(기존 시험 그대로). 켜면 첫 프레임만
//!   키프레임이고 뒤는 델타 프레임이며, 서버의 키프레임 요청(PLI/FIR, `Event::KeyframeRequest`)이 오면 다음 비디오를
//!   키프레임으로 보낸다. `set_answer_keyframe_requests(false)`면 요청에 답하지 않는다(키프레임 대기가 이어지는 상황).
//!   받은 요청은 도착 시각과 함께 기록한다(`keyframe_requests`). 프레임을 키워 패킷 여러 개짜리로 만들 수 있다
//!   (`Encoder::frame_bytes`).
//!
//! 오디오 RED(D-087): 기본으로 RED(RFC 2198, 거리 `AUDIO_RED_DISTANCES`)를 켜고 offer에 싣는다. `PendingRawPublisher::with_red`로
//! 끄면 RED 없는 Opus만 제안한다(RED를 모르는 방송자 흉내, SFU가 일반 Opus로 맞추는지 본다).
//!
//! 실제 송출과 같은 구성이다: 보내기 전용 비디오(H.264)+오디오(Opus), RTCP SR 0.5초, 비디오 RTP는
//! `캡처 - epoch`(epoch = 시작 - 10초). 연결(`Connected`)되면 20ms마다 오디오, 40ms마다 비디오를 스스로 보낸다.

use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use dongsi_core::media::AUDIO_RED_DISTANCES;
use dongsi_net::publish::SR_INTERVAL;
use str0m::change::{SdpAnswer, SdpPendingOffer};
use str0m::format::Codec;
use str0m::media::{
    Direction, Frequency, KeyframeRequestKind, MediaKind, MediaTime, Mid, Simulcast, SimulcastLayer,
};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};

use crate::impair::{Fate, Impair};
use crate::media::{delta_frame, payload, video_frame};
use crate::viewer::route_local_ip;

/// rid 둘(`h`·`l`)로 simulcast를 제안하는 방송자 offer(보내기 전용 비디오 + 오디오). 서버는 이런 offer를 받지 않는다
/// (D-104: 비디오 트랙 하나).
pub fn simulcast_offer(server_ip: std::net::IpAddr) -> String {
    let local = SocketAddr::new(route_local_ip(server_ip), 9);
    let mut rtc = Rtc::builder()
        .clear_codecs()
        .enable_opus(true, true)
        .set_red_distances(&AUDIO_RED_DISTANCES)
        .enable_h264(true)
        .build(Instant::now());
    rtc.add_local_candidate(Candidate::host(local, "udp").unwrap());
    let mut simulcast = Simulcast::new();
    for rid in ["h", "l"] {
        simulcast.add_send_layer(SimulcastLayer::new(rid));
    }
    let mut api = rtc.sdp_api();
    api.add_media(
        MediaKind::Video,
        Direction::SendOnly,
        Some("dongsi".into()),
        Some("video".into()),
        Some(simulcast),
    );
    api.add_media(
        MediaKind::Audio,
        Direction::SendOnly,
        Some("dongsi".into()),
        Some("audio".into()),
        None,
    );
    let (offer, _pending) = api.apply().unwrap();
    offer.to_sdp_string()
}

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

/// 인코더 흉내 설정(모듈 설명).
#[derive(Debug, Clone, Copy)]
pub struct Encoder {
    /// 비디오 프레임 페이로드의 대략적인 크기(바이트). `media.rs`의 기본(3000, RTP 패킷 3개쯤)보다 크면 H.264 NAL 끝에
    /// 0xAA 채움을 더한다(0 바이트가 없어 시작 코드로 잘못 나뉘지 않는다). 패킷 하나는 1200바이트쯤이다.
    pub frame_bytes: usize,
}

/// `PendingRawPublisher::start_with`의 설정. 기본값은 `start`와 같다(모든 비디오가 키프레임).
#[derive(Debug, Clone, Default)]
pub struct RawOptions {
    pub encoder: Option<Encoder>,
}

/// 서버에서 받은 키프레임 요청 하나.
#[derive(Debug, Clone, Copy)]
pub struct KeyframeRequestRecord {
    pub at: Instant,
    pub kind: KeyframeRequestKind,
}

/// 인코더 흉내가 키프레임을 보낸 기록(tick, 시각).
#[derive(Debug, Clone, Copy)]
pub struct KeyframeSent {
    pub tick: u32,
    pub at: Instant,
}

pub struct RawPublisher {
    stop: Arc<AtomicBool>,
    impair: Impair,
    answer_pli: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<KeyframeRequestRecord>>>,
    keys_sent: Arc<Mutex<Vec<KeyframeSent>>>,
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
    /// 오디오 RED를 켠 방송자(기본).
    pub fn new(server_ip: std::net::IpAddr) -> Self {
        Self::with_red(server_ip, true)
    }

    /// `red`가 false면 offer에 RED를 싣지 않는다.
    pub fn with_red(server_ip: std::net::IpAddr, red: bool) -> Self {
        let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
        let local = SocketAddr::new(
            route_local_ip(server_ip),
            socket.local_addr().unwrap().port(),
        );
        let mut rtc = Rtc::builder()
            .clear_codecs()
            .enable_opus(true, red)
            .set_red_distances(&AUDIO_RED_DISTANCES)
            .enable_h264(true)
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
        self.start_with(answer, tag, RawOptions::default())
    }

    /// `start`와 같고, 인코더 흉내 같은 설정을 준다.
    pub fn start_with(self, answer: &str, tag: u32, options: RawOptions) -> RawPublisher {
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
        let impair = Impair::new();
        impair.set_sdp(answer);
        let answer_pli = Arc::new(AtomicBool::new(true));
        let requests: Arc<Mutex<Vec<KeyframeRequestRecord>>> = Arc::default();
        let keys_sent: Arc<Mutex<Vec<KeyframeSent>>> = Arc::default();
        let thread_impair = impair.clone();
        let thread_answer_pli = answer_pli.clone();
        let thread_requests = requests.clone();
        let thread_keys_sent = keys_sent.clone();
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
            // 인코더 흉내: 첫 프레임과 서버가 요청한 뒤 첫 프레임이 키프레임이다.
            let mut want_key = true;
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
                            let key = match options.encoder {
                                None => true,
                                Some(_) => std::mem::take(&mut want_key),
                            };
                            if options.encoder.is_some() && key {
                                thread_keys_sent.lock().unwrap().push(KeyframeSent {
                                    tick,
                                    at: Instant::now(),
                                });
                            }
                            let pad = options.encoder.map_or(0, |e| e.frame_bytes);
                            write_video(
                                &mut rtc,
                                video_mid,
                                VideoFrame { tick, key, pad },
                                tag,
                                at,
                                epoch,
                            );
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
                                for (to, data) in thread_impair.release() {
                                    let _ = socket.send_to(&data, to);
                                }
                                if thread_impair.judge(t.destination, &t.contents) == Fate::Pass {
                                    let _ = socket.send_to(&t.contents, t.destination);
                                }
                            }
                        }
                        Ok(Output::Event(Event::Connected)) => {
                            connected.store(true, Ordering::Relaxed);
                            media_origin.get_or_insert_with(Instant::now);
                        }
                        Ok(Output::Event(Event::KeyframeRequest(r))) => {
                            thread_requests.lock().unwrap().push(KeyframeRequestRecord {
                                at: Instant::now(),
                                kind: r.kind,
                            });
                            if thread_answer_pli.load(Ordering::Relaxed) {
                                want_key = true;
                            }
                        }
                        Ok(Output::Event(_)) => {}
                        Err(_) => {
                            dead.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                };
                // 붙잡았던 패킷을 내보낼 때가 됐으면 내보낸다(보낼 패킷이 없어도).
                if !blackout.load(Ordering::Relaxed) {
                    for (to, data) in thread_impair.release() {
                        let _ = socket.send_to(&data, to);
                    }
                }
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
            impair,
            answer_pli,
            requests,
            keys_sent,
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

/// 만들 비디오 프레임 하나.
struct VideoFrame {
    tick: u32,
    key: bool,
    /// 페이로드를 이 크기까지 키운다(0이면 `media.rs` 기본 크기).
    pad: usize,
}

/// H.264 프레임을 `target` 바이트 근처까지 키운다(NAL 끝에 0xAA 채움. 0 바이트가 없어 시작 코드로 나뉘지 않는다).
fn grow(mut frame: Vec<u8>, target: usize) -> Vec<u8> {
    if frame.len() < target {
        frame.resize(target, 0xAA);
    }
    frame
}

fn write_video(rtc: &mut Rtc, mid: Mid, f: VideoFrame, tag: u32, at: Instant, epoch: Instant) {
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
    let frame = if f.key {
        video_frame(f.tick, tag)
    } else {
        delta_frame(f.tick, tag)
    };
    let _ = writer.write(pt, at, rtp, grow(frame, f.pad));
}

impl RawPublisher {
    /// true면 보내는 패킷도 받는 패킷도 모두 버린다(네트워크가 완전히 끊긴 것처럼).
    pub fn set_blackout(&self, on: bool) {
        self.blackout.store(on, Ordering::Relaxed);
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// 보내는 패킷의 손실 주입 손잡이(impair.rs).
    pub fn impair(&self) -> &Impair {
        &self.impair
    }

    /// false면 서버의 키프레임 요청에 답하지 않는다(요청은 기록한다). 인코더 흉내에서만 뜻이 있다. 기본 true.
    pub fn set_answer_keyframe_requests(&self, on: bool) {
        self.answer_pli.store(on, Ordering::Relaxed);
    }

    /// 서버에서 받은 키프레임 요청(PLI/FIR)을 도착 순서대로.
    pub fn keyframe_requests(&self) -> Vec<KeyframeRequestRecord> {
        self.requests.lock().unwrap().clone()
    }

    /// 인코더 흉내가 키프레임을 보낸 기록.
    pub fn keyframes_sent(&self) -> Vec<KeyframeSent> {
        self.keys_sent.lock().unwrap().clone()
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
