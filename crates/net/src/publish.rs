//! WebRTC 송출 클라이언트(str0m).
//!
//! 방송자 앱이 인코딩한 프레임을 서버(SFU)로 보낸다. 서버는 ICE-lite이고 후보 주소 하나를 알려 준다.
//!
//! 스레드 구성:
//! - 소켓 읽기 스레드: UDP 패킷을 받아 명령 채널로 넘긴다.
//! - RTC 스레드: 명령 채널(프레임, 패킷)을 받아 str0m을 구동한다. 채널이 깨우므로 프레임이 들어오면
//!   바로 보낸다(지연을 줄이려고 소켓 타임아웃에 기대지 않는다).
//!
//! 혼잡 대응(D-081): str0m BWE(GoogCC, TWCC 피드백)와 pacer를 켠다. 추정값과 서버가 보낸 REMB(내장 서버는
//! 가장 느린 시청자의 추정값)로 [`RateAdapter`]가 비디오 목표 비트레이트를 정하고, 앱이
//! [`Publisher::video_target_kbps`]를 인코더에 준다.
//!
//! 오디오·비디오 동기화: 두 트랙 모두 캡처 시각(QPC 기반 `Instant`)을 `wallclock`으로 넘긴다.
//! str0m이 이 값으로 RTCP SR(NTP↔RTP)을 만들고, 서버가 SR로 캡처 시각을 복원해 시청자에게
//! 같은 기준으로 다시 보낸다.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use dongsi_core::VideoCodec;
use dongsi_core::priority::{ThreadRole, boost_current_thread};
use str0m::bwe::{Bitrate, BweKind};
use str0m::change::{SdpAnswer, SdpPendingOffer};
use str0m::format::Codec;
use str0m::media::{Direction, MediaKind, MediaTime, Mid};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tracing::{debug, info, warn};

use crate::NetError;
use crate::rate::{RateAdapter, RateConfig};

const SOCKET_BUFFER: usize = 8 * 1024 * 1024;
const QUEUE_LEN: usize = 256;
/// RTCP SR 간격. 시청 쪽은 SR이 와야 오디오·비디오를 맞출 수 있으므로 기본값(오디오 5초)보다 짧게 한다.
pub const SR_INTERVAL: Duration = Duration::from_millis(500);

/// 송출 상태.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishStats {
    pub connected: bool,
    pub closed: Option<String>,
    pub video_kbps: f32,
    pub audio_kbps: f32,
    pub rtt_ms: Option<f32>,
    pub loss: Option<f32>,
    pub nacks: u64,
    pub keyframe_requests: u64,
    pub dropped_frames: u64,
    /// 비디오 목표 비트레이트(kbps, D-081). 상한(`max_kbps`)보다 낮으면 네트워크에 맞춰 낮춘 것이다.
    pub target_kbps: u32,
    /// 비디오 상한: 화질 설정의 비트레이트(kbps).
    pub max_kbps: u32,
    /// 쓸 수 있는 전체 대역폭 추정(kbps): BWE(TWCC)와 서버가 보낸 REMB 중 작은 값. 아직 없으면 `None`.
    pub estimate_kbps: Option<u32>,
    /// 서버가 보낸 REMB(내장 서버: 가장 느린 시청자의 추정값, kbps). 3초 안에 받은 것만.
    pub remb_kbps: Option<u32>,
    /// 네트워크 때문에 상한보다 낮추고 있다.
    pub limited: bool,
}

enum Cmd {
    Packet(Vec<u8>, SocketAddr, Instant),
    Video {
        codec: VideoCodec,
        data: Vec<u8>,
        capture: Instant,
    },
    Audio {
        data: Vec<u8>,
        capture: Instant,
        sample_index: u64,
    },
    /// BWE 설정: `reset`이면 그 시작값(kbps)에서 다시 추정하고, `desired`(kbps)까지 찾아본다.
    Bwe {
        reset: Option<u32>,
        desired: u32,
    },
    Close,
}

struct Shared {
    stats: Mutex<PublishStats>,
    rate: Mutex<RateAdapter>,
    keyframe_wanted: AtomicBool,
    stop: AtomicBool,
}

