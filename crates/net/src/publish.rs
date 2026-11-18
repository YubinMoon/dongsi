//! WebRTC 송출 클라이언트(str0m).
//!
//! 방송자 앱이 인코딩한 프레임을 서버(SFU)로 보낸다. 서버는 ICE-lite이고 후보 주소 하나를 알려 준다.
//!
//! 스레드 구성:
//! - 소켓 읽기 스레드: UDP 패킷을 받아 명령 채널로 넘긴다.
//! - RTC 스레드: 명령 채널(프레임, 패킷)을 받아 str0m을 구동한다. 채널이 깨우므로 프레임이 들어오면
//!   바로 보낸다(지연을 줄이려고 소켓 타임아웃에 기대지 않는다).
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
use str0m::change::{SdpAnswer, SdpPendingOffer};
use str0m::format::Codec;
use str0m::media::{Direction, MediaKind, MediaTime, Mid};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tracing::{debug, info, warn};

use crate::NetError;

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
    Close,
}

struct Shared {
    stats: Mutex<PublishStats>,
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
    /// 서버에 보낼 SDP offer.
    pub offer: String,
}

impl PendingPublisher {
    /// `server_ip`는 시그널링에 쓴 서버 IP. 이 IP로 가는 경로의 로컬 주소를 후보로 쓴다.
    ///
    /// 소켓은 모든 주소(0.0.0.0)에 연다. 서버가 알려 주는 후보가 접속 주소와 다를 수 있기 때문이다
    /// (같은 PC에서 루프백으로 접속하면 서버는 LAN 주소를 후보로 준다).
    pub fn new(server_ip: IpAddr, has_audio: bool) -> Result<Self, NetError> {
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
            .enable_h265(true)
            .enable_vp8(true)
            .enable_vp9(true)
            .enable_av1(true)
            .set_rtcp_report_interval_audio(SR_INTERVAL)
            .set_rtcp_report_interval_video(SR_INTERVAL)
            .set_send_buffer_video(4000)
            .set_stats_interval(Some(Duration::from_secs(1)))
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
            ..
        } = self;
        let answer = SdpAnswer::from_sdp_string(answer)
            .map_err(|e| NetError::Rtc(format!("SDP answer 형식 오류: {e}")))?;
        rtc.sdp_api()
            .accept_answer(pending, answer)
            .map_err(|e| NetError::Rtc(format!("SDP answer를 받지 못했습니다: {e}")))?;

        let shared = Arc::new(Shared {
            stats: Mutex::new(PublishStats::default()),
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
            .spawn(move || read_loop(reader_socket, reader_tx, reader_shared))?;

        let local = local_candidate;
        let loop_shared = shared.clone();
        let runner = thread::Builder::new()
            .name("dongsi-pub-rtc".into())
            .spawn(move || {
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
                };
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
}

impl RtcLoop {
    fn run(&mut self, rx: Receiver<Cmd>) {
        while let Some(timeout) = self.drive() {
            let wait = timeout
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(100));
            let cmd = match rx.recv_timeout(wait) {
                Ok(c) => c,
                Err(RecvTimeoutError::Timeout) => {
                    let _ = self.rtc.handle_input(Input::Timeout(Instant::now()));
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            };
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
        self.rtc.disconnect();
        let _ = self.drive();
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
        }
        true
    }

    fn write_video(&mut self, codec: VideoCodec, data: Vec<u8>, capture: Instant) {
        let Some(writer) = self.rtc.writer(self.video_mid) else {
            return;
        };
        let Some(pt) = pick_pt(writer.payload_params().map(|p| (p.pt(), p)), codec) else {
            if self.last_video_codec != Some(codec) {
                warn!("서버와 협상되지 않은 코덱입니다: {codec}");
            }
            self.last_video_codec = Some(codec);
            return;
        };
        self.last_video_codec = Some(codec);
        let since = capture.saturating_duration_since(self.epoch);
        let rtp = MediaTime::from_90khz((since.as_micros() as u64) * 9 / 100);
        if let Err(e) = writer.write(pt, capture, rtp, data) {
            debug!("비디오 쓰기 실패: {e}");
        }
    }

    fn write_audio(&mut self, data: Vec<u8>, capture: Instant, sample_index: u64) {
        let Some(mid) = self.audio_mid else {
            return;
        };
        let Some(writer) = self.rtc.writer(mid) else {
            return;
        };
        let Some(pt) = writer
            .payload_params()
            .find(|p| p.spec().codec == Codec::Opus)
            .map(|p| p.pt())
        else {
            return;
        };
        let rtp = MediaTime::new(sample_index, str0m::media::Frequency::FORTY_EIGHT_KHZ);
        if let Err(e) = writer.write(pt, capture, rtp, data) {
            debug!("오디오 쓰기 실패: {e}");
        }
    }

    /// 출력을 모두 처리하고 다음 타임아웃을 돌려준다. 연결이 끝났으면 None.
    fn drive(&mut self) -> Option<Instant> {
        loop {
            if !self.rtc.is_alive() {
                self.close_with("연결이 끊어졌습니다".into());
                return None;
            }
            match self.rtc.poll_output() {
                Ok(Output::Timeout(t)) => return Some(t),
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

    fn close_with(&mut self, reason: String) {
        let mut stats = self.shared.stats.lock().unwrap();
        stats.connected = false;
        stats.closed.get_or_insert(reason);
    }
}

/// 코덱에 맞는 PT를 고른다. H.264는 High 프로파일(64xxxx), packetization-mode=1을 우선한다.
fn pick_pt<'a>(
    params: impl Iterator<Item = (str0m::media::Pt, &'a str0m::format::PayloadParams)>,
    codec: VideoCodec,
) -> Option<str0m::media::Pt> {
    let want = match codec {
        VideoCodec::H264 => Codec::H264,
        VideoCodec::H265 => Codec::H265,
        VideoCodec::Vp8 => Codec::Vp8,
        VideoCodec::Vp9 => Codec::Vp9,
        VideoCodec::Av1 => Codec::Av1,
    };
    params
        .filter(|(_, p)| p.spec().codec == want)
        .max_by_key(|(_, p)| {
            let f = &p.spec().format;
            let mode1 = f.packetization_mode == Some(1);
            let high = f.profile_level_id.is_some_and(|id| id >> 16 == 0x64);
            let vp9_profile0 = f.profile_id.is_none_or(|id| id == 0);
            (mode1, high, vp9_profile0)
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
