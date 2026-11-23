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
//! 재전송(D-087): 방송자 비디오 송신 스트림의 RTX 비율 상한을 0.5로 올린다([`VIDEO_RTX_RATIO_CAP`]).
//!
//! 진단 로그(D-087): 목표 비트레이트를 바꿀 때 까닭을 info로 남기고, 10초마다 지난 10초 동안 늘어난 값(목표 변경,
//! 키프레임 원인, NACK, RTX SSRC 패킷, 대기열 가득 참)이 있으면 한 줄로 요약한다([`Diag`]). 모두 0이면 남기지 않는다.
//!
//! 오디오 RED(D-087): 오디오는 NACK가 없어 한 번 잃으면 PLC뿐이므로, Opus에 RFC 2198 RED를 켜고 직전 1·2개 프레임의
//! 사본을 같이 보낸다([`AUDIO_RED_DISTANCES`]). 서버(answer)가 RED를 고르면 쓰이고 아니면 일반 Opus로 맞춘다.
//! 대가로 오디오 payload가 3배가 되어, [`PublishStats::audio_kbps`]도 RED 사본을 포함한 송신 바이트로 센다
//! (str0m `MediaEgressStats.bytes`는 RED로 감싼 RTP payload 길이의 합, `streams/send.rs:918-919`).
//!
//! 오디오·비디오 동기화: 두 트랙 모두 캡처 시각(QPC 기반 `Instant`)을 `wallclock`으로 넘긴다.
//! str0m이 이 값으로 RTCP SR(NTP↔RTP)을 만들고, 서버가 SR로 캡처 시각을 복원해 시청자에게
//! 같은 기준으로 다시 보낸다.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use dongsi_core::VideoCodec;
use dongsi_core::media::AUDIO_RED_DISTANCES;
use dongsi_core::priority::{ThreadRole, boost_current_thread};
use str0m::bwe::{Bitrate, BweKind};
use str0m::change::{SdpAnswer, SdpPendingOffer};
use str0m::format::Codec;
use str0m::media::{Direction, MediaKind, MediaTime, Mid};
use str0m::net::{Protocol, Receive};
use str0m::rtp::Ssrc;
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tracing::{debug, info, warn};

use crate::NetError;
use crate::rate::{RateAdapter, RateConfig, RateReason};

