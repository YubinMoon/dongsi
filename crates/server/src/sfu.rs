//! SFU: UDP 소켓 하나로 모든 WebRTC 세션(str0m, ICE-lite)을 처리하고, 방송자의 미디어를 시청자에게 전달한다.
//!
//! - 미디어는 다시 인코딩하지 않는다. 프레임 단위로 받아(`MediaData`) 시청자 세션에 그대로 쓴다.
//! - 같은 소켓으로 로컬 감지 질의(D-030)에도 응답한다. 첫 바이트로 WebRTC 패킷과 구분하고, 감지 질의는
//!   읽기 스레드가 [`crate::discovery`]로 바로 처리한다(SFU 스레드의 미디어 처리 뒤에 줄 서지 않는다).
//!
//! ## 오디오·비디오 동기화
//! 시청자에게 보내는 `wallclock`은 방송자의 캡처 시각에서 나와야 한다. 방송자 앱은 두 트랙에 같은
//! 시계(QPC)로 만든 캡처 시각을 넘기고, str0m이 이를 RTCP SR(NTP↔RTP)로 보낸다. 여기서는
//! `MediaData::last_sender_info`로 프레임의 캡처 시각(방송자 시계)을 구하고, 방송자마다 하나인
//! 기준점으로 이 서버의 `Instant`로 옮긴다. 두 트랙이 같은 기준점을 쓰므로 상대 시각이 유지된다.
//!
//! ## 미디어 전 SR
//! str0m은 미디어를 한 번도 쓰지 않은 송신 스트림도 RTCP SR을 보내는데, 그 내용이 "RTP 0 ↔ 지금 NTP"
//! (`sender_packet_count` 0)라서 시청 쪽이 비디오(RTP가 약 10초에서 시작)와 오디오(0에서 시작)를 약 10초
//! 어긋나게 계산한다. 그래서
//! - 방송자가 보낸 `sender_packet_count` 0인 SR은 캡처 시각 복원에 쓰지 않고(`wallclock`),
//! - 시청자 세션의 송신 스트림은 SDP 협상 때 만들어진 것을 지웠다가(`defer_stream_tx`) 그 트랙의 첫
//!   미디어를 쓸 때 같은 SSRC·RTX로 다시 선언한다(`forward`). 스트림이 없는 동안은 SR도 나가지 않는다.
//! - 새 스트림의 첫 패킷이 나가기 전에 SR이 만들어져도 다시 "RTP 0"이 된다. 그래서 새 스트림을 선언하기 전에 다른 큐를
//!   비우고(`flush_tx_queues`) 새 스트림은 pacer 밖(unpaced)에 둔 뒤, 타임아웃을 한 번 주고 곧바로 내보낸다. 그래도 첫
//!   패킷이 나가지 않았으면 스트림을 지우고 다음 미디어에서 다시 한다(`forward`). 시청자 세션에는 BWE가 없어(D-104) str0m이
//!   pacing 없이 패킷을 바로 다 내보내므로, 이 셋은 첫 패킷이 밀릴 때를 위한 안전장치다.
//!
//! ## 오디오 RED(D-087)
//! 오디오는 NACK가 없어 한 번 잃으면 PLC뿐이다. 모든 세션(방송자·시청자)이 `enable_opus(true, true)`와
//! [`AUDIO_RED_DISTANCES`](`[1, 2]`)로 RED(RFC 2198)를 켠다(`add_session`). frame mode의 SFU는 방송자 세션에서
//! RED를 풀어 Opus 프레임으로 꺼내고(`MediaData`), 시청자 세션의 Writer가 자기 송신 이력으로 다시 RED를 만든다.
//! 그래서 방송자→SFU와 SFU→시청자 두 구간이 서로 독립이고, 둘 다 켜야 한다. offer에 RED가 없는 상대는 str0m이
//! answer에서 일반 Opus(red=None)로 맞춘다. 방송자 세션에서 오디오 손실을 RED가 메우면 SFU의 reordering 대기
//! (아래 "오디오 재정렬 대기")도 생기지 않는다. 대가는 오디오 payload
//! 3배: SFU 송신 대역이 시청자 수 × 약 +265kbps(128kbps Opus 기준) 늘어난다. `forward`의 `match_params`는 RED 여부를
//! 보지 않는다(Opus PT만 맞춘다).
//!
//! ## 오디오 재정렬 대기(D-092)
//! 방송자 세션의 수신 버퍼는 seq가 빈 오디오 패킷이 늦게라도 오기를 기다리며 뒤 프레임을 붙잡는다. str0m 기본값은 완성된
//! 프레임 15개(`reordering_size_audio`, Opus 10ms면 약 150ms, 시간 한도는 1초)다. 오디오는 NACK가 없어 잃은 패킷은 오지
//! 않으므로 이 대기는 손실마다 약 150ms 멈췄다가 몰아 보내기만 만든다(시청자는 seq가 이어진 채 늦게 받아 지연과 시청 버퍼가
//! 오른다). RED가 못 메운 손실(연속 3개 이상)에서도 짧게 끝나도록 [`AUDIO_REORDER_FRAMES`](3, 약 30ms)로 줄인다. 대가는
//! 3프레임 넘게 늦게 도착한 순서 뒤바뀜 패킷을 버리는 것이다(10ms 하나를 잃은 것과 같다). 시청자 세션은 오디오를 받지 않아
//! 영향이 없다. 시험은 `tests/e2e/recovery.rs` (e).
//!
//! ## 키프레임 요청
//! 방송자 세션마다 "키프레임이 필요함"을 기억한다(`KeyframeNeed`). 시청자 연결·시청자 PLI·수신 끊김으로
//! 켜고, PLI를 보낸 뒤 키프레임이 안 오면 0.5초마다 다시 보낸다. 방송자에게서 키프레임이 오면 끈다.
//! 시청자 한 명의 요청(RTCP PLI/FIR, 앱의 멈춤 감지 `RequestKeyframe`)은 2초에 한 번만 방송자 요청으로
//! 잇는다(`ViewerKeyframeLimit`). 시청자 한 명이 보채도 방송 전체가 매번 IDR을 만들지 않게 한다(D-087).
//! 새 시청자 연결은 지금처럼 바로 요청한다.
//!
//! ## 끊긴 뒤 키프레임까지 비디오 전달 중단(D-087)
//! 방송자 비디오 프레임이 `contiguous=false`(앞 프레임을 잃음, str0m `media/event.rs:257`)면 그 프레임과 뒤따르는
//! 키프레임이 아닌 프레임을 시청자에게 보내지 않고, 키프레임(`is_keyframe`, 키프레임 자신이 `contiguous=false`여도
//! 전달)이 오면 다시 보낸다(`VideoGate`). 예전에는 PLI만 올리고 깨진 프레임과 뒤 P-프레임을 그대로 보냈는데, 시청
//! 쪽 H.264 가속 디코더는 frame_num 빈틈을 넘어가서 키프레임이 올 때까지 깨진 화면이 보였다. 이제는 마지막 정상
//! 화면에서 멈췄다가 키프레임에서 이어진다. 오디오는 항상 전달한다. 받은 프레임은 버려도 받은 것으로 본다
//! (`KeyframeNeed::video_arrived`).
//!
//! ## 비디오 트랙 하나(D-104)
//! 방송자는 비디오를 rid 없는 트랙 하나(방송 설정 그대로)로 보내고, SFU는 그 프레임을 모든 시청자에게 그대로 넘긴다.
//! 키프레임 요청(`KeyframeNeed`)과 끊김 처리(`VideoGate`)도 방송마다 하나다. offer에 `a=simulcast`가 있는 방송자는
//! 받지 않는다(`add_session`). 시청자 세션에는 BWE를 켜지 않고 방송자에게 REMB도 보내지 않는다: 회선이 방송 비트레이트보다
//! 느린 시청자는 끊긴다. 내장 서버도 같다(호스트 업로드가 비트레이트 × 시청자 수보다 모자라도 반응하지 않는다).
//! 방송자 → 서버 구간 BWE(D-081)는 방송자 앱이 서버의 TWCC 피드백으로 한다.
//!
//! ## 시청자 송신 스트림의 재전송(RTX)
//! str0m은 송신 스트림의 최근 1초 재전송 비율이 `DEFAULT_RTX_RATIO_CAP`(0.15)을 넘으면 대기 중인 재전송을 모두
//! 지운다(`streams/send.rs:40, 863-872`). `declare_stream_tx`는 이 기본값을 거므로(`change/direct.rs:342`) 손실이
//! 잦은 Wi-Fi 시청자는 NACK에 답을 못 받고 키프레임을 기다리게 된다. 그래서 비디오 송신 스트림을 선언할 때
//! `set_rtx_cache`로 상한을 `RTX_RATIO_CAP`으로 올린다(`forward`).
use std::collections::{HashMap, VecDeque};
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::{Receiver, Sender, TrySendError, select};
use dongsi_core::discovery::is_discovery_packet;
use dongsi_core::media::AUDIO_RED_DISTANCES;
use dongsi_core::priority::{ThreadRole, boost_current_thread};
use str0m::change::SdpOffer;
use str0m::media::{KeyframeRequestKind, MediaData, MediaKind, Mid};
use str0m::net::{DatagramRecv, Protocol, Receive};
use str0m::rtp::Ssrc;
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use dongsi_net::publish::SR_INTERVAL;

use crate::discovery::{DiscoverySnapshot, Responder};
use crate::metrics::{KeyframeSource, Metrics, Peer};