/// SDP 응답을 기다리는 송출 준비 상태.
pub struct PendingPublisher {
    rtc: Rtc,
    pending: SdpPendingOffer,
    socket: UdpSocket,
    /// 후보로 알린 로컬 주소. 받은 패킷의 목적지로 넘긴다.
    local: SocketAddr,
    video_mid: Mid,
    audio_mid: Option<Mid>,
    rate: RateAdapter,
    /// 서버에 보낼 SDP offer.
    pub offer: String,
}

impl PendingPublisher {
    /// `server_ip`는 시그널링에 쓴 서버 IP. 이 IP로 가는 경로의 로컬 주소를 후보로 쓴다.
    ///
    /// 소켓은 모든 주소(0.0.0.0)에 연다. 서버가 알려 주는 후보가 접속 주소와 다를 수 있기 때문이다
    /// (같은 PC에서 루프백으로 접속하면 서버는 LAN 주소를 후보로 준다).
    ///
    /// `rate`는 비트레이트 상한과 오디오다(`audio_kbps`가 `None`이면 오디오 트랙을 만들지 않는다).
    pub fn new(server_ip: IpAddr, rate: RateConfig) -> Result<Self, NetError> {
        let has_audio = rate.audio_kbps.is_some();
        let rate = RateAdapter::new(rate);
        let local_ip = route_local_ip(server_ip)?;
        let bind_ip: IpAddr = if local_ip.is_ipv4() {
            std::net::Ipv4Addr::UNSPECIFIED.into()
        } else {
            std::net::Ipv6Addr::UNSPECIFIED.into()
        };
        let socket = UdpSocket::bind(SocketAddr::new(bind_ip, 0))?;
        tune_socket(&socket);
        let local = SocketAddr::new(local_ip, socket.local_addr()?.port());

        let mut rtc = Rtc::builder()
            .clear_codecs()
            .enable_opus(true, false)
            .enable_h264(true)
            .enable_av1(true)
            .set_rtcp_report_interval_audio(SR_INTERVAL)
            .set_rtcp_report_interval_video(SR_INTERVAL)
            .set_send_buffer_video(4000)
            .set_stats_interval(Some(Duration::from_secs(1)))
            .enable_bwe(Some(kbps(rate.initial_estimate_kbps())))
            .build(Instant::now());
        let candidate = Candidate::host(local, "udp")
            .map_err(|e| NetError::Rtc(format!("후보 주소 오류: {e}")))?;
        rtc.add_local_candidate(candidate);

        let mut api = rtc.sdp_api();
        let video_mid = api.add_media(
            MediaKind::Video,
            Direction::SendOnly,
            Some("dongsi".into()),
            Some("video".into()),
            None,
        );
        let audio_mid = has_audio.then(|| {
            api.add_media(
                MediaKind::Audio,
                Direction::SendOnly,
                Some("dongsi".into()),
                Some("audio".into()),
                None,
            )
        });
        let (offer, pending) = api
            .apply()
            .ok_or_else(|| NetError::Rtc("SDP offer를 만들지 못했습니다".into()))?;
        Ok(Self {
            rtc,
            pending,
            socket,
            local,
            video_mid,
            audio_mid,
            rate,
            offer: offer.to_sdp_string(),
        })
    }