const SOCKET_BUFFER: usize = 8 * 1024 * 1024;
const QUEUE_LEN: usize = 256;
/// 비디오 송신 스트림이 보낸 패킷을 재전송용으로 들고 있는 개수. 빌더의 `set_send_buffer_video`와 같은 값을 쓴다
/// (스트림마다 `set_rtx_cache`를 다시 걸면 캐시가 새로 만들어지므로 크기를 맞춘다).
const SEND_BUFFER_VIDEO: usize = 4000;
/// 재전송용 패킷을 들고 있는 시간(str0m 기본값과 같다, `str0m/streams/send.rs:38`).
const RTX_CACHE_AGE: Duration = Duration::from_secs(3);
/// 방송자 비디오 송신 스트림의 RTX 비율 상한(D-087, 옛 값은 str0m 기본 0.15).
///
/// str0m은 최근 1초의 재전송 바이트 비율이 상한을 넘으면 대기 중인 재전송을 모두 지운다
/// (`str0m/streams/send.rs:863-872`, 기본값 `:40`, SDP API가 스트림을 만들 때 기본값을 건다 `str0m/change/sdp.rs:1008,1047`).
/// Wi-Fi에서 손실이 한꺼번에 몰리면 비율이 15%를 쉽게 넘어, 서버가 NACK으로 다시 달라는 패킷도 보내지 않게 되고
/// 시청자는 다음 키프레임까지 기다리며 멈춘다. 0.5로 올리면 비디오 바이트의 절반까지는 재전송에 쓴다.
/// `None`(상한 없음)으로 하지 않은 까닭: 막힌 링크에서 재전송이 끝없이 나가 혼잡을 더 키운다.
/// 오디오는 str0m에서 NACK 대상이 아니라서(`str0m/format/codec_config.rs:132-136`) 건드리지 않는다.
const VIDEO_RTX_RATIO_CAP: Option<f32> = Some(0.5);
/// 진단 요약 로그 간격(D-087).
const DIAG_INTERVAL: Duration = Duration::from_secs(10);
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
    /// 상한(화질 설정) 변경으로 목표가 바뀐 횟수. 앱 스레드가 올리고 진단 로그가 읽는다(D-087).
    cap_changes: AtomicU64,
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
            // 오디오 RED(D-087): 서버(answer)가 RED를 받으면 쓰이고, 아니면 일반 Opus로 맞춘다.
            .enable_opus(true, true)
            .set_red_distances(&AUDIO_RED_DISTANCES)
            .enable_h264(true)
            .enable_av1(true)
            .set_rtcp_report_interval_audio(SR_INTERVAL)
            .set_rtcp_report_interval_video(SR_INTERVAL)
            .set_send_buffer_video(SEND_BUFFER_VIDEO)
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
        // SDP API가 만든 비디오 송신 스트림에 RTX 비율 상한을 건다(D-087). 스트림은 `accept_answer`가 만든다
        // (`str0m/change/sdp.rs:1039-1047`). 못 찾아도 송출은 계속한다.
        let video_rtx = match rtc.direct_api().stream_tx_by_mid(video_mid, None) {
            Some(stream) => {
                stream.set_rtx_cache(SEND_BUFFER_VIDEO, RTX_CACHE_AGE, VIDEO_RTX_RATIO_CAP);
                stream.rtx()
            }
            None => {
                warn!("비디오 송신 스트림을 찾지 못해 재전송 비율 상한을 올리지 못했습니다");
                None
            }
        };
        rtc.bwe()
            .set_bitrate(Bitrate::ZERO, kbps(rate.desired_kbps()));

        let shared = Arc::new(Shared {
            stats: Mutex::new(PublishStats {
                target_kbps: rate.target_kbps(),
                max_kbps: rate.max_video_kbps(),
                ..PublishStats::default()
            }),
            rate: Mutex::new(rate),
            cap_changes: AtomicU64::new(0),
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
                    video_rtx,
                    audio_mid,
                    shared: loop_shared,
                    epoch: Instant::now() - Duration::from_secs(10),
                    last_video_codec: None,
                    bytes: Default::default(),
                    diag: Diag::new(Instant::now()),
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
        let (before, target, reset, desired) = {
            let mut r = self.shared.rate.lock().unwrap();
            let before = r.target_kbps();
            let reset = r.set_max_video(max_kbps, Instant::now());
            (
                before,
                r.target_kbps(),
                reset.then(|| r.initial_estimate_kbps()),
                r.desired_kbps(),
            )
        };
        if target != before {
            self.shared.cap_changes.fetch_add(1, Ordering::Relaxed);
            info!(
                from = before,
                to = target,
                max_kbps,
                "상한(화질 설정)을 바꿔 비디오 비트레이트 목표가 바뀝니다"
            );
        }
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
    /// 비디오 송신 스트림의 RTX SSRC. 협상되지 않았거나 스트림을 못 찾았으면 `None`.
    video_rtx: Option<Ssrc>,
    audio_mid: Option<Mid>,
    shared: Arc<Shared>,
    epoch: Instant,
    last_video_codec: Option<VideoCodec>,
    bytes: ByteCounters,
    diag: Diag,
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
                    if let Some(rtx) = self.video_rtx
                        && rtp_ssrc(&t.contents) == Some(*rtx)
                    {
                        self.diag.rtx_ssrc_packets += 1;
                    }
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
                self.diag.kf_connected += 1;
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
        if let Some(change) = r.tick(now) {
            self.diag.rate.count(change.reason);
            // 목표가 바뀌면 인코더가 IDR을 낼 가능성이 높다(실제 IDR 여부는 인코더 쪽이다).
            info!(
                from = before,
                to = change.kbps,
                reason = change.reason.label(),
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
        if now.saturating_duration_since(self.diag.last_report) >= DIAG_INTERVAL {
            let totals = self.diag.totals(&stats, &self.shared);
            if let Some(delta) = self.diag.roll(now, totals) {
                info!(
                    twcc_kbps = ?r.twcc_kbps(),
                    remb_kbps = ?r.remb_kbps(now),
                    target_kbps = r.target_kbps(),
                    max_kbps = r.max_video_kbps(),
                    rate_down = delta.rate.down,
                    rate_urgent_down = delta.rate.urgent_down,
                    rate_up = delta.rate.up,
                    rate_fast_up = delta.rate.fast_up,
                    rate_cap = delta.rate.cap,
                    kf_server = delta.kf_server,
                    kf_connected = delta.kf_connected,
                    kf_queue_full = delta.queue_full,
                    nacks = delta.nacks,
                    rtx_ssrc_packets = delta.rtx_ssrc_packets,
                    rtt_ms = ?stats.rtt_ms,
                    loss = ?stats.loss,
                    "송출 진단(지난 10초): rate_*는 목표 변경(까닭별), kf_*는 키프레임 요청 원인, kf_queue_full은 대기열이 가득 차 버린 프레임 수, rtx_ssrc_packets는 RTX SSRC로 나간 패킷 수(탐색 padding 포함)"
                );
            }
        }
    }

    fn close_with(&mut self, reason: String) {
        let mut stats = self.shared.stats.lock().unwrap();
        stats.connected = false;
        stats.closed.get_or_insert(reason);
    }
}

/// 목표 비트레이트 변경 횟수(까닭별).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct RateCounts {
    down: u64,
    urgent_down: u64,
    up: u64,
    fast_up: u64,
    /// 상한(화질 설정) 변경으로 바뀐 것.
    cap: u64,
}

impl RateCounts {
    fn count(&mut self, reason: RateReason) {
        match reason {
            RateReason::Down => self.down += 1,
            RateReason::UrgentDown => self.urgent_down += 1,
            RateReason::Up => self.up += 1,
            RateReason::FastUp => self.fast_up += 1,
        }
    }
}

/// 진단 카운터 값(누적 또는 한 구간에 늘어난 값, D-087).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct DiagCounts {
    rate: RateCounts,
    /// 서버가 보낸 PLI/FIR(`Event::KeyframeRequest`).
    kf_server: u64,
    /// 연결(`Event::Connected`)로 요청한 키프레임.
    kf_connected: u64,
    /// 송출 대기열이 가득 차 프레임을 버리고 키프레임을 요청한 횟수(`Publisher::send_video`). 버린 프레임 수와 같다.
    queue_full: u64,
    /// 받은 NACK 수(`MediaEgressStats::nacks`).
    nacks: u64,
    /// 비디오 RTX SSRC로 나간 패킷 수. BWE 탐색 padding도 RTX SSRC로 나가므로 순수 재전송보다 많을 수 있다.
    rtx_ssrc_packets: u64,
}