/// 연결된 방송자 세션이 이 시간 동안 어떤 패킷(STUN·DTLS·RTP·RTCP)도 보내지 않으면 끊긴 것으로 본다.
///
/// 이 서버는 ICE-lite라서 상대가 사라져도 상대의 STUN 요청이 15초(`is` 크레이트의 상수, 설정 불가)
/// 없을 때에야 str0m이 `Disconnected`를 낸다. 방송자는 미디어가 없을 때도 RTCP SR을 스트림마다
/// 0.5초(`SR_INTERVAL`)마다 보내므로(측정: 미디어 없음일 때 패킷 간격 최대 약 0.5~0.65초), 8초는 SR을
/// 연속 15번 넘게 못 받은 것이다. Wi-Fi 송출이 로밍·재연결로 몇 초 끊겨도 방송이 끝나지 않게 3초에서 늘렸다
/// (D-059 변경, 2026-11-23). 방송자 쪽 str0m(full ICE)은 서버를 잃고 약 22초 뒤에야 알아채므로
/// (`publisher_notices_server_loss`) 8초 안에 이어지면 방송이 그대로 간다. 시청자 세션과 연결되지 못한 세션에는
/// 적용하지 않는다(ICE 15초 그대로).
const PUBLISHER_SILENCE_LIMIT: Duration = Duration::from_secs(8);
/// 방송자 침묵을 확인하는 간격.
const LIVENESS_CHECK_INTERVAL: Duration = Duration::from_millis(200);
/// 방송자 세션 오디오 수신 버퍼가 빈 seq를 기다리는 완성 프레임 수(모듈 설명 "오디오 재정렬 대기", D-092). str0m 기본 15.
const AUDIO_REORDER_FRAMES: usize = 3;
/// 시청자 비디오 송신 스트림의 재전송 캐시 크기(패킷 수). 세션 빌더의 `set_send_buffer_video`와 같은 값이다
/// (`declare_stream_tx`가 빌더 값을 쓰지만 `set_rtx_cache`는 크기를 다시 받는다).
const SEND_BUFFER_VIDEO: usize = 4000;
/// 시청자 비디오 송신 스트림의 재전송 캐시 보관 시간(str0m 기본값 `DEFAULT_RTX_CACHE_DURATION`과 같다,
/// `streams/send.rs:38`).
const RTX_CACHE_AGE: Duration = Duration::from_secs(3);
/// 시청자 비디오 송신 스트림의 재전송 비율 상한(최근 1초, 재전송 / 전체). str0m 기본값은 0.15라서 손실이 15%만
/// 넘어도 대기 중인 재전송을 모두 지운다(`streams/send.rs:40, 863-872`). 0.5로 올린다(D-087). `None`(무제한)으로
/// 하지 않은 까닭: 정말 막힌 링크에서는 재전송이 혼잡을 키우므로 안전장치는 남겨 둔다(2026-11-22 조사 보고서 2.3).
/// 오디오는 건드리지 않는다: str0m이 오디오 코덱에는 NACK 피드백을 협상하지 않아(`fb_nack=false`,
/// `format/codec_config.rs:132-136`) 오디오 재전송 자체가 없다.
const RTX_RATIO_CAP: f32 = 0.5;
/// 세션마다 str0m 통계 이벤트(`MediaIngressStats`·`MediaEgressStats`)를 받는 간격(D-087 진단). 세션 하나당 이 간격마다
/// 미디어(mid)마다 이벤트가 하나씩 나온다(방송자 2개, 시청자 2개). 시청자 10명 × 방송 여러 개라도 초당 수십 개다.
const STATS_INTERVAL: Duration = Duration::from_secs(1);
/// 방송·시청자별 진단 요약을 로그에 남기는 간격.
const PERIOD_LOG_INTERVAL: Duration = Duration::from_secs(10);
/// 방송자 요약에서 이 구간의 붙잡은 시간 최댓값이 이 이상이면 다른 값이 0이어도 남긴다. 붙잡은 시간은 프레임이
/// 있으면 늘 0이 아니라서 "0이 아님"으로는 거를 수 없다. 값은 측정 없이 정한 추정이다(미검증): 큰 키프레임의 패킷이
/// 퍼져 도착하는 시간보다 위로 잡았다. 실제 로그를 보고 조정한다.
const HOLD_LOG_MIN: Duration = Duration::from_millis(50);
/// 같은 이유로, 한 번의 처리에서 나온 비디오 프레임 수 최댓값이 이 이상이면 남긴다(정상은 1~2).
const BURST_LOG_MIN: u32 = 4;

pub(crate) enum SfuCmd {
    AddSession {
        session: u64,
        broadcast: u64,
        role: Role,
        offer: String,
        candidate: SocketAddr,
        reply: oneshot::Sender<Result<String, String>>,
    },
    Remove {
        session: u64,
    },
    /// 시청자 앱의 멈춤 감지가 키프레임을 요청했다. `session`은 요청한 시청자 세션(2초에 한 번만 방송자 요청이 된다).
    RequestKeyframe {
        broadcast: u64,
        session: u64,
    },
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Publisher,
    Viewer,
}

impl From<Role> for Peer {
    fn from(role: Role) -> Self {
        match role {
            Role::Publisher => Peer::Publisher,
            Role::Viewer => Peer::Viewer,
        }
    }
}

/// 주고받은 양을 지표(D-083)에 세는 UDP 소켓. 읽기 스레드와 SFU 스레드가 하나씩 가진다.
struct MeteredSocket {
    inner: UdpSocket,
    metrics: Arc<Metrics>,
}

impl MeteredSocket {
    fn send_to(&self, peer: Peer, buf: &[u8], dest: SocketAddr) -> std::io::Result<usize> {
        let result = self.inner.send_to(buf, dest);
        match &result {
            Ok(n) => self.metrics.udp_sent(peer, *n),
            Err(_) => self.metrics.udp_send_failed(),
        }
        result
    }
}

#[derive(Debug)]
pub(crate) enum SfuEvent {
    SessionClosed { session: u64 },
}