    /// 서버의 SDP answer를 받아 송출을 시작한다.
    pub fn start(self, answer: &str) -> Result<Publisher, NetError> {
        let PendingPublisher {
            mut rtc,
            pending,
            socket,
            local: local_candidate,
            video_mid,
            audio_mid,
            rate,
            ..
        } = self;
        let answer = SdpAnswer::from_sdp_string(answer)
            .map_err(|e| NetError::Rtc(format!("SDP answer 형식 오류: {e}")))?;
        rtc.sdp_api()
            .accept_answer(pending, answer)
            .map_err(|e| NetError::Rtc(format!("SDP answer를 받지 못했습니다: {e}")))?;
        rtc.bwe()
            .set_bitrate(Bitrate::ZERO, kbps(rate.desired_kbps()));

        let shared = Arc::new(Shared {
            stats: Mutex::new(PublishStats {
                target_kbps: rate.target_kbps(),
                max_kbps: rate.max_video_kbps(),
                ..PublishStats::default()
            }),
            rate: Mutex::new(rate),
            keyframe_wanted: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        });
        let (tx, rx) = mpsc::sync_channel::<Cmd>(QUEUE_LEN);

        let reader_socket = socket.try_clone()?;
        reader_socket.set_read_timeout(Some(Duration::from_millis(200)))?;
        let reader_tx = tx.clone();
        let reader_shared = shared.clone();
        let reader = thread::Builder::new()
            .name("dongsi-pub-recv".into())
            .spawn(move || {
                let _boost = boost_current_thread(ThreadRole::Network);
                read_loop(reader_socket, reader_tx, reader_shared)
            })?;

        let local = local_candidate;
        let loop_shared = shared.clone();
        let runner = thread::Builder::new()
            .name("dongsi-pub-rtc".into())
            .spawn(move || {
                let _boost = boost_current_thread(ThreadRole::Network);
                let mut state = RtcLoop {
                    rtc,
                    socket,
                    local,
                    video_mid,
                    audio_mid,
                    shared: loop_shared,
                    epoch: Instant::now() - Duration::from_secs(10),
                    last_video_codec: None,
                    bytes: Default::default(),
                    trace: trace::publish(),
                };
                if let Some(sink) = state.trace {
                    sink.mark("publisher");
                }
                state.run(rx);
            })?;

        Ok(Publisher {
            tx,
            shared,
            threads: vec![runner, reader],
        })
    }
}