impl DiagCounts {
    /// `prev` 이후 늘어난 값.
    fn since(&self, prev: &DiagCounts) -> DiagCounts {
        let d = |a: u64, b: u64| a.saturating_sub(b);
        DiagCounts {
            rate: RateCounts {
                down: d(self.rate.down, prev.rate.down),
                urgent_down: d(self.rate.urgent_down, prev.rate.urgent_down),
                up: d(self.rate.up, prev.rate.up),
                fast_up: d(self.rate.fast_up, prev.rate.fast_up),
                cap: d(self.rate.cap, prev.rate.cap),
            },
            kf_server: d(self.kf_server, prev.kf_server),
            kf_connected: d(self.kf_connected, prev.kf_connected),
            queue_full: d(self.queue_full, prev.queue_full),
            nacks: d(self.nacks, prev.nacks),
            rtx_ssrc_packets: d(self.rtx_ssrc_packets, prev.rtx_ssrc_packets),
        }
    }

    fn is_zero(&self) -> bool {
        *self == DiagCounts::default()
    }
}

/// RTC 스레드의 진단 상태: 이 스레드만 올리는 카운터와 10초 구간 기준값.
struct Diag {
    rate: RateCounts,
    kf_connected: u64,
    rtx_ssrc_packets: u64,
    last_report: Instant,
    prev: DiagCounts,
}

impl Diag {
    fn new(now: Instant) -> Self {
        Diag {
            rate: RateCounts::default(),
            kf_connected: 0,
            rtx_ssrc_packets: 0,
            last_report: now,
            prev: DiagCounts::default(),
        }
    }

    /// 지금까지의 누적값. 다른 스레드가 올리는 값(상한 변경, 대기열 가득 참)과 str0m 통계(NACK, PLI/FIR)를 모은다.
    fn totals(&self, stats: &PublishStats, shared: &Shared) -> DiagCounts {
        DiagCounts {
            rate: RateCounts {
                cap: shared.cap_changes.load(Ordering::Relaxed),
                ..self.rate
            },
            kf_server: stats.keyframe_requests,
            kf_connected: self.kf_connected,
            queue_full: stats.dropped_frames,
            nacks: stats.nacks,
            rtx_ssrc_packets: self.rtx_ssrc_packets,
        }
    }