pub(crate) struct SfuHandle {
    pub cmd: Sender<SfuCmd>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl SfuHandle {
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.cmd.send(SfuCmd::Shutdown);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl Drop for SfuHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(crate) fn spawn(
    socket: UdpSocket,
    discovery: Option<DiscoverySnapshot>,
    events: mpsc::UnboundedSender<SfuEvent>,
    metrics: Arc<Metrics>,
) -> std::io::Result<SfuHandle> {
    dongsi_net::tune_udp_socket(&socket);
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    let reader_socket = MeteredSocket {
        inner: socket.try_clone()?,
        metrics: metrics.clone(),
    };
    let socket = MeteredSocket {
        inner: socket,
        metrics,
    };
    let stop = Arc::new(AtomicBool::new(false));
    let (pkt_tx, pkt_rx) = crossbeam_channel::bounded::<Packet>(16_384);
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<SfuCmd>();

    let reader_stop = stop.clone();
    let responder = discovery.map(Responder::new);
    let reader = thread::Builder::new()
        .name("dongsi-sfu-recv".into())
        .spawn(move || {
            let _boost = boost_current_thread(ThreadRole::Network);
            read_loop(reader_socket, pkt_tx, reader_stop, responder)
        })?;

    let runner = thread::Builder::new()
        .name("dongsi-sfu".into())
        .spawn(move || {
            let _boost = boost_current_thread(ThreadRole::Network);
            let mut sfu = Sfu {
                socket,
                sessions: Vec::new(),
                events,
                anchors: HashMap::new(),
                last_liveness_check: Instant::now(),
                last_period_log: Instant::now(),
            };
            sfu.run(pkt_rx, cmd_rx);
        })?;

    Ok(SfuHandle {
        cmd: cmd_tx,
        stop,
        threads: vec![runner, reader],
    })
}

struct Packet {
    data: Vec<u8>,
    source: SocketAddr,
    at: Instant,
}

/// 소켓에서 패킷을 읽는다. 감지 질의(D-030)는 `responder`가 여기서 바로 응답하고(SFU 스레드와 채널을
/// 거치지 않는다), 나머지는 채널로 SFU 스레드에 보낸다.
fn read_loop(
    socket: MeteredSocket,
    tx: Sender<Packet>,
    stop: Arc<AtomicBool>,
    mut responder: Option<Responder>,
) {
    let mut buf = vec![0u8; 2048];
    while !stop.load(Ordering::Relaxed) {
        match socket.inner.recv_from(&mut buf) {
            Ok((n, source)) => {
                if is_discovery_packet(&buf[..n]) {
                    socket.metrics.udp_received(Peer::Discovery, n);
                    if let Some(responder) = &mut responder {
                        responder.handle(&buf[..n], source, Instant::now(), |bytes| {
                            let _ = socket.send_to(Peer::Discovery, bytes, source);
                        });
                    }
                    continue;
                }
                let packet = Packet {
                    data: buf[..n].to_vec(),
                    source,
                    at: Instant::now(),
                };
                // 처리 스레드가 밀리면 버린다(커널 버퍼에서 버려지는 것과 같다).
                match tx.try_send(packet) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => socket.metrics.udp_dropped(),
                    Err(TrySendError::Disconnected(_)) => break,
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::ConnectionReset
                ) => {}
            Err(e) => {
                debug!("SFU 소켓 읽기 오류: {e}");
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

struct Session {
    id: u64,
    broadcast: u64,
    role: Role,
    rtc: Rtc,
    local: SocketAddr,
    connected: bool,
    /// 방송자: 들어오는 트랙(kind → mid)
    in_mids: HashMap<MediaKind, Mid>,
    /// 시청자: 나가는 트랙(kind → mid)
    out_mids: HashMap<MediaKind, Mid>,
    /// 시청자: 첫 미디어를 쓸 때까지 보류해 둔 송신 스트림(mid → SSRC, RTX SSRC).
    pending_tx: HashMap<Mid, (Ssrc, Option<Ssrc>)>,
    /// 방송자: 키프레임이 필요한지
    keyframe: KeyframeNeed,
    /// 방송자: 끊긴 뒤 키프레임까지 비디오를 시청자에게 보내지 않는 상태.
    video_gate: VideoGate,
    /// 시청자: 키프레임 요청을 방송자 요청으로 잇는 간격 제한.
    keyframe_limit: ViewerKeyframeLimit,
    /// 이 세션으로 마지막 패킷이 도착한 때(읽기 스레드가 받은 시각). 만들어진 때로 시작한다.
    last_rx: Instant,
    /// 시청자: 비디오 송신 스트림의 RTX SSRC. 이 SSRC로 나간 패킷을 재전송으로 센다(`drive_watch`).
    video_rtx: Option<Ssrc>,
    /// str0m 통계의 누적값을 mid마다 마지막으로 본 값(NACK, PLI, FIR). 차이만 지표에 더한다(`counter_deltas`).
    stats_seen: HashMap<Mid, [u64; 3]>,
    /// 방송자: 이번 로그 구간의 진단 값.
    pub_period: PublisherPeriod,
    /// 시청자: 이번 로그 구간의 진단 값.
    view_period: ViewerPeriod,
}

/// 방송자에게 키프레임이 필요한지와 PLI를 보낸 기록.
///
/// - 켜는 때: 시청자 연결(`force`), 시청자의 PLI·수신 끊김(`force` 아님).
/// - 끄는 때: 방송자에게서 키프레임이 왔다. 켜진 뒤 도착한 키프레임은 연결된 모든 시청자에게 전달된다.
/// - PLI를 못 보냈으면(수신 스트림을 아직 모름) 짧은 간격으로 다시 시도하고, 보냈는데 키프레임이 안
///   오면 500ms 뒤에 다시 보낸다(PLI 유실 대비, D-087: 예전에는 1초). `force`가 아니면 PLI가 500ms보다
///   자주 나가지 않는다.
#[derive(Debug, Default)]
struct KeyframeNeed {
    wanted: bool,
    /// 방송자의 비디오가 한 번이라도 왔다.
    media_seen: bool,
    /// 방송자의 미디어가 오기 전에 켜진 요청이 있다(그 PLI는 방송자가 받았는지 알 수 없다).
    wanted_before_media: bool,
    /// 이번에 켠 뒤로 PLI를 보냈고 키프레임을 아직 못 받았다.
    requested: bool,
    last_sent: Option<Instant>,
    last_attempt: Option<Instant>,
}

/// `force`가 아닌 PLI의 최소 간격.
const KEYFRAME_MIN_GAP: Duration = Duration::from_millis(500);
/// 보낸 PLI에 키프레임이 안 올 때 다시 보내는 간격. 예전 1초에서 줄였다(D-087): 끊긴 뒤 비디오 전달을 멈추고
/// (`VideoGate`) 키프레임을 기다리므로 PLI 한 번이 유실되면 그만큼 멈춤이 길어진다. 최소 간격과 같으니 이 간격
/// 안에는 `may_send`도 `due`도 보내지 않는다.
const KEYFRAME_RETRY: Duration = Duration::from_millis(500);
/// PLI를 보내지 못했을 때(수신 스트림을 모름) 다시 시도하는 간격.
const KEYFRAME_UNSENT_RETRY: Duration = Duration::from_millis(100);

impl KeyframeNeed {
    fn want(&mut self) {
        self.wanted = true;
        if !self.media_seen {
            self.wanted_before_media = true;
        }
    }

    /// 방송자의 비디오 프레임이 도착했다. 미디어 전에 켜진 요청이 있었으면 true: 그때 보낸 PLI는 방송자가
    /// 아직 보내는 중이 아닐 때 닿아서 쓰이지 않을 수 있으므로(시험에서 PLI가 한 번도 닿지 않은 경우가
    /// 있었다) 미디어가 흐르기 시작한 지금 한 번 더 보내야 한다. 처음 한 번만 true다.
    fn video_arrived(&mut self) -> bool {
        let first = !self.media_seen;
        self.media_seen = true;
        first && std::mem::take(&mut self.wanted_before_media)
    }

    /// 방송자에게서 키프레임이 왔다.
    fn satisfied(&mut self) {
        self.wanted = false;
        self.requested = false;
    }

    /// 새 요청으로 PLI를 지금 보내도 되는지.
    fn may_send(&self, now: Instant, force: bool) -> bool {
        if force {
            return true;
        }
        let Some(t) = self.last_sent else {
            return true;
        };
        // 보낸 PLI의 답을 기다리는 중(`requested`)에도 재시도 간격(`KEYFRAME_RETRY`)과 같은 500ms다.
        const _: () = assert!(KEYFRAME_RETRY.as_millis() == KEYFRAME_MIN_GAP.as_millis());
        now.saturating_duration_since(t) >= KEYFRAME_MIN_GAP
    }

    /// 요청이 아직 필요한데 시간이 지나서 PLI를 (다시) 보낼 때가 됐는지.
    fn due(&self, now: Instant) -> bool {
        if !self.wanted {
            return false;
        }
        if self.requested {
            return self
                .last_sent
                .is_none_or(|t| now.saturating_duration_since(t) >= KEYFRAME_RETRY);
        }
        self.last_attempt
            .is_none_or(|t| now.saturating_duration_since(t) >= KEYFRAME_UNSENT_RETRY)
            && self.may_send(now, false)
    }

    fn sent(&mut self, now: Instant) {
        self.requested = true;
        self.last_sent = Some(now);
        self.last_attempt = Some(now);
    }

    fn failed(&mut self, now: Instant) {
        self.last_attempt = Some(now);
    }
}

/// 방송자 비디오가 끊긴 뒤 키프레임까지 시청자에게 보내지 않는 상태(모듈 설명, D-087).
///
/// - 키프레임(`is_keyframe`): 상태를 끄고 전달한다. 키프레임 자신이 `contiguous=false`여도 디코더가 거기서 새로
///   시작하므로 전달한다.
/// - 키프레임이 아닌데 `contiguous=false`: 상태를 켜고 그 프레임을 버린다.
/// - 상태가 켜져 있으면 키프레임이 아닌 비디오 프레임은 버린다.
/// - 처음 연결 직후(끊김이 없었음)에는 상태가 꺼져 있어 지금까지처럼 전달한다.
///
/// 오디오는 이 상태를 거치지 않는다(호출하지 않는다).
#[derive(Debug, Default)]
struct VideoGate {
    waiting: Option<GateWait>,
}

#[derive(Debug)]
struct GateWait {
    since: Instant,
    dropped: u32,
}

/// `VideoGate::on_frame`의 결과.
#[derive(Debug, PartialEq, Eq)]
struct GateStep {
    /// 시청자에게 보낼지.
    forward: bool,
    /// 이 프레임으로 상태가 바뀌었으면 그 내용(로그용).
    change: Option<GateChange>,
}

#[derive(Debug, PartialEq, Eq)]
enum GateChange {
    /// 끊겨서 켜졌다.
    Started,
    /// 키프레임이 와서 꺼졌다. 버린 프레임 수와 기다린 시간.
    Resumed { dropped: u32, waited: Duration },
}

impl VideoGate {
    fn on_frame(&mut self, now: Instant, keyframe: bool, contiguous: bool) -> GateStep {
        if keyframe {
            let change = self.waiting.take().map(|w| GateChange::Resumed {
                dropped: w.dropped,
                waited: now.saturating_duration_since(w.since),
            });
            return GateStep {
                forward: true,
                change,
            };
        }
        let mut change = None;
        match &mut self.waiting {
            Some(w) => w.dropped += 1,
            None if !contiguous => {
                self.waiting = Some(GateWait {
                    since: now,
                    dropped: 1,
                });
                change = Some(GateChange::Started);
            }
            None => {
                return GateStep {
                    forward: true,
                    change: None,
                };
            }
        }
        GateStep {
            forward: false,
            change,
        }
    }
}

/// 시청자 한 명의 키프레임 요청을 방송자 요청으로 잇는 간격 제한(D-087).
///
/// 방송자의 IDR은 시청자 모두에게 가므로 한 명의 요청이 매번 방송 전체의 IDR이 되면 안 된다(IDR은 평소 프레임의
/// 여러 배라 손실이 있는 시청자 회선에 부담이 커지고, 그 시청자는 더 끊긴다). 시청자 세션마다 마지막으로 방송자
/// 요청으로 이어진 때를 두고, 그로부터 `VIEWER_KEYFRAME_INTERVAL` 안의 요청은 무시한다. 새 시청자 연결
/// (`Fanout::ViewerConnected`)은 여기를 거치지 않고 바로 요청하며 이 창도 열지 않는다: 연결 직후에는 첫 미디어가
/// 키프레임을 기다릴 뿐이고, 그 IDR이 이 시청자에게 닿지 않았을 때의 PLI가 막히면 안 된다.
#[derive(Debug, Default)]
struct ViewerKeyframeLimit {
    last: Option<Instant>,
}

/// 시청자 한 명의 키프레임 요청이 방송자 요청으로 이어지는 최소 간격.
const VIEWER_KEYFRAME_INTERVAL: Duration = Duration::from_secs(2);

impl ViewerKeyframeLimit {
    /// 이 요청을 방송자 요청으로 이어도 되는지. 이으면 창이 새로 시작된다.
    fn allow(&mut self, now: Instant) -> bool {
        if self
            .last
            .is_some_and(|t| now.saturating_duration_since(t) < VIEWER_KEYFRAME_INTERVAL)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// 방송자 세션의 로그 구간(`PERIOD_LOG_INTERVAL`) 진단 값. 같은 값을 서버 합계 지표(`metrics.rs`)에도 더한다.
#[derive(Debug, Default)]
struct PublisherPeriod {
    /// `contiguous=false`로 온 비디오 프레임
    broken: u64,
    /// 시청자에게 보내지 않고 버린 비디오 프레임
    dropped: u64,
    /// 실제로 보낸 PLI
    pli_sent: u64,
    /// 받은 키프레임
    keyframes: u64,
    /// 방송자에게 보낸 NACK(str0m 수신 통계의 차이)
    nacks_sent: u64,
    /// 프레임의 첫 패킷이 도착한 때부터 SFU가 프레임을 꺼낸 때까지의 최댓값
    hold_max: Duration,
    /// 한 번의 `drive`에서 나온 비디오 프레임 수의 최댓값
    burst_max: u32,
    /// 수신 손실(최근 RR의 fraction_lost)의 최댓값
    loss_max: Option<f32>,
    /// 수신 RTT(RTCP XR DLRR)의 최댓값. 방송자가 DLRR에 응하지 않으면 없다.
    rtt_max: Option<Duration>,
}

impl PublisherPeriod {
    /// 로그에 남길 만한 일이 있었는지. 모두 0인 조용한 방송은 남기지 않는다. 키프레임을 받은 것만으로는 남기지
    /// 않는다(GOP마다 오는 정상 값). 붙잡은 시간·몰아 보낸 수는 평소에도 0이 아니라서 문턱을 둔다.
    fn noteworthy(&self) -> bool {
        self.broken > 0
            || self.dropped > 0
            || self.pli_sent > 0
            || self.nacks_sent > 0
            || self.hold_max >= HOLD_LOG_MIN
            || self.burst_max >= BURST_LOG_MIN
            || self.loss_max.is_some_and(|l| l > 0.0)
    }
}

/// 시청자 세션의 로그 구간 진단 값.
#[derive(Debug, Default)]
struct ViewerPeriod {
    /// 시청자에게서 받은 NACK
    nacks: u64,
    /// 받은 PLI + FIR
    pli_fir: u64,
    /// 비디오 RTX SSRC로 보낸 패킷(재전송)
    rtx_packets: u64,
    /// 2초 제한으로 무시한 키프레임 요청(RTCP PLI/FIR)
    ignored_rtcp: u64,
    /// 2초 제한으로 무시한 키프레임 요청(시그널링)
    ignored_signaling: u64,
    /// 시청자가 보고한 손실(RR의 fraction_lost, 구간 평균)의 최댓값
    loss_max: Option<f32>,
    /// 시청자 RR로 잰 RTT의 최댓값
    rtt_max: Option<Duration>,
}

impl ViewerPeriod {
    fn noteworthy(&self) -> bool {
        self.nacks > 0
            || self.pli_fir > 0
            || self.rtx_packets > 0
            || self.ignored_rtcp > 0
            || self.ignored_signaling > 0
            || self.loss_max.is_some_and(|l| l > 0.0)
    }
}

/// 누적 카운터의 직전 값과 새 값의 차이. 값이 줄었으면 스트림이 다시 만들어져 0부터 다시 센 것이므로(`forward`가
/// 첫 패킷이 안 나간 송신 스트림을 지우고 다시 선언한다) 새 값 전체가 늘어난 만큼이다.
fn counter_delta(prev: u64, cur: u64) -> u64 {
    if cur >= prev { cur - prev } else { cur }
}

/// mid마다 마지막으로 본 누적값(`seen`)과 `cur`의 차이를 구하고 `seen`을 갱신한다. 이 차이만 서버 합계에 더하면
/// 세션이 끝나도 합계가 줄지 않는다.
fn counter_deltas(seen: &mut HashMap<Mid, [u64; 3]>, mid: Mid, cur: [u64; 3]) -> [u64; 3] {
    let prev = seen.entry(mid).or_insert([0; 3]);
    let out = std::array::from_fn(|i| counter_delta(prev[i], cur[i]));
    *prev = cur;
    out
}

fn max_opt<T: PartialOrd + Copy>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if y > x { y } else { x }),
        (x, None) => x,
        (None, y) => y,
    }
}

/// 한 세션의 로그 구간 요약을 남기고 값을 비운다. 조용했으면 남기지 않는다.
fn log_period(s: &mut Session, window: Duration) {
    let window_ms = window.as_millis() as u64;
    match s.role {
        Role::Publisher => {
            let p = std::mem::take(&mut s.pub_period);
            if p.noteworthy() {
                info!(
                    broadcast = s.broadcast,
                    window_ms,
                    broken = p.broken,
                    dropped = p.dropped,
                    pli_sent = p.pli_sent,
                    keyframes = p.keyframes,
                    nacks_sent = p.nacks_sent,
                    hold_max_ms = p.hold_max.as_millis() as u64,
                    burst_max = p.burst_max,
                    loss_pct = p.loss_max.map(|l| (l * 1000.0).round() as f64 / 10.0),
                    rtt_max_ms = p.rtt_max.map(|r| r.as_millis() as u64),
                    "방송자 수신 진단: 끊김 프레임·버린 프레임·PLI·NACK·붙잡은 시간·몰아 보낸 프레임 수"
                );
            }
        }
        Role::Viewer => {
            let p = std::mem::take(&mut s.view_period);
            if p.noteworthy() {
                info!(
                    session = s.id,
                    broadcast = s.broadcast,
                    window_ms,
                    nacks = p.nacks,
                    pli_fir = p.pli_fir,
                    rtx_packets = p.rtx_packets,
                    ignored_rtcp = p.ignored_rtcp,
                    ignored_signaling = p.ignored_signaling,
                    loss_pct = p.loss_max.map(|l| (l * 1000.0).round() as f64 / 10.0),
                    rtt_max_ms = p.rtt_max.map(|r| r.as_millis() as u64),
                    "시청자 송신 진단: NACK·PLI/FIR·재전송·무시한 키프레임 요청·손실·RTT"
                );
            }
        }
    }
}

/// 방송자 시계 → 이 서버 Instant 기준점.
#[derive(Clone, Copy)]
struct Anchor {
    instant: Instant,
    system: SystemTime,
}

struct Sfu {
    socket: MeteredSocket,
    sessions: Vec<Session>,
    events: mpsc::UnboundedSender<SfuEvent>,
    /// broadcast → 기준점
    anchors: HashMap<u64, Anchor>,
    last_liveness_check: Instant,
    /// 방송·시청자별 진단 요약을 마지막으로 남긴 때(`PERIOD_LOG_INTERVAL`).
    last_period_log: Instant,
}

#[allow(clippy::large_enum_variant)] // 짧게 쓰고 버리는 목록이라 Box로 할당하지 않는다.
enum Fanout {
    Media(u64, MediaData),
    /// 방송의 키프레임 요청.
    Keyframe(u64),
    /// 방송에 시청자가 연결됐다. 그 방송자에게 키프레임을 바로(`force`) 요청한다.
    ViewerConnected(u64),
}

impl Sfu {
    fn run(&mut self, pkt_rx: Receiver<Packet>, cmd_rx: Receiver<SfuCmd>) {
        loop {
            let timeout = self.drive_all();
            let wait = timeout
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(50));
            select! {
                recv(pkt_rx) -> pkt => match pkt {
                    Ok(p) => {
                        self.handle_packet(p);
                        // 쌓인 패킷을 이어서 처리하되, 출력 처리가 너무 밀리지 않게 묶음 크기를 제한한다.
                        for p in pkt_rx.try_iter().take(64) {
                            self.handle_packet(p);
                        }
                    }
                    Err(_) => break,
                },
                recv(cmd_rx) -> cmd => match cmd {
                    Ok(SfuCmd::Shutdown) | Err(_) => break,
                    Ok(c) => self.handle_cmd(c),
                },
                default(wait) => {}
            }
            let now = Instant::now();
            for s in &mut self.sessions {
                if s.rtc.is_alive() {
                    let _ = s.rtc.handle_input(Input::Timeout(now));
                }
            }
            self.check_publisher_liveness(now);
        }
        for s in &mut self.sessions {
            s.rtc.disconnect();
        }
        let _ = self.drive_all();
        info!("SFU 종료");
    }

    /// 연결된 방송자 세션이 `PUBLISHER_SILENCE_LIMIT` 동안 조용하면 끊는다. 끊은 세션은 `drive_all`이
    /// 정리하면서 `SessionClosed`를 알린다(시그널링이 방송을 끝내고 시청자에게 알린다).
    ///
    /// 이 스레드가 멈췄다 돌아온 때(1초 넘게 확인하지 못함)는 `last_rx`가 낡아 보일 수 있으므로 그 확인은 건너뛴다.
    /// 건너뛰는 경우는 이것 하나다(D-108).
    fn check_publisher_liveness(&mut self, now: Instant) {
        let since = now.saturating_duration_since(self.last_liveness_check);
        if since < LIVENESS_CHECK_INTERVAL {
            return;
        }
        self.last_liveness_check = now;
        if since > Duration::from_secs(1) {
            return;
        }
        for s in &mut self.sessions {
            if s.role != Role::Publisher || !s.connected || !s.rtc.is_alive() {
                continue;
            }
            let silent = now.saturating_duration_since(s.last_rx);
            if silent > PUBLISHER_SILENCE_LIMIT {
                warn!(
                    session = s.id,
                    broadcast = s.broadcast,
                    silent_ms = silent.as_millis() as u64,
                    "방송자가 {}초 넘게 아무 패킷도 보내지 않아 세션을 닫습니다",
                    PUBLISHER_SILENCE_LIMIT.as_secs()
                );
                s.rtc.disconnect();
            }
        }
    }

    fn handle_cmd(&mut self, cmd: SfuCmd) {
        match cmd {
            SfuCmd::AddSession {
                session,
                broadcast,
                role,
                offer,
                candidate,
                reply,
            } => {
                let result = self.add_session(session, broadcast, role, &offer, candidate);
                let _ = reply.send(result);
            }
            SfuCmd::Remove { session } => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session) {
                    s.rtc.disconnect();
                }
            }
            SfuCmd::RequestKeyframe { broadcast, session } => {
                // 앱의 멈춤 감지 요청도 시청자 PLI와 같이 그 시청자의 2초 제한을 받고, `force` 없이 `KeyframeNeed`의
                // 500ms 간격·재시도를 따른다(D-087). 예전에는 `force`라 간격 제한을 모두 건너뛰었다.
                let Some(v) = self.sessions.iter_mut().find(|s| {
                    s.id == session && s.broadcast == broadcast && s.role == Role::Viewer
                }) else {
                    return;
                };
                if v.keyframe_limit.allow(Instant::now()) {
                    self.request_keyframe(broadcast, false);
                } else {
                    v.view_period.ignored_signaling += 1;
                    self.socket
                        .metrics
                        .sfu_keyframe_request_ignored(KeyframeSource::Signaling);
                    debug!(
                        session,
                        broadcast, "시청자의 키프레임 요청이 2초 안에 또 와서 무시함(멈춤 감지)"
                    );
                }
            }
            SfuCmd::Shutdown => {}
        }
    }

    fn add_session(
        &mut self,
        id: u64,
        broadcast: u64,
        role: Role,
        offer: &str,
        candidate: SocketAddr,
    ) -> Result<String, String> {
        let offer_text = offer;
        let offer = SdpOffer::from_sdp_string(offer).map_err(|e| format!("SDP 형식 오류: {e}"))?;
        // 비디오 트랙은 하나다(D-104). simulcast를 제안하는 방송자는 받지 않는다.
        if role == Role::Publisher && offer_text.lines().any(|l| l.starts_with("a=simulcast:")) {
            return Err("simulcast offer는 받지 않습니다(비디오 트랙 하나, D-104)".into());
        }
        let mut rtc = Rtc::builder()
            .set_ice_lite(true)
            .clear_codecs()
            // 오디오 RED(D-087): 방송자 세션(RED를 풀어 받음)과 시청자 세션(RED로 다시 감싸 보냄)이 같은 빌더다.
            .enable_opus(true, true)
            .set_red_distances(&AUDIO_RED_DISTANCES)
            // 잃은 오디오를 오래 기다리지 않는다(모듈 설명 "오디오 재정렬 대기", D-092).
            .set_reordering_size_audio(AUDIO_REORDER_FRAMES)
            // 비디오는 H.264만(D-103).
            .enable_h264(true)
            .set_rtcp_report_interval_audio(SR_INTERVAL)
            .set_rtcp_report_interval_video(SR_INTERVAL)
            .set_send_buffer_video(SEND_BUFFER_VIDEO)
            .set_stats_interval(Some(STATS_INTERVAL))
            .build(Instant::now());
        let cand = Candidate::host(candidate, "udp").map_err(|e| format!("후보 주소 오류: {e}"))?;
        rtc.add_local_candidate(cand);
        let answer = rtc
            .sdp_api()
            .accept_offer(offer)
            .map_err(|e| format!("SDP offer를 받지 못했습니다: {e}"))?;

        // 시청자의 송신 스트림은 첫 미디어를 쓸 때까지 지워 둔다(모듈 설명의 "미디어 전 SR").
        let pending_tx = match role {
            Role::Viewer => defer_stream_tx(&mut rtc, offer_text),
            Role::Publisher => HashMap::new(),
        };
        self.sessions.push(Session {
            id,
            broadcast,
            role,
            rtc,
            local: candidate,
            connected: false,
            in_mids: HashMap::new(),
            out_mids: HashMap::new(),
            pending_tx,
            keyframe: KeyframeNeed::default(),
            video_gate: VideoGate::default(),
            keyframe_limit: ViewerKeyframeLimit::default(),
            last_rx: Instant::now(),
            video_rtx: None,
            stats_seen: HashMap::new(),
            pub_period: PublisherPeriod::default(),
            view_period: ViewerPeriod::default(),
        });
        debug!(session = id, ?role, %candidate, "세션 추가");
        let sdp = answer.to_sdp_string();
        Ok(match role {
            Role::Viewer => unify_msid(&sdp, VIEWER_STREAM_ID),
            Role::Publisher => sdp,
        })
    }

    fn handle_packet(&mut self, p: Packet) {
        let len = p.data.len();
        let Ok(contents) = DatagramRecv::try_from(p.data.as_slice()) else {
            self.socket.metrics.udp_received(Peer::Unknown, len);
            return;
        };
        let probe = Input::Receive(
            p.at,
            Receive {
                proto: Protocol::Udp,
                source: p.source,
                destination: p.source, // accepts()는 목적지를 보지 않는다.
                contents,
            },
        );
        let Some(idx) = self.sessions.iter().position(|s| s.rtc.accepts(&probe)) else {
            self.socket.metrics.udp_received(Peer::Unknown, len);
            return;
        };
        let Input::Receive(_, mut receive) = probe else {
            return;
        };
        let session = &mut self.sessions[idx];
        self.socket.metrics.udp_received(session.role.into(), len);
        // str0m은 목적지가 자기 로컬 후보와 같아야 받는다. 소켓은 0.0.0.0에 바인딩되어 실제 목적지를
        // 알 수 없으므로, 이 세션에 알려 준 후보 주소를 목적지로 넘긴다.
        receive.destination = session.local;
        session.last_rx = session.last_rx.max(p.at);
        if let Err(e) = session.rtc.handle_input(Input::Receive(p.at, receive)) {
            debug!(session = session.id, "입력 오류: {e}");
            session.rtc.disconnect();
        }
    }

    /// 모든 세션의 출력을 처리하고 가장 이른 타임아웃을 돌려준다.
    fn drive_all(&mut self) -> Instant {
        let mut fanout: VecDeque<Fanout> = VecDeque::new();
        let mut next = Instant::now() + Duration::from_millis(100);
        for s in &mut self.sessions {
            let t = drive(s, &self.socket, &mut fanout);
            next = next.min(t);
        }
        // `forward`가 시청자 세션을 바로 구동하면서 새 항목을 더할 수 있다.
        while let Some(f) = fanout.pop_front() {
            match f {
                Fanout::Media(broadcast, data) => self.forward(broadcast, data, &mut fanout),
                Fanout::Keyframe(broadcast) => self.request_keyframe(broadcast, false),
                Fanout::ViewerConnected(broadcast) => self.request_keyframe(broadcast, true),
            }
        }
        // 키프레임이 아직 필요한 방송자에게 PLI를 (다시) 보낼 때가 됐는지 본다.
        let now = Instant::now();
        for s in &mut self.sessions {
            if s.role == Role::Publisher && s.keyframe.due(now) {
                send_pli(s, now, &self.socket.metrics);
            }
        }
        // 진단 요약(조용한 세션은 남기지 않는다). 세션이 끝날 때도 남은 구간을 남긴다.
        if now.saturating_duration_since(self.last_period_log) >= PERIOD_LOG_INTERVAL {
            let window = now.saturating_duration_since(self.last_period_log);
            self.last_period_log = now;
            for s in &mut self.sessions {
                log_period(s, window);
            }
        }
        let window = now.saturating_duration_since(self.last_period_log);
        // 끝난 세션 정리
        let mut i = 0;
        while i < self.sessions.len() {
            if self.sessions[i].rtc.is_alive() {
                i += 1;
                continue;
            }
            let mut s = self.sessions.swap_remove(i);
            log_period(&mut s, window);
            debug!(session = s.id, "세션 종료");
            if s.role == Role::Publisher {
                self.anchors.remove(&s.broadcast);
            }
            let _ = self.events.send(SfuEvent::SessionClosed { session: s.id });
        }
        let connected = |role| {
            self.sessions
                .iter()
                .filter(|s| s.role == role && s.connected)
                .count()
        };
        self.socket
            .metrics
            .set_webrtc_sessions(connected(Role::Publisher), connected(Role::Viewer));
        next
    }

    fn forward(&mut self, broadcast: u64, data: MediaData, fanout: &mut VecDeque<Fanout>) {
        let Some(kind) = self
            .sessions
            .iter()
            .find(|s| s.broadcast == broadcast && s.role == Role::Publisher)
            .and_then(|s| {
                s.in_mids
                    .iter()
                    .find(|(_, m)| **m == data.mid)
                    .map(|(k, _)| *k)
            })
        else {
            return;
        };
        let wallclock = self.wallclock(broadcast, &data);
        for s in &mut self.sessions {
            if s.broadcast != broadcast || s.role != Role::Viewer || !s.connected {
                continue;
            }
            let Some(mid) = s.out_mids.get(&kind).copied() else {
                continue;
            };
            let Some(pt) = s.rtc.writer(mid).and_then(|w| w.match_params(data.params)) else {
                continue;
            };
            // 이 트랙의 첫 미디어: 보류해 둔 송신 스트림을 선언하고 바로 쓴다.
            let fresh = s.pending_tx.remove(&mid);
            if fresh.is_some() {
                flush_tx_queues(s, &self.socket, fanout);
            }
            if let Some((ssrc, rtx)) = fresh {
                if kind == MediaKind::Video {
                    s.video_rtx = rtx;
                }
                // pacer 밖에 둔다. pacer가 첫 패킷을 붙잡으면 그 사이에 만든 SR이 "RTP 0"이 된다(아래). 방송자가 이미
                // pacing해서 보내므로 다시 늦출 필요도 없다. 시청자 세션에는 BWE가 없어(D-104) str0m이 pacing하지 않으므로
                // 지금은 안전장치다(모듈 설명 "미디어 전 SR").
                let mut api = s.rtc.direct_api();
                let tx = api.declare_stream_tx(ssrc, rtx, mid, None);
                tx.set_unpaced(true);
                if kind == MediaKind::Video {
                    // 캐시를 새로 만들므로 선언 직후, 첫 쓰기 전에 건다. 시청자 송신 스트림을 선언하는 곳은
                    // 여기뿐이다(`defer_stream_tx`가 SDP 협상 때 만든 것은 모두 지운다).
                    tx.set_rtx_cache(SEND_BUFFER_VIDEO, RTX_CACHE_AGE, Some(RTX_RATIO_CAP));
                }
            }
            let Some(writer) = s.rtc.writer(mid) else {
                continue;
            };
            if let Err(e) = writer.write(pt, wallclock, data.time, data.data.clone()) {
                debug!(session = s.id, "전달 실패: {e}");
                if let Some((ssrc, rtx)) = fresh {
                    // 쓰지 못한 스트림이 남으면 미디어 전 SR이 나간다. 다음 미디어까지 다시 보류한다.
                    s.rtc.direct_api().remove_stream_tx(ssrc);
                    s.pending_tx.insert(mid, (ssrc, rtx));
                }
                continue;
            }
            if let Some((ssrc, rtx)) = fresh {
                // 첫 패킷이 나가기 전에 SR이 만들어지면 RTP 시각을 알 수 없어 다시 "RTP 0"이 된다. str0m은
                // 새 스트림의 첫 타임아웃에서 종류만 정하고(SR 없음) 패킷을 내보낼 때 RTP 시계를 정하므로,
                // 여기서 타임아웃을 한 번 주고 곧바로 내보낸다.
                let _ = s.rtc.handle_input(Input::Timeout(Instant::now()));
                let (_, sent) = drive_watch(s, &self.socket, fanout, Some(ssrc));
                if !sent {
                    // 첫 패킷이 나가지 않았다. 다음 타임아웃이 "RTP 0" SR을 만들기 전에 스트림을 지우고 다음 미디어에서
                    // 다시 한다(비디오는 키프레임부터 다시 받는다).
                    debug!(
                        session = s.id,
                        ?kind,
                        "새 송신 스트림의 첫 패킷이 나가지 않아 다시 보류합니다"
                    );
                    s.rtc.direct_api().remove_stream_tx(ssrc);
                    s.pending_tx.insert(mid, (ssrc, rtx));
                    if kind == MediaKind::Video {
                        fanout.push_back(Fanout::Keyframe(broadcast));
                    }
                }
            }
        }
    }

    /// 프레임의 캡처 시각을 이 서버의 Instant로 옮긴다(위 모듈 설명 참고).
    fn wallclock(&mut self, broadcast: u64, data: &MediaData) -> Instant {
        // 방송자가 미디어를 쓰기 전에 보낸 SR(`sender_packet_count` 0)은 "RTP 0 ↔ 지금"이라 쓸 수 없다.
        let Some(sr) = data
            .last_sender_info
            .filter(|sr| sr.sender_packet_count > 0)
        else {
            return data.network_time;
        };
        let delta = data.time.as_seconds() - sr.rtp_time.rebase(data.time.frequency()).as_seconds();
        if !delta.is_finite() || delta.abs() > 30.0 {
            return data.network_time;
        }
        let capture = if delta >= 0.0 {
            sr.ntp_time + Duration::from_secs_f64(delta)
        } else {
            sr.ntp_time - Duration::from_secs_f64(-delta)
        };
        // 처음에는 이 서버의 시계(SystemTime↔Instant)로 옮긴다. 두 PC의 시계가 맞으면 시청 쪽 SR이
        // 실제 캡처 시각이 되어 화면에서 화면까지 지연을 잴 수 있다. 시계가 어긋나 미래로 가면 아래에서
        // 기준점을 앞당긴다.
        let anchor = *self.anchors.entry(broadcast).or_insert_with(|| Anchor {
            instant: Instant::now(),
            system: SystemTime::now(),
        });
        let mut w = match capture.duration_since(anchor.system) {
            Ok(d) => anchor.instant + d,
            Err(e) => anchor
                .instant
                .checked_sub(e.duration())
                .unwrap_or(anchor.instant),
        };
        if w > data.network_time {
            // 기준점의 지연이 실제보다 컸다. 가장 짧은 지연에 맞춰 기준점을 앞당긴다.
            let shift = w - data.network_time;
            // 이동은 처음 몇 번만 크고 곧 수렴한다. 20ms 이상(두 PC의 시계 차이 등)만 info로 남긴다.
            let shift_ms = shift.as_secs_f64() * 1000.0;
            if shift_ms >= 20.0 {
                info!(broadcast, shift_ms, "방송자 시각 기준점을 앞당김");
            } else {
                debug!(broadcast, shift_ms, "방송자 시각 기준점을 앞당김");
            }
            if let Some(a) = self.anchors.get_mut(&broadcast)
                && let Some(i) = a.instant.checked_sub(shift)
            {
                a.instant = i;
            }
            w = data.network_time;
        }
        w
    }

    /// 방송자에게 키프레임이 필요하다고 기록하고, 간격 제한이 허락하면 PLI를 보낸다. 못 보내면(방송자의
    /// 수신 스트림을 아직 모름) `drive_all`이 나중에 다시 시도한다.
    fn request_keyframe(&mut self, broadcast: u64, force: bool) {
        let Some(p) = self
            .sessions
            .iter_mut()
            .find(|s| s.broadcast == broadcast && s.role == Role::Publisher)
        else {
            return;
        };
        let now = Instant::now();
        p.keyframe.want();
        if p.keyframe.may_send(now, force) {
            send_pli(p, now, &self.socket.metrics);
        }
    }
}

/// (S)RTP 패킷의 SSRC. RTCP·STUN·DTLS면 `None`. SRTP도 RTP 머리는 암호화하지 않는다.
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

/// 이 세션의 송신 큐에 남은 패킷을 모두 내보낸다(새 송신 스트림을 선언하기 직전).
///
/// 다른 큐에 패킷이 남아 있으면 새 스트림의 첫 패킷이 첫 타임아웃 뒤에 나가지 못하고, 다음 타임아웃이 그 스트림의 SR을
/// 먼저 만들 수 있다. 아직 보낸 패킷이 없는 스트림의 SR은 "RTP 0 ↔ 지금"이라 시청 쪽 A/V가 어긋난다(모듈 설명 "미디어 전
/// SR"). 비운 뒤 새 스트림을 선언하고 타임아웃을 한 번 주면 그 첫 패킷이 바로 나간다(`forward`). 시청자 세션에는 BWE가
/// 없어(D-104) str0m이 pacing 없이 패킷을 바로 다 내보내므로 큐는 보통 이미 비어 있다(안전장치).
fn flush_tx_queues(s: &mut Session, socket: &MeteredSocket, fanout: &mut VecDeque<Fanout>) {
    let mids: Vec<Mid> = s
        .out_mids
        .values()
        .filter(|m| !s.pending_tx.contains_key(m))
        .copied()
        .collect();
    if mids.is_empty() {
        return;
    }
    for _ in 0..FLUSH_LIMIT {
        // 큐 정보는 pacer가 타임아웃 때 잰 값이다. 타임아웃을 먼저 주고 본다.
        let _ = s.rtc.handle_input(Input::Timeout(Instant::now()));
        let mut api = s.rtc.direct_api();
        let queued = mids.iter().any(|mid| {
            api.stream_tx_by_mid(*mid, None)
                .and_then(|tx| tx.queue_info().map(|q| q.packet_count()))
                .is_some_and(|n| n > 0)
        });
        if !queued {
            return;
        }
        drive(s, socket, fanout);
    }
    debug!(session = s.id, "송신 큐를 다 비우지 못했습니다");
}

/// `flush_tx_queues`가 한 번에 내보내는 최대 패킷 수(키프레임 몇 개 분량).
const FLUSH_LIMIT: usize = 1024;

/// 방송자 비디오에 PLI를 요청한다(RTCP는 다음 타임아웃에 나간다).
fn send_pli(p: &mut Session, now: Instant, metrics: &Metrics) {
    let sent = p
        .in_mids
        .get(&MediaKind::Video)
        .copied()
        .and_then(|mid| p.rtc.writer(mid))
        .is_some_and(|mut w| w.request_keyframe(None, KeyframeRequestKind::Pli).is_ok());
    if sent {
        p.keyframe.sent(now);
        metrics.sfu_pli_sent();
        p.pub_period.pli_sent += 1;
    } else {
        p.keyframe.failed(now);
    }
}

/// 시청자 세션의 송신 스트림을 모두 지우고 (mid → SSRC, RTX SSRC)로 돌려준다.
///
/// SDP 협상(`accept_offer`)은 answer에 알릴 SSRC를 정하려고 미디어마다 송신 스트림을 만든다. answer를
/// 만든 뒤라 SSRC는 이미 알려졌다. 스트림이 있으면 str0m이 미디어를 쓰기 전에도 SR을 보내므로 지워 두고,
/// 첫 미디어를 쓸 때 같은 SSRC·RTX로 다시 선언한다(`forward`).
fn defer_stream_tx(rtc: &mut Rtc, offer: &str) -> HashMap<Mid, (Ssrc, Option<Ssrc>)> {
    let mut api = rtc.direct_api();
    let mut out = HashMap::new();
    for mid in offer_mids(offer) {
        let Some(tx) = api.stream_tx_by_mid(mid, None) else {
            continue;
        };
        let (ssrc, rtx) = (tx.ssrc(), tx.rtx());
        if api.remove_stream_tx(ssrc) {
            out.insert(mid, (ssrc, rtx));
        }
    }
    out
}

/// SDP의 `a=mid:` 값들.
fn offer_mids(sdp: &str) -> Vec<Mid> {
    sdp.lines()
        .filter_map(|l| l.trim_end_matches([CR, LF]).strip_prefix("a=mid:"))
        .map(|m| Mid::from(m.trim()))
        .collect()
}

fn drive(s: &mut Session, socket: &MeteredSocket, fanout: &mut VecDeque<Fanout>) -> Instant {
    drive_watch(s, socket, fanout, None).0
}

/// `drive_watch` 한 바퀴를 마칠 때: 방송자 세션이 비디오 프레임을 하나라도 꺼냈으면 그 수를 기록한다.
fn finish_drive(s: &mut Session, socket: &MeteredSocket, video_frames: u32) {
    if s.role == Role::Publisher && video_frames > 0 {
        socket.metrics.sfu_frames_per_drive(video_frames);
        s.pub_period.burst_max = s.pub_period.burst_max.max(video_frames);
    }
}

/// `drive`와 같고, `watch` SSRC의 RTP 패킷을 내보냈는지도 돌려준다.
fn drive_watch(
    s: &mut Session,
    socket: &MeteredSocket,
    fanout: &mut VecDeque<Fanout>,
    watch: Option<Ssrc>,
) -> (Instant, bool) {
    let watch = watch.map(|s| *s);
    let mut seen = false;
    // 이 호출에서 나온 방송자 비디오 프레임 수(한 번에 내보낸 프레임 수, 지표 `dongsi_sfu_frames_per_drive`).
    let mut video_frames = 0u32;
    loop {
        if !s.rtc.is_alive() {
            finish_drive(s, socket, video_frames);
            return (Instant::now(), seen);
        }
        match s.rtc.poll_output() {
            Ok(Output::Timeout(t)) => {
                finish_drive(s, socket, video_frames);
                return (t, seen);
            }
            Ok(Output::Transmit(t)) => {
                if watch.is_some() || s.video_rtx.is_some() {
                    let ssrc = rtp_ssrc(&t.contents);
                    if watch.is_some() && ssrc == watch {
                        seen = true;
                    }
                    // 시청자 비디오의 RTX SSRC로 나간 패킷은 재전송으로 센다. str0m은 보낸 재전송 수를 공개 API로
                    // 주지 않는다(`send_stats.rs`의 `packets_resent`는 내부 값).
                    if s.role == Role::Viewer && ssrc.is_some() && ssrc == s.video_rtx.map(|r| *r) {
                        socket.metrics.sfu_rtx_packet();
                        s.view_period.rtx_packets += 1;
                    }
                }
                if let Err(e) = socket.send_to(s.role.into(), &t.contents, t.destination) {
                    debug!(session = s.id, "송신 실패: {e}");
                }
            }
            Ok(Output::Event(e)) => match e {
                Event::Connected => {
                    s.connected = true;
                    info!(session = s.id, role = ?s.role, "WebRTC 연결됨");
                    if s.role == Role::Viewer {
                        fanout.push_back(Fanout::ViewerConnected(s.broadcast));
                    }
                }
                Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                    s.rtc.disconnect();
                }
                Event::MediaAdded(m) => match s.role {
                    Role::Publisher => {
                        s.in_mids.insert(m.kind, m.mid);
                    }
                    Role::Viewer => {
                        s.out_mids.insert(m.kind, m.mid);
                    }
                },
                Event::MediaData(data) => {
                    if s.role == Role::Publisher {
                        let now = Instant::now();
                        let mut forward = true;
                        if s.in_mids.get(&MediaKind::Video) == Some(&data.mid) {
                            // 진단: 프레임을 붙잡은 시간 = 프레임의 첫 패킷이 도착한 때(`MediaData::network_time`,
                            // str0m `buffer_rx.rs` `first_network_time`은 프레임을 이룬 패킷들의 `received` 최솟값)부터
                            // 이 이벤트를 꺼낸 때까지. 프레임이 여러 패킷에 걸쳐 도착하는 시간, 재전송·순서 대기,
                            // SFU 스레드가 다른 일로 밀린 시간이 다 들어 있다. 오디오는 재지 않는다.
                            let hold = now.saturating_duration_since(data.network_time);
                            socket.metrics.sfu_frame_hold(hold);
                            s.pub_period.hold_max = s.pub_period.hold_max.max(hold);
                            video_frames += 1;
                            if !data.contiguous {
                                socket.metrics.sfu_broken_frame();
                                s.pub_period.broken += 1;
                            }
                            // 아래에서 버리는 프레임도 받은 것으로 본다.
                            if s.keyframe.video_arrived() {
                                send_pli(s, now, &socket.metrics);
                            }
                            if data.is_keyframe() {
                                socket.metrics.sfu_keyframe_received();
                                s.pub_period.keyframes += 1;
                                s.keyframe.satisfied();
                            } else if !data.contiguous {
                                fanout.push_back(Fanout::Keyframe(s.broadcast));
                            }
                            // 끊긴 뒤 키프레임까지 비디오를 보내지 않는다(`VideoGate`). 오디오는 항상 전달한다.
                            let key = data.is_keyframe();
                            let step = s.video_gate.on_frame(now, key, data.contiguous);
                            forward = step.forward;
                            if !forward {
                                socket.metrics.sfu_dropped_frame();
                                s.pub_period.dropped += 1;
                            }
                            match step.change {
                                Some(GateChange::Started) => debug!(
                                    broadcast = s.broadcast,
                                    "방송자 비디오가 끊겨 키프레임까지 시청자에게 비디오를 보내지 않음"
                                ),
                                Some(GateChange::Resumed { dropped, waited }) => info!(
                                    broadcast = s.broadcast,
                                    dropped,
                                    waited_ms = waited.as_millis() as u64,
                                    "키프레임이 와서 시청자에게 비디오 전달을 다시 시작"
                                ),
                                None => {}
                            }
                        }
                        if forward {
                            fanout.push_back(Fanout::Media(s.broadcast, data));
                        }
                    }
                }
                Event::MediaIngressStats(st) if s.role == Role::Publisher => {
                    // 방송자 비디오의 수신 통계. nacks·plis·firs는 세션 시작부터의 누적이고(`StreamRxStats`는 지우지
                    // 않는다, str0m `streams/receive.rs:137-154`), loss는 우리가 마지막으로 보낸 RR의 fraction_lost,
                    // rtt는 마지막 XR DLRR에서 온다(방송자가 응하지 않으면 `None`). loss·rtt는 구간의 최댓값을 남긴다.
                    if s.in_mids.get(&MediaKind::Video) == Some(&st.mid) {
                        let [nacks, _, _] =
                            counter_deltas(&mut s.stats_seen, st.mid, [st.nacks, st.plis, st.firs]);
                        socket.metrics.sfu_publisher_nacks_sent(nacks);
                        s.pub_period.nacks_sent += nacks;
                        s.pub_period.loss_max = max_opt(s.pub_period.loss_max, st.loss);
                        s.pub_period.rtt_max = max_opt(s.pub_period.rtt_max, st.rtt);
                    }
                }
                Event::MediaEgressStats(st) if s.role == Role::Viewer => {
                    // 시청자 송신 통계. nacks·plis·firs는 누적(`StreamTxStats`, str0m `streams/send_stats.rs:13-29`,
                    // 송신 스트림을 다시 선언하면 0부터), loss는 이번 구간에 받은 RR의 가중 평균(`None`이면 RR 없음),
                    // rtt는 마지막 RR에서 온다.
                    let [nacks, plis, firs] =
                        counter_deltas(&mut s.stats_seen, st.mid, [st.nacks, st.plis, st.firs]);
                    socket.metrics.sfu_viewer_feedback(nacks, plis + firs);
                    s.view_period.nacks += nacks;
                    s.view_period.pli_fir += plis + firs;
                    if s.out_mids.get(&MediaKind::Video) == Some(&st.mid) {
                        s.view_period.loss_max = max_opt(s.view_period.loss_max, st.loss);
                        s.view_period.rtt_max = max_opt(s.view_period.rtt_max, st.rtt);
                    }
                }
                Event::KeyframeRequest(_) if s.role == Role::Viewer => {
                    // 시청자 한 명의 PLI/FIR는 2초에 한 번만 방송자 요청이 된다(`ViewerKeyframeLimit`).
                    if s.keyframe_limit.allow(Instant::now()) {
                        fanout.push_back(Fanout::Keyframe(s.broadcast));
                    } else {
                        s.view_period.ignored_rtcp += 1;
                        socket
                            .metrics
                            .sfu_keyframe_request_ignored(KeyframeSource::Rtcp);
                        debug!(
                            session = s.id,
                            "시청자의 키프레임 요청이 2초 안에 또 와서 무시함(PLI/FIR)"
                        );
                    }
                }
                _ => {}
            },
            Err(e) => {
                warn!(session = s.id, "RTC 오류: {e}");
                s.rtc.disconnect();
                finish_drive(s, socket, video_frames);
                return (Instant::now(), seen);
            }
        }
    }
}

const CR: char = '\r';
const LF: char = '\n';
#[cfg(test)]
const CRLF: &str = "\r\n";

/// 시청자에게 보내는 트랙의 MediaStream ID.
const VIEWER_STREAM_ID: &str = "dongsi";

/// answer의 msid 스트림 ID를 하나로 맞춘다.
///
/// 브라우저(WebView2)는 같은 MediaStream에 속한 오디오·비디오 트랙만 서로 맞춰 재생한다(lip sync).
/// 시청자의 recvonly offer에는 msid가 없어서 str0m이 트랙마다 무작위 스트림 ID를 붙이므로, 여기서
/// 같은 ID로 바꾼다.
fn unify_msid(sdp: &str, stream: &str) -> String {
    let mut out = String::with_capacity(sdp.len() + 32);
    for line in sdp.split_inclusive(LF) {
        let body = line.trim_end_matches([CR, LF]);
        let ending = &line[body.len()..];
        if let Some(rest) = body.strip_prefix("a=msid:") {
            // a=msid:<stream> <track>
            match rest.split_once(' ') {
                Some((_, track)) => out.push_str(&format!("a=msid:{stream} {track}")),
                None => out.push_str(body),
            }
        } else if let Some(rest) = body.strip_prefix("a=ssrc:")
            && let Some((ssrc, attr)) = rest.split_once(' ')
            && let Some(value) = attr.strip_prefix("msid:")
        {
            // a=ssrc:<ssrc> msid:<stream> <track>
            match value.split_once(' ') {
                Some((_, track)) => out.push_str(&format!("a=ssrc:{ssrc} msid:{stream} {track}")),
                None => out.push_str(body),
            }
        } else if body.starts_with("a=msid-semantic:") {
            out.push_str(&format!("a=msid-semantic: WMS {stream}"));
        } else {
            out.push_str(body);
        }
        out.push_str(ending);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::metrics::sample_value;
    use dongsi_core::discovery::{DiscoveryResponse, PartAssembler, decode_part, encode_query};
    use dongsi_core::{Fingerprint, InstallId};
    use std::sync::RwLock;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn keyframe_need_limits_requests() {
        let t0 = Instant::now();
        let mut k = KeyframeNeed::default();
        // 처음에는 바로 보낼 수 있다.
        assert!(k.may_send(t0, false));
        k.want();
        k.sent(t0);
        // 보낸 PLI의 답을 기다리는 동안도 500ms 안에는 다시 보내지 않는다(force 제외).
        assert!(!k.may_send(t0 + ms(1), false));
        assert!(!k.may_send(t0 + ms(499), false));
        assert!(k.may_send(t0 + ms(500), false));
        assert!(k.may_send(t0 + ms(1), true));
        // 키프레임이 오면 대기를 끝내고, 이어지는 새 요청은 500ms 간격만 지키면 된다.
        k.satisfied();
        assert!(!k.due(t0 + ms(5000)));
        assert!(!k.may_send(t0 + ms(499), false));
        assert!(k.may_send(t0 + ms(500), false));
    }

    #[test]
    fn keyframe_need_retries_until_keyframe() {
        let t0 = Instant::now();
        let mut k = KeyframeNeed::default();
        assert!(!k.due(t0), "요청이 없으면 보내지 않는다");
        k.want();
        // 아직 PLI를 못 보냈다: 100ms마다 다시 시도한다.
        assert!(k.due(t0));
        k.failed(t0);
        assert!(!k.due(t0 + ms(99)));
        assert!(k.due(t0 + ms(100)));
        // 보냈는데 키프레임이 안 온다: 500ms마다 다시 보낸다.
        k.sent(t0 + ms(100));
        assert!(!k.due(t0 + ms(599)));
        assert!(k.due(t0 + ms(600)));
        k.sent(t0 + ms(600));
        assert!(!k.due(t0 + ms(1099)));
        assert!(k.due(t0 + ms(1100)));
        // 키프레임이 오면 멈춘다.
        k.satisfied();
        assert!(!k.due(t0 + ms(10_000)));
    }

    #[test]
    fn video_gate_drops_until_keyframe() {
        let t0 = Instant::now();
        let mut g = VideoGate::default();
        let fwd = GateStep {
            forward: true,
            change: None,
        };
        // 끊김이 없으면(처음 연결 직후 포함) 키프레임이 아니어도 전달한다.
        assert_eq!(g.on_frame(t0, false, true), fwd);
        assert!(g.on_frame(t0 + ms(16), true, true).forward);
        assert_eq!(g.on_frame(t0 + ms(33), false, true), fwd);
        // 끊긴 프레임: 버리고 상태를 켠다.
        let s = g.on_frame(t0 + ms(50), false, false);
        assert!(!s.forward);
        assert_eq!(s.change, Some(GateChange::Started));
        // 뒤따르는 P-프레임(contiguous여도)은 버린다. 같은 상태라 다시 켜지 않는다.
        for i in 1..=3 {
            let s = g.on_frame(t0 + ms(50 + 16 * i), false, i % 2 == 0);
            assert!(!s.forward);
            assert_eq!(s.change, None);
        }
        // 키프레임: 전달하고 상태를 끈다. 버린 프레임 수(끊긴 것 포함 4)와 기다린 시간이 나온다.
        let s = g.on_frame(t0 + ms(150), true, true);
        assert!(s.forward);
        assert_eq!(
            s.change,
            Some(GateChange::Resumed {
                dropped: 4,
                waited: ms(100)
            })
        );
        // 다시 정상 전달.
        assert_eq!(g.on_frame(t0 + ms(166), false, true), fwd);
    }

    #[test]
    fn video_gate_forwards_keyframe_even_if_not_contiguous() {
        let t0 = Instant::now();
        let mut g = VideoGate::default();
        // 끊김이 없던 상태에서 contiguous=false인 키프레임: 전달하고 상태는 켜지 않는다.
        let s = g.on_frame(t0, true, false);
        assert!(s.forward);
        assert_eq!(s.change, None);
        assert!(g.on_frame(t0 + ms(16), false, true).forward);
        // 기다리는 중에 contiguous=false인 키프레임이 와도 전달하고 끈다.
        assert!(!g.on_frame(t0 + ms(33), false, false).forward);
        let s = g.on_frame(t0 + ms(66), true, false);
        assert!(s.forward);
        assert!(matches!(
            s.change,
            Some(GateChange::Resumed { dropped: 1, .. })
        ));
        assert!(g.on_frame(t0 + ms(83), false, true).forward);
    }

    #[test]
    fn viewer_keyframe_limit_is_per_viewer_2s() {
        let t0 = Instant::now();
        let mut a = ViewerKeyframeLimit::default();
        let mut b = ViewerKeyframeLimit::default();
        // 첫 요청은 통과, 2초 안의 요청은 무시.
        assert!(a.allow(t0));
        assert!(!a.allow(t0 + ms(1)));
        assert!(!a.allow(t0 + ms(1900)));
        // 무시된 요청은 창을 늘리지 않는다: 첫 통과로부터 2초면 통과.
        assert!(a.allow(t0 + ms(2000)));
        assert!(!a.allow(t0 + ms(3999)));
        assert!(a.allow(t0 + ms(4000)));
        // 다른 시청자는 따로 센다.
        assert!(b.allow(t0 + ms(1900)));
        assert!(!b.allow(t0 + ms(2000)));
    }

    #[test]
    fn counter_delta_handles_resets() {
        assert_eq!(counter_delta(0, 5), 5);
        assert_eq!(counter_delta(5, 5), 0);
        assert_eq!(counter_delta(5, 9), 4);
        // 송신 스트림이 다시 만들어져 0부터 다시 셌다: 새 값 전체가 늘어난 만큼이다.
        assert_eq!(counter_delta(9, 3), 3);
    }

    #[test]
    fn cumulative_stats_add_only_differences_and_survive_session_end() {
        let m = Metrics::new("시험".into(), Arc::default());
        let mid = Mid::from("0");
        let audio = Mid::from("1");
        let total = |m: &Metrics| {
            let text = m.render();
            (
                sample_value(&text, "dongsi_sfu_viewer_nacks_received_total").unwrap(),
                sample_value(&text, "dongsi_sfu_viewer_pli_fir_received_total").unwrap(),
            )
        };
        let apply = |seen: &mut HashMap<Mid, [u64; 3]>, mid, cur: [u64; 3]| {
            let [nacks, plis, firs] = counter_deltas(seen, mid, cur);
            m.sfu_viewer_feedback(nacks, plis + firs);
        };

        // 시청자 A: 1초마다 누적값이 온다(NACK, PLI, FIR).
        let mut a = HashMap::new();
        apply(&mut a, mid, [4, 1, 0]);
        apply(&mut a, mid, [4, 1, 0]); // 변화 없음
        apply(&mut a, mid, [10, 1, 1]);
        apply(&mut a, audio, [0, 0, 0]); // 오디오 mid는 따로 센다
        // 시청자 B
        let mut b = HashMap::new();
        apply(&mut b, mid, [3, 2, 0]);
        assert_eq!(total(&m), (13, 4));

        // 시청자 A 세션이 끝나 (seen이 사라져도) 합계는 줄지 않는다.
        drop(a);
        assert_eq!(total(&m), (13, 4));
        // B는 이어서 늘어난 만큼만 더한다. 송신 스트림이 다시 만들어져 값이 줄면 새 값 전체를 더한다.
        apply(&mut b, mid, [5, 2, 0]);
        assert_eq!(total(&m), (15, 4));
        apply(&mut b, mid, [1, 0, 0]);
        assert_eq!(total(&m), (16, 4));
    }

    #[test]
    fn period_summary_skips_quiet_sessions() {
        // 모두 0인 방송자·시청자는 남기지 않는다.
        assert!(!PublisherPeriod::default().noteworthy());
        assert!(!ViewerPeriod::default().noteworthy());
        // 키프레임을 받은 것, 짧게 붙잡은 것, 프레임 1~2개씩 나온 것, 손실 0만으로는 남기지 않는다.
        let quiet = PublisherPeriod {
            keyframes: 3,
            hold_max: ms(30),
            burst_max: 2,
            loss_max: Some(0.0),
            rtt_max: Some(ms(40)),
            ..Default::default()
        };
        assert!(!quiet.noteworthy());
        for p in [
            PublisherPeriod {
                broken: 1,
                ..Default::default()
            },
            PublisherPeriod {
                dropped: 1,
                ..Default::default()
            },
            PublisherPeriod {
                pli_sent: 1,
                ..Default::default()
            },
            PublisherPeriod {
                nacks_sent: 1,
                ..Default::default()
            },
            PublisherPeriod {
                hold_max: HOLD_LOG_MIN,
                ..Default::default()
            },
            PublisherPeriod {
                burst_max: BURST_LOG_MIN,
                ..Default::default()
            },
            PublisherPeriod {
                loss_max: Some(0.02),
                ..Default::default()
            },
        ] {
            assert!(p.noteworthy(), "{p:?}");
        }
        let quiet = ViewerPeriod {
            loss_max: Some(0.0),
            rtt_max: Some(ms(5)),
            ..Default::default()
        };
        assert!(!quiet.noteworthy());
        for p in [
            ViewerPeriod {
                nacks: 1,
                ..Default::default()
            },
            ViewerPeriod {
                pli_fir: 1,
                ..Default::default()
            },
            ViewerPeriod {
                rtx_packets: 1,
                ..Default::default()
            },
            ViewerPeriod {
                ignored_rtcp: 1,
                ..Default::default()
            },
            ViewerPeriod {
                ignored_signaling: 1,
                ..Default::default()
            },
            ViewerPeriod {
                loss_max: Some(0.1),
                ..Default::default()
            },
        ] {
            assert!(p.noteworthy(), "{p:?}");
        }
        assert_eq!(max_opt(Some(1), None), Some(1));
        assert_eq!(max_opt(None, Some(2)), Some(2));
        assert_eq!(max_opt(Some(3), Some(2)), Some(3));
        assert_eq!(max_opt::<u8>(None, None), None);
    }

    #[test]
    fn keyframe_need_resends_once_when_media_starts() {
        let mut k = KeyframeNeed::default();
        // 미디어 전에 시청자가 와서 요청이 켜졌다.
        k.want();
        assert!(k.video_arrived(), "첫 비디오에서 한 번 더 보낸다");
        assert!(!k.video_arrived(), "한 번뿐이다");
        k.want();
        assert!(!k.video_arrived());

        // 미디어가 흐른 뒤에 켜진 요청은 해당하지 않는다.
        let mut k = KeyframeNeed::default();
        assert!(!k.video_arrived(), "요청이 없었으면 보내지 않는다");
        k.want();
        assert!(!k.video_arrived());
    }

    #[test]
    fn rtp_ssrc_reads_rtp_only() {
        let mut rtp = vec![0x80, 0x60, 0, 1, 0, 0, 0, 0, 0x12, 0x34, 0x56, 0x78, 0xAA];
        assert_eq!(rtp_ssrc(&rtp), Some(0x1234_5678));
        rtp[1] = 0xE0; // 마커 + PT 96
        assert_eq!(rtp_ssrc(&rtp), Some(0x1234_5678));
        let sr = [0x80, 200, 0, 6, 0x12, 0x34, 0x56, 0x78, 0, 0, 0, 0];
        assert_eq!(rtp_ssrc(&sr), None);
        let stun = [0x00, 0x01, 0, 0, 0x21, 0x12, 0xA4, 0x42, 0, 0, 0, 0];
        assert_eq!(rtp_ssrc(&stun), None);
        assert_eq!(rtp_ssrc(&[0x80, 0x60]), None);
    }

    #[test]
    fn msid_is_unified() {
        let sdp = [
            "v=0",
            "a=msid-semantic: WMS abc def",
            "m=video 9 UDP/TLS/RTP/SAVPF 96",
            "a=msid:abc t1",
            "a=ssrc:1 msid:abc t1",
            "a=ssrc:1 cname:x",
            "m=audio 9 UDP/TLS/RTP/SAVPF 111",
            "a=msid:def t2",
            "",
        ]
        .join(CRLF);
        let out = unify_msid(&sdp, "s");
        let lines: Vec<&str> = out.split(CRLF).collect();
        assert!(lines.contains(&"a=msid-semantic: WMS s"));
        assert!(lines.contains(&"a=msid:s t1"));
        assert!(lines.contains(&"a=ssrc:1 msid:s t1"));
        assert!(lines.contains(&"a=ssrc:1 cname:x"));
        assert!(lines.contains(&"a=msid:s t2"));
        assert_eq!(lines.len(), 9);
        assert!(!out.contains("abc") && !out.contains("def"));
    }

    /// 고장 4(H2): 감지 질의는 SFU 스레드를 거치지 않는다. 미디어 패킷이 SFU 쪽 큐를 가득 채우고
    /// 아무도 소비하지 않아도(SFU 스레드가 막힌 것과 같다) 읽기 스레드가 바로 응답한다.
    #[test]
    fn discovery_is_answered_without_the_sfu_thread() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        server
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let addr = server.local_addr().unwrap();
        let snapshot = Arc::new(RwLock::new(DiscoveryResponse {
            nonce: 0,
            fingerprint: Fingerprint::from_bytes([1; 32]),
            name: "시험".into(),
            port: addr.port(),
            host_only: false,
            broadcasts: (0..12)
                .map(|i| dongsi_core::discovery::DiscoveredBroadcast {
                    broadcaster: InstallId::from_bytes([i + 1; 16]),
                    name: "가".repeat(32),
                    width: 1920,
                    height: 1080,
                    fps: 60,
                    locked: false,
                })
                .collect(),
        }));
        // 큐 크기 1, 소비자 없음: 미디어 패킷 하나로 가득 찬다.
        let (tx, rx) = crossbeam_channel::bounded::<Packet>(1);
        let stop = Arc::new(AtomicBool::new(false));
        let metrics = Arc::new(Metrics::new("시험".into(), Arc::default()));
        let reader = {
            let stop = stop.clone();
            let responder = Some(Responder::new(snapshot));
            let socket = MeteredSocket {
                inner: server,
                metrics: metrics.clone(),
            };
            thread::spawn(move || read_loop(socket, tx, stop, responder))
        };

        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        for _ in 0..50 {
            client
                .send_to(&[0x80, 0x60, 0, 1, 0, 0, 0, 0], addr)
                .unwrap();
        }
        client.send_to(&encode_query(77), addr).unwrap();

        let mut assembler = PartAssembler::new();
        let mut buf = [0u8; 2048];
        while !assembler.is_complete() {
            let n = client.recv(&mut buf).expect("감지 응답이 오지 않았습니다");
            let part = decode_part(&buf[..n]).unwrap();
            assert_eq!(part.response.nonce, 77);
            assembler.push(part);
        }
        assert_eq!(assembler.merge().unwrap().broadcasts.len(), 12);
        assert!(
            rx.len() <= 1,
            "큐는 가득 차 있어야 합니다(미디어는 SFU로 갑니다)"
        );
        // 지표: 감지 질의를 받고 응답 조각을 보냈고, 큐에 못 들어간 미디어는 버린 것으로 센다.
        let text = metrics.render();
        let value = |series| crate::metrics::sample_value(&text, series).unwrap();
        assert_eq!(
            value("dongsi_udp_received_packets_total{kind=\"discovery\"}"),
            1
        );
        assert!(value("dongsi_udp_sent_packets_total{kind=\"discovery\"}") >= 1);
        assert!(value("dongsi_udp_dropped_packets_total") >= 1);

        stop.store(true, Ordering::Relaxed);
        reader.join().unwrap();
    }
}