/// 송출 중인 연결.
pub struct Publisher {
    tx: SyncSender<Cmd>,
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl Publisher {
    /// 인코딩된 비디오 프레임을 보낸다. 대기열이 가득 차면 버리고 키프레임을 다시 요청하게 한다.
    pub fn send_video(&self, codec: VideoCodec, data: Vec<u8>, capture: Instant) {
        match self.tx.try_send(Cmd::Video {
            codec,
            data,
            capture,
        }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.shared.keyframe_wanted.store(true, Ordering::Relaxed);
                self.shared.stats.lock().unwrap().dropped_frames += 1;
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// Opus 패킷(20ms)을 보낸다. `sample_index`는 48kHz 기준 첫 샘플 번호.
    pub fn send_audio(&self, data: Vec<u8>, capture: Instant, sample_index: u64) {
        let _ = self.tx.try_send(Cmd::Audio {
            data,
            capture,
            sample_index,
        });
    }

    /// 비디오 목표 비트레이트(kbps, D-081). 인코더에 이 값을 준다. 네트워크가 충분하면 상한과 같다.
    pub fn video_target_kbps(&self) -> u32 {
        self.shared.rate.lock().unwrap().target_kbps()
    }

    /// 비디오 상한(화질 설정의 비트레이트)을 바꾸고 새 목표를 돌려준다(규칙은 [`RateAdapter::set_max_video`]).
    pub fn set_max_video_kbps(&self, max_kbps: u32) -> u32 {
        let (target, reset, desired) = {
            let mut r = self.shared.rate.lock().unwrap();
            let reset = r.set_max_video(max_kbps, Instant::now());
            (
                r.target_kbps(),
                reset.then(|| r.initial_estimate_kbps()),
                r.desired_kbps(),
            )
        };
        {
            let mut stats = self.shared.stats.lock().unwrap();
            stats.target_kbps = target;
            stats.max_kbps = max_kbps.max(crate::rate::MIN_VIDEO_KBPS);
            stats.limited = target < stats.max_kbps;
        }
        // 대기열이 가득 차도 잠깐 기다려 보낸다(설정은 버리면 안 된다). 송출이 끝났으면 무시한다.
        let _ = self.tx.send(Cmd::Bwe { reset, desired });
        target
    }

    /// 서버(시청자)가 키프레임을 요청했는지. 읽으면 초기화된다.
    pub fn take_keyframe_request(&self) -> bool {
        self.shared.keyframe_wanted.swap(false, Ordering::Relaxed)
    }

    pub fn stats(&self) -> PublishStats {
        self.shared.stats.lock().unwrap().clone()
    }

    pub fn is_closed(&self) -> bool {
        self.shared.stats.lock().unwrap().closed.is_some()
    }

    pub fn close(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        let _ = self.tx.try_send(Cmd::Close);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn read_loop(socket: UdpSocket, tx: SyncSender<Cmd>, shared: Arc<Shared>) {
    let mut buf = vec![0u8; 2048];
    while !shared.stop.load(Ordering::Relaxed) {
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                if tx
                    .send(Cmd::Packet(buf[..n].to_vec(), from, Instant::now()))
                    .is_err()
                {
                    break;
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            // Windows: 상대가 닫힌 포트로 보낸 뒤 ICMP 응답이 오면 ConnectionReset이 난다. 무시한다.
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
            Err(e) => {
                debug!("송출 소켓 읽기 오류: {e}");
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

#[derive(Default)]
struct ByteCounters {
    video: Option<(u64, Instant)>,
    audio: Option<(u64, Instant)>,
}

struct RtcLoop {
    rtc: Rtc,
    socket: UdpSocket,
    local: SocketAddr,
    video_mid: Mid,
    audio_mid: Option<Mid>,
    shared: Arc<Shared>,
    epoch: Instant,
    last_video_codec: Option<VideoCodec>,
    bytes: ByteCounters,
    /// 쓰기 기록(`DONGSI_MEDIA_TRACE`). 꺼져 있으면 `None`.
    trace: Option<&'static trace::Sink>,
}

impl RtcLoop {
    fn run(&mut self, rx: Receiver<Cmd>) {
        while let Some(timeout) = self.drive() {
            let wait = timeout
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(100));
            match rx.recv_timeout(wait) {
                Ok(cmd) => {
                    if !self.handle_cmd(cmd) {
                        break;
                    }
                    // 쌓인 명령을 한꺼번에 처리한다.
                    while let Ok(cmd) = rx.try_recv() {
                        if !self.handle_cmd(cmd) {
                            self.rtc.disconnect();
                            let _ = self.drive();
                            return;
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            // 기한이 지났으면 명령이 계속 와 있어도 타임아웃을 준다. BWE pacer는 타임아웃 때만 패킷을 내보내므로,
            // 명령(받은 패킷·미디어 쓰기)이 쉬지 않고 오면 미디어가 큐에 갇혀 늦어지다가 버려진다(2026-11-22 e2e를
            // 병렬로 돌릴 때 오디오가 300~500ms 몰려 오고 20~30% 빠졌다).
            let now = Instant::now();
            if now >= timeout {
                let _ = self.rtc.handle_input(Input::Timeout(now));
            }
        }
        self.rtc.disconnect();
        let _ = self.drive();
        if let Some(sink) = self.trace {
            sink.flush();
        }
        let mut stats = self.shared.stats.lock().unwrap();
        stats.connected = false;
        stats
            .closed
            .get_or_insert_with(|| "송출을 끝냈습니다".into());
    }

    /// false면 종료.
    fn handle_cmd(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::Close => return false,
            Cmd::Packet(data, source, at) => {
                let Ok(contents) = data.as_slice().try_into() else {
                    return true;
                };
                let input = Input::Receive(
                    at,
                    Receive {
                        proto: Protocol::Udp,
                        source,
                        destination: self.local,
                        contents,
                    },
                );
                if let Err(e) = self.rtc.handle_input(input) {
                    warn!("송출 RTC 입력 오류: {e}");
                    self.close_with(format!("연결 오류: {e}"));
                    return false;
                }
            }
            Cmd::Video {
                codec,
                data,
                capture,
            } => self.write_video(codec, data, capture),
            Cmd::Audio {
                data,
                capture,
                sample_index,
            } => self.write_audio(data, capture, sample_index),
            Cmd::Bwe { reset, desired } => {
                let mut bwe = self.rtc.bwe();
                if let Some(start) = reset {
                    bwe.reset(kbps(start));
                }
                bwe.set_bitrate(Bitrate::ZERO, kbps(desired));
            }
        }
        true
    }

    fn write_video(&mut self, codec: VideoCodec, data: Vec<u8>, capture: Instant) {
        let dequeued = self.trace.map(|_| Instant::now());
        let since = capture.saturating_duration_since(self.epoch);
        let rtp_value = (since.as_micros() as u64) * 9 / 100;
        let trace = self.trace;
        let log = |pt: Option<u8>, result: &str| {
            if let (Some(sink), Some(at)) = (trace, dequeued) {
                sink.write_row('v', &codec.to_string(), pt, rtp_value, capture, at, result);
            }
        };
        let Some(writer) = self.rtc.writer(self.video_mid) else {
            log(None, "no_writer");
            return;
        };
        let Some(pt) = pick_pt(writer.payload_params().map(|p| (p.pt(), p)), codec) else {
            if self.last_video_codec != Some(codec) {
                warn!("서버와 협상되지 않은 코덱입니다: {codec}");
            }
            self.last_video_codec = Some(codec);
            log(None, "no_pt");
            return;
        };
        self.last_video_codec = Some(codec);
        let rtp = MediaTime::from_90khz(rtp_value);
        let res = writer.write(pt, capture, rtp, data);
        if let Err(e) = &res {
            debug!("비디오 쓰기 실패: {e}");
        }
        log(Some(*pt), &trace::result_text(&res));
    }

    fn write_audio(&mut self, data: Vec<u8>, capture: Instant, sample_index: u64) {
        let Some(mid) = self.audio_mid else {
            return;
        };
        let dequeued = self.trace.map(|_| Instant::now());
        let trace = self.trace;
        let log = |pt: Option<u8>, result: &str| {
            if let (Some(sink), Some(at)) = (trace, dequeued) {
                sink.write_row('a', "opus", pt, sample_index, capture, at, result);
            }
        };
        let Some(writer) = self.rtc.writer(mid) else {
            log(None, "no_writer");
            return;
        };
        let Some(pt) = writer
            .payload_params()
            .find(|p| p.spec().codec == Codec::Opus)
            .map(|p| p.pt())
        else {
            log(None, "no_pt");
            return;
        };
        let rtp = MediaTime::new(sample_index, str0m::media::Frequency::FORTY_EIGHT_KHZ);
        let res = writer.write(pt, capture, rtp, data);
        if let Err(e) = &res {
            debug!("오디오 쓰기 실패: {e}");
        }
        log(Some(*pt), &trace::result_text(&res));
    }

    /// 출력을 모두 처리하고 다음 타임아웃을 돌려준다. 연결이 끝났으면 None.
    fn drive(&mut self) -> Option<Instant> {
        loop {
            if !self.rtc.is_alive() {
                self.close_with("연결이 끊어졌습니다".into());
                return None;
            }
            match self.rtc.poll_output() {
                Ok(Output::Timeout(t)) => {
                    self.update_rate(Instant::now());
                    return Some(t);
                }
                Ok(Output::Transmit(t)) => {
                    if let Err(e) = self.socket.send_to(&t.contents, t.destination) {
                        debug!("송신 실패: {e}");
                    }
                }
                Ok(Output::Event(e)) => self.handle_event(e),
                Err(e) => {
                    warn!("송출 RTC 오류: {e}");
                    self.close_with(format!("연결 오류: {e}"));
                    return None;
                }
            }
        }
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Connected => {
                info!("송출 연결됨");
                self.shared.stats.lock().unwrap().connected = true;
                self.shared.keyframe_wanted.store(true, Ordering::Relaxed);
            }
            Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                self.close_with("서버와 연결이 끊어졌습니다".into());
                self.rtc.disconnect();
            }
            Event::KeyframeRequest(_) => {
                self.shared.keyframe_wanted.store(true, Ordering::Relaxed);
                self.shared.stats.lock().unwrap().keyframe_requests += 1;
            }
            Event::EgressBitrateEstimate(BweKind::Twcc { estimate, .. }) => {
                self.shared.rate.lock().unwrap().on_twcc(to_kbps(estimate));
            }
            Event::EgressBitrateEstimate(BweKind::Remb { estimate, .. }) => {
                self.shared
                    .rate
                    .lock()
                    .unwrap()
                    .on_remb(to_kbps(estimate), Instant::now());
            }
            Event::MediaEgressStats(s) => {
                let mut stats = self.shared.stats.lock().unwrap();
                let is_video = s.mid == self.video_mid;
                let slot = if is_video {
                    &mut self.bytes.video
                } else {
                    &mut self.bytes.audio
                };
                if let Some((prev, at)) = *slot {
                    let secs = s.timestamp.saturating_duration_since(at).as_secs_f32();
                    if secs > 0.0 {
                        let kbps = (s.bytes.saturating_sub(prev) as f32 * 8.0) / secs / 1000.0;
                        if is_video {
                            stats.video_kbps = kbps;
                        } else {
                            stats.audio_kbps = kbps;
                        }
                    }
                }
                *slot = Some((s.bytes, s.timestamp));
                if is_video {
                    stats.rtt_ms = s.rtt.map(|d| d.as_secs_f32() * 1000.0);
                    stats.loss = s.loss;
                    stats.nacks = s.nacks;
                }
            }
            _ => {}
        }
    }

    /// 목표 비트레이트를 다시 정하고 통계에 적는다.
    fn update_rate(&mut self, now: Instant) {
        let mut r = self.shared.rate.lock().unwrap();
        let before = r.target_kbps();
        if let Some(target) = r.tick(now) {
            info!(
                from = before,
                to = target,
                twcc = ?r.twcc_kbps(),
                remb = ?r.remb_kbps(now),
                "네트워크에 맞춰 비디오 비트레이트 목표를 바꿉니다"
            );
        }
        let mut stats = self.shared.stats.lock().unwrap();
        stats.target_kbps = r.target_kbps();
        stats.max_kbps = r.max_video_kbps();
        stats.estimate_kbps = r.available_kbps(now);
        stats.remb_kbps = r.remb_kbps(now);
        stats.limited = r.limited();
    }

    fn close_with(&mut self, reason: String) {
        let mut stats = self.shared.stats.lock().unwrap();
        stats.connected = false;
        stats.closed.get_or_insert(reason);
    }
}

fn kbps(k: u32) -> Bitrate {
    Bitrate::kbps(k as u64)
}

fn to_kbps(b: Bitrate) -> u32 {
    (b.as_u64() / 1000).min(u32::MAX as u64) as u32
}

/// 코덱에 맞는 PT를 고른다. H.264는 High 프로파일(64xxxx), packetization-mode=1을 우선한다.
fn pick_pt<'a>(
    params: impl Iterator<Item = (str0m::media::Pt, &'a str0m::format::PayloadParams)>,
    codec: VideoCodec,
) -> Option<str0m::media::Pt> {
    let want = match codec {
        VideoCodec::H264 => Codec::H264,
        VideoCodec::Av1 => Codec::Av1,
    };
    params
        .filter(|(_, p)| p.spec().codec == want)
        .max_by_key(|(_, p)| {
            let f = &p.spec().format;
            let mode1 = f.packetization_mode == Some(1);
            let high = f.profile_level_id.is_some_and(|id| id >> 16 == 0x64);
            (mode1, high)
        })
        .map(|(pt, _)| pt)
}

/// 이 PC에서 `target`으로 갈 때 쓰는 로컬 IP.
///
/// 루프백이면 서버와 같은 규칙으로 이 PC의 LAN 주소를 쓴다(없으면 루프백).
fn route_local_ip(target: IpAddr) -> Result<IpAddr, NetError> {
    if target.is_loopback() {
        return Ok(crate::ifaces::primary_lan_ip()
            .map(IpAddr::V4)
            .unwrap_or(target));
    }
    let bind = if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let probe = UdpSocket::bind(bind)?;
    probe.connect(SocketAddr::new(target, 9))?;
    Ok(probe.local_addr()?.ip())
}

/// 소켓 버퍼를 키운다(4K 키프레임처럼 큰 프레임을 한 번에 보낼 때 버려지지 않게).
pub fn tune_socket(socket: &UdpSocket) {
    let sock = socket2::SockRef::from(socket);
    let _ = sock.set_send_buffer_size(SOCKET_BUFFER);
    let _ = sock.set_recv_buffer_size(SOCKET_BUFFER);
}

/// 송출 쓰기 기록(`DONGSI_MEDIA_TRACE=<폴더>`의 `publish.csv`). 기본은 꺼져 있다.
///
/// crates/net은 crates/media에 의존하지 않으므로 같은 환경 변수를 따로 읽는다. 형식과 쓰기 방식
/// (이어 쓰기, 비어 있을 때만 머리글, 기록 스레드가 버퍼링해 250ms마다 내보냄)은 media의 `trace`와 같다.
/// 캡처 시각은 `Instant`라 UNIX epoch µs로 바꿔 적는다(처음 쓸 때 잡은 (`Instant`, 벽시계) 한 쌍 기준).
pub(crate) mod trace {
    use std::fs::{self, File, OpenOptions};
    use std::io::{BufWriter, Write};
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    pub const ENV: &str = "DONGSI_MEDIA_TRACE";
    /// 열: 종류(v/a), 코덱, PT, RTP 타임스탬프(64비트 값), 캡처 시각(UNIX epoch µs),
    /// 큐에서 꺼낸 시각 − 캡처(µs), 쓰기 결과(ok / no_writer / no_pt / err:…).
    pub const HEADER: &str = "kind,codec,pt,rtp,capture_epoch_us,dequeue_lag_us,result";
    const FLUSH_EVERY: Duration = Duration::from_millis(250);

    enum Msg {
        Line(String),
        Flush(SyncSender<()>),
    }

    pub struct Sink {
        tx: Sender<Msg>,
    }

    impl Sink {
        pub fn open(path: &Path, header: &str) -> std::io::Result<Sink> {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            let file = OpenOptions::new().create(true).append(true).open(path)?;
            let empty = file.metadata()?.len() == 0;
            let mut w = BufWriter::new(file);
            if empty {
                writeln!(w, "{header}")?;
                w.flush()?;
            }
            let (tx, rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("dongsi-pub-trace".into())
                .spawn(move || writer_loop(w, rx))?;
            Ok(Sink { tx })
        }

        pub fn line(&self, s: String) {
            let _ = self.tx.send(Msg::Line(s));
        }

        /// 구분 줄: `# <label> epoch_us=<µs>`
        pub fn mark(&self, label: &str) {
            self.line(format!("# {label} epoch_us={}", now_epoch_us()));
        }

        #[allow(clippy::too_many_arguments)]
        pub fn write_row(
            &self,
            kind: char,
            codec: &str,
            pt: Option<u8>,
            rtp: u64,
            capture: Instant,
            dequeued: Instant,
            result: &str,
        ) {
            let pt = pt.map_or(String::new(), |p| p.to_string());
            self.line(format!(
                "{kind},{codec},{pt},{rtp},{},{},{result}",
                epoch_us(capture),
                signed_us(dequeued, capture)
            ));
        }

        /// 지금까지 넣은 줄을 파일에 내보낼 때까지 기다린다(최대 2초).
        pub fn flush(&self) {
            let (ack_tx, ack_rx) = mpsc::sync_channel(1);
            if self.tx.send(Msg::Flush(ack_tx)).is_ok() {
                let _ = ack_rx.recv_timeout(Duration::from_secs(2));
            }
        }
    }

    fn writer_loop(mut w: BufWriter<File>, rx: Receiver<Msg>) {
        let mut last = Instant::now();
        loop {
            match rx.recv_timeout(FLUSH_EVERY) {
                Ok(Msg::Line(s)) => {
                    let _ = w.write_all(s.as_bytes());
                    let _ = w.write_all(b"\n");
                }
                Ok(Msg::Flush(ack)) => {
                    let _ = w.flush();
                    last = Instant::now();
                    let _ = ack.send(());
                    continue;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = w.flush();
                    return;
                }
            }
            if last.elapsed() >= FLUSH_EVERY {
                let _ = w.flush();
                last = Instant::now();
            }
        }
    }

    fn now_epoch_us() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_micros() as i64)
    }

    /// `a - b`(µs, 부호 있음).
    pub fn signed_us(a: Instant, b: Instant) -> i64 {
        match a.checked_duration_since(b) {
            Some(d) => d.as_micros() as i64,
            None => -(b.duration_since(a).as_micros() as i64),
        }
    }

    /// `Instant`를 UNIX epoch µs로 바꾼다. 처음 부를 때 잡은 (`Instant`, 벽시계) 한 쌍이 기준이다.
    pub fn epoch_us(t: Instant) -> i64 {
        static ANCHOR: OnceLock<(Instant, i64)> = OnceLock::new();
        let (ai, ae) = *ANCHOR.get_or_init(|| (Instant::now(), now_epoch_us()));
        ae + signed_us(t, ai)
    }

    /// 쓰기 결과를 CSV 한 칸에 들어가는 글로.
    pub fn result_text<E: std::fmt::Display>(r: &Result<(), E>) -> String {
        match r {
            Ok(()) => "ok".into(),
            Err(e) => format!("err:{}", e.to_string().replace([',', '\n', '\r'], " ")),
        }
    }

    fn dir() -> Option<&'static Path> {
        static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
        DIR.get_or_init(|| {
            std::env::var_os(ENV)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        })
        .as_deref()
    }

    /// `publish.csv`로 가는 통로. 기록이 꺼져 있으면 `None`.
    pub fn publish() -> Option<&'static Sink> {
        static S: OnceLock<Option<Sink>> = OnceLock::new();
        S.get_or_init(|| {
            let path = dir()?.join("publish.csv");
            match Sink::open(&path, HEADER) {
                Ok(s) => Some(s),
                Err(e) => {
                    tracing::warn!(path = %path.display(), "송출 기록 파일을 열지 못했습니다: {e}");
                    None
                }
            }
        })
        .as_ref()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn rows_header_once_and_append() {
            let dir = std::env::temp_dir().join(format!(
                "dongsi-pubtrace-test-{}-{}",
                std::process::id(),
                now_epoch_us()
            ));
            let path = dir.join("publish.csv");
            let t0 = Instant::now();
            for round in 0..2 {
                let s = Sink::open(&path, HEADER).unwrap();
                s.mark("publisher");
                s.write_row(
                    'v',
                    "h264",
                    Some(102),
                    9000,
                    t0,
                    t0 + Duration::from_millis(3),
                    "ok",
                );
                s.write_row('a', "opus", None, 960 * round, t0, t0, "no_pt");
                s.flush();
            }
            let text = fs::read_to_string(&path).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            assert_eq!(lines.len(), 1 + 2 * 3, "{text}");
            assert_eq!(lines[0], HEADER);
            assert!(lines[1].starts_with("# publisher epoch_us="));
            let v: Vec<&str> = lines[2].split(',').collect();
            assert_eq!(v.len(), HEADER.split(',').count());
            assert_eq!(&v[..4], &["v", "h264", "102", "9000"]);
            assert_eq!(v[5], "3000");
            assert_eq!(v[6], "ok");
            let a: Vec<&str> = lines[3].split(',').collect();
            assert_eq!(&a[..4], &["a", "opus", "", "0"]);
            assert_eq!(a[5], "0");
            assert!(lines[4].starts_with("# publisher epoch_us="), "{text}");
            // epoch 환산은 같은 Instant에 같은 값이다.
            assert_eq!(epoch_us(t0), epoch_us(t0));
            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn signed_us_negative_and_result_text() {
            let a = Instant::now();
            let b = a + Duration::from_millis(5);
            assert_eq!(signed_us(b, a), 5000);
            assert_eq!(signed_us(a, b), -5000);
            assert_eq!(result_text::<String>(&Ok(())), "ok");
            assert_eq!(result_text(&Err("a,b\nc".to_string())), "err:a b c");
        }
    }
}