    /// 구간을 끝내고 늘어난 값을 돌려준다. 모두 0이면 `None`(조용한 송출에서 로그가 쌓이지 않게).
    fn roll(&mut self, now: Instant, totals: DiagCounts) -> Option<DiagCounts> {
        self.last_report = now;
        let delta = totals.since(&self.prev);
        self.prev = totals;
        (!delta.is_zero()).then_some(delta)
    }
}

/// (S)RTP 패킷의 SSRC. RTCP·STUN·DTLS면 `None`. SRTP도 RTP 머리는 암호화하지 않는다.
///
/// 서버의 `crates/server/src/sfu.rs` `rtp_ssrc`와 같다(crate가 달라 따로 둔다).
fn rtp_ssrc(packet: &[u8]) -> Option<u32> {
    if packet.len() < 12 || packet[0] >> 6 != 2 {
        return None;
    }
    // RTCP(PT 200~206)는 둘째 바이트 하위 7비트가 72~78이다(RFC 5761). 우리 RTP PT는 동적 범위(96~127)다.
    if (64..96).contains(&(packet[1] & 0x7f)) {
        return None;
    }
    Some(u32::from_be_bytes([
        packet[8], packet[9], packet[10], packet[11],
    ]))
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

/// 소켓 버퍼를 키운다(고비트레이트 1440p 키프레임처럼 큰 프레임을 한 번에 보낼 때 버려지지 않게).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtp_ssrc_reads_rtp_and_skips_rtcp() {
        // V=2, PT 96, SSRC 0x01020304.
        let mut rtp = vec![0x80, 96, 0, 1, 0, 0, 0, 0, 1, 2, 3, 4];
        assert_eq!(rtp_ssrc(&rtp), Some(0x0102_0304));
        // marker 비트가 켜져도 같다.
        rtp[1] = 0x80 | 96;
        assert_eq!(rtp_ssrc(&rtp), Some(0x0102_0304));
        // RTCP SR(PT 200), STUN(앞 2비트 0), 너무 짧은 패킷.
        let rtcp = vec![0x80, 200, 0, 6, 1, 2, 3, 4, 0, 0, 0, 0];
        assert_eq!(rtp_ssrc(&rtcp), None);
        assert_eq!(rtp_ssrc(&[0x00; 20]), None);
        assert_eq!(rtp_ssrc(&rtp[..11]), None);
    }

    #[test]
    fn diag_reports_only_what_grew_and_stays_quiet_otherwise() {
        let t0 = Instant::now();
        let mut diag = Diag::new(t0);
        let mut totals = DiagCounts::default();
        // 아무것도 안 늘면 남기지 않는다.
        assert_eq!(diag.roll(t0 + DIAG_INTERVAL, totals), None);
        assert_eq!(diag.last_report, t0 + DIAG_INTERVAL);

        totals.nacks = 7;
        totals.rate.urgent_down = 1;
        totals.rtx_ssrc_packets = 40;
        let d = diag.roll(t0 + DIAG_INTERVAL * 2, totals).unwrap();
        assert_eq!(d.nacks, 7);
        assert_eq!(d.rate.urgent_down, 1);
        assert_eq!(d.rtx_ssrc_packets, 40);
        assert_eq!(d.kf_server, 0);

        // 다음 구간은 그 구간에 늘어난 만큼만 센다(누적값에서 앞 구간을 뺀다).
        totals.nacks = 9;
        totals.kf_server = 2;
        let d = diag.roll(t0 + DIAG_INTERVAL * 3, totals).unwrap();
        assert_eq!(d.nacks, 2);
        assert_eq!(d.kf_server, 2);
        assert_eq!(d.rate, RateCounts::default());
        assert_eq!(d.rtx_ssrc_packets, 0);

        // 변화가 없으면 다시 조용하다.
        assert_eq!(diag.roll(t0 + DIAG_INTERVAL * 4, totals), None);
    }

    #[test]
    fn rate_counts_by_reason() {
        let mut c = RateCounts::default();
        c.count(RateReason::Down);
        c.count(RateReason::UrgentDown);
        c.count(RateReason::UrgentDown);
        c.count(RateReason::Up);
        c.count(RateReason::FastUp);
        assert_eq!(
            c,
            RateCounts {
                down: 1,
                urgent_down: 2,
                up: 1,
                fast_up: 1,
                cap: 0
            }
        );
    }
}
