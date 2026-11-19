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
//!
//! ## 키프레임 요청
//! 방송자 세션마다 "키프레임이 필요함"을 기억한다(`KeyframeNeed`). 시청자 연결·시청자 PLI·수신 끊김으로
//! 켜고, PLI를 보낸 뒤 키프레임이 안 오면 1초마다 다시 보낸다. 방송자에게서 키프레임이 오면 끈다.
use std::collections::{HashMap, VecDeque};
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::{Receiver, Sender, select};
use dongsi_core::discovery::is_discovery_packet;
use str0m::change::SdpOffer;
use str0m::media::{KeyframeRequestKind, MediaData, MediaKind, Mid};
use str0m::net::{DatagramRecv, Protocol, Receive};
use str0m::rtp::Ssrc;
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use dongsi_net::publish::SR_INTERVAL;

use crate::discovery::{DiscoverySnapshot, Responder};

/// 연결된 방송자 세션이 이 시간 동안 어떤 패킷(STUN·DTLS·RTP·RTCP)도 보내지 않으면 끊긴 것으로 본다.
///
/// 이 서버는 ICE-lite라서 상대가 사라져도 상대의 STUN 요청이 15초(`is` 크레이트의 상수, 설정 불가)
/// 없을 때에야 str0m이 `Disconnected`를 낸다. 방송자는 미디어가 없을 때도 RTCP SR을 스트림마다
/// 0.5초(`SR_INTERVAL`)마다 보내므로(측정: 미디어 없음일 때 패킷 간격 최대 약 0.5~0.65초), 3초는 SR을
/// 연속 5번 넘게 못 받은 것이다. 시청자 세션과 연결되지 못한 세션에는 적용하지 않는다(ICE 15초 그대로).
const PUBLISHER_SILENCE_LIMIT: Duration = Duration::from_secs(3);
/// 방송자 침묵을 확인하는 간격.
const LIVENESS_CHECK_INTERVAL: Duration = Duration::from_millis(200);

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
    RequestKeyframe {
        broadcast: u64,
    },
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Publisher,
    Viewer,
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
) -> std::io::Result<SfuHandle> {
    dongsi_net::tune_udp_socket(&socket);
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    let reader_socket = socket.try_clone()?;
    let stop = Arc::new(AtomicBool::new(false));
    let (pkt_tx, pkt_rx) = crossbeam_channel::bounded::<Packet>(16_384);
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<SfuCmd>();

    let reader_stop = stop.clone();
    let responder = discovery.map(Responder::new);
    let reader = thread::Builder::new()
        .name("dongsi-sfu-recv".into())
        .spawn(move || read_loop(reader_socket, pkt_tx, reader_stop, responder))?;

    let runner = thread::Builder::new()
        .name("dongsi-sfu".into())
        .spawn(move || {
            let mut sfu = Sfu {
                socket,
                sessions: Vec::new(),
                events,
                anchors: HashMap::new(),
                last_liveness_check: Instant::now(),
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
    socket: UdpSocket,
    tx: Sender<Packet>,
    stop: Arc<AtomicBool>,
    mut responder: Option<Responder>,
) {
    let mut buf = vec![0u8; 2048];
    while !stop.load(Ordering::Relaxed) {
        match socket.recv_from(&mut buf) {
            Ok((n, source)) => {
                if is_discovery_packet(&buf[..n]) {
                    if let Some(responder) = &mut responder {
                        responder.handle(&buf[..n], source, Instant::now(), |bytes| {
                            let _ = socket.send_to(bytes, source);
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
                if tx.try_send(packet).is_err() && tx.is_empty() {
                    break;
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
    /// 이 세션으로 마지막 패킷이 도착한 때(읽기 스레드가 받은 시각). 만들어진 때로 시작한다.
    last_rx: Instant,
}

/// 방송자에게 키프레임이 필요한지와 PLI를 보낸 기록.
///
/// - 켜는 때: 시청자 연결(`force`), 시청자의 PLI·수신 끊김(`force` 아님).
/// - 끄는 때: 방송자에게서 키프레임이 왔다. 켜진 뒤 도착한 키프레임은 연결된 모든 시청자에게 전달된다.
/// - PLI를 못 보냈으면(수신 스트림을 아직 모름) 짧은 간격으로 다시 시도하고, 보냈는데 키프레임이 안
///   오면 1초 뒤에 다시 보낸다(PLI 유실 대비). `force`가 아니면 PLI가 500ms(보낸 PLI의 답을 기다리는
///   중이면 1초)보다 자주 나가지 않는다.
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
/// 보낸 PLI에 키프레임이 안 올 때 다시 보내는 간격.
const KEYFRAME_RETRY: Duration = Duration::from_secs(1);
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
        let gap = if self.requested {
            KEYFRAME_RETRY
        } else {
            KEYFRAME_MIN_GAP
        };
        now.saturating_duration_since(t) >= gap
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

/// 방송자 시계 → 이 서버 Instant 기준점.
#[derive(Clone, Copy)]
struct Anchor {
    instant: Instant,
    system: SystemTime,
}

struct Sfu {
    socket: UdpSocket,
    sessions: Vec<Session>,
    events: mpsc::UnboundedSender<SfuEvent>,
    /// broadcast → 기준점
    anchors: HashMap<u64, Anchor>,
    last_liveness_check: Instant,
}

#[allow(clippy::large_enum_variant)] // 짧게 쓰고 버리는 목록이라 Box로 할당하지 않는다.
enum Fanout {
    Media(u64, MediaData),
    Keyframe(u64),
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
            self.check_publisher_liveness(now, pkt_rx.is_empty());
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
    /// 이 스레드가 멈췄다 돌아온 때(1초 넘게 확인하지 못함)와 처리하지 않은 패킷이 쌓여 있을 때는
    /// `last_rx`가 낡아 보일 수 있으므로 그 확인은 건너뛴다.
    fn check_publisher_liveness(&mut self, now: Instant, queue_empty: bool) {
        let since = now.saturating_duration_since(self.last_liveness_check);
        if since < LIVENESS_CHECK_INTERVAL {
            return;
        }
        self.last_liveness_check = now;
        if since > Duration::from_secs(1) || !queue_empty {
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
            SfuCmd::RequestKeyframe { broadcast } => self.request_keyframe(broadcast, true),
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
        let mut rtc = Rtc::builder()
            .set_ice_lite(true)
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
            last_rx: Instant::now(),
        });
        debug!(session = id, ?role, %candidate, "세션 추가");
        let sdp = answer.to_sdp_string();
        Ok(match role {
            Role::Viewer => unify_msid(&sdp, VIEWER_STREAM_ID),
            Role::Publisher => sdp,
        })
    }

    fn handle_packet(&mut self, p: Packet) {
        let Ok(contents) = DatagramRecv::try_from(p.data.as_slice()) else {
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
            return;
        };
        let Input::Receive(_, mut receive) = probe else {
            return;
        };
        let session = &mut self.sessions[idx];
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
                send_pli(s, now);
            }
        }
        // 끝난 세션 정리
        let mut i = 0;
        while i < self.sessions.len() {
            if self.sessions[i].rtc.is_alive() {
                i += 1;
                continue;
            }
            let s = self.sessions.swap_remove(i);
            debug!(session = s.id, "세션 종료");
            if s.role == Role::Publisher {
                self.anchors.remove(&s.broadcast);
            }
            let _ = self.events.send(SfuEvent::SessionClosed { session: s.id });
        }
        next
    }

    fn forward(&mut self, broadcast: u64, data: MediaData, fanout: &mut VecDeque<Fanout>) {
        let kind = match self
            .sessions
            .iter()
            .find(|s| s.broadcast == broadcast && s.role == Role::Publisher)
            .and_then(|s| {
                s.in_mids
                    .iter()
                    .find(|(_, m)| **m == data.mid)
                    .map(|(k, _)| *k)
            }) {
            Some(k) => k,
            None => return,
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
            if let Some((ssrc, rtx)) = fresh {
                s.rtc.direct_api().declare_stream_tx(ssrc, rtx, mid, None);
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
            if fresh.is_some() {
                // 첫 패킷이 나가기 전에 SR이 만들어지면 RTP 시각을 알 수 없어 다시 "RTP 0"이 된다. str0m은
                // 새 스트림의 첫 타임아웃에서 종류만 정하고(SR 없음) 패킷을 내보낼 때 RTP 시계를 정하므로,
                // 여기서 타임아웃을 한 번 주고 곧바로 내보낸다.
                let _ = s.rtc.handle_input(Input::Timeout(Instant::now()));
                drive(s, &self.socket, fanout);
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
            send_pli(p, now);
        }
    }
}

/// 방송자의 비디오에 PLI를 요청한다(RTCP는 다음 타임아웃에 나간다).
fn send_pli(p: &mut Session, now: Instant) {
    let sent = p
        .in_mids
        .get(&MediaKind::Video)
        .copied()
        .and_then(|mid| p.rtc.writer(mid))
        .is_some_and(|mut w| w.request_keyframe(None, KeyframeRequestKind::Pli).is_ok());
    if sent {
        p.keyframe.sent(now);
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

fn drive(s: &mut Session, socket: &UdpSocket, fanout: &mut VecDeque<Fanout>) -> Instant {
    loop {
        if !s.rtc.is_alive() {
            return Instant::now();
        }
        match s.rtc.poll_output() {
            Ok(Output::Timeout(t)) => return t,
            Ok(Output::Transmit(t)) => {
                if let Err(e) = socket.send_to(&t.contents, t.destination) {
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
                        if data.mid == *s.in_mids.get(&MediaKind::Video).unwrap_or(&data.mid) {
                            if s.keyframe.video_arrived() {
                                send_pli(s, Instant::now());
                            }
                            if data.is_keyframe() {
                                s.keyframe.satisfied();
                            } else if !data.contiguous {
                                fanout.push_back(Fanout::Keyframe(s.broadcast));
                            }
                        }
                        fanout.push_back(Fanout::Media(s.broadcast, data));
                    }
                }
                Event::KeyframeRequest(_) if s.role == Role::Viewer => {
                    fanout.push_back(Fanout::Keyframe(s.broadcast));
                }
                _ => {}
            },
            Err(e) => {
                warn!(session = s.id, "RTC 오류: {e}");
                s.rtc.disconnect();
                return Instant::now();
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

    use dongsi_core::discovery::{DiscoveryResponse, PartAssembler, decode_part, encode_query};
    use dongsi_core::{Fingerprint, InstallId, VideoCodec};
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
        // 보낸 PLI의 답을 기다리는 동안은 1초 안에 다시 보내지 않는다(force 제외).
        assert!(!k.may_send(t0 + ms(500), false));
        assert!(!k.may_send(t0 + ms(999), false));
        assert!(k.may_send(t0 + ms(1000), false));
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
        // 보냈는데 키프레임이 안 온다: 1초마다 다시 보낸다.
        k.sent(t0 + ms(100));
        assert!(!k.due(t0 + ms(1099)));
        assert!(k.due(t0 + ms(1100)));
        k.sent(t0 + ms(1100));
        assert!(!k.due(t0 + ms(2099)));
        assert!(k.due(t0 + ms(2100)));
        // 키프레임이 오면 멈춘다.
        k.satisfied();
        assert!(!k.due(t0 + ms(10_000)));
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
                    codec: VideoCodec::H264,
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
        let reader = {
            let stop = stop.clone();
            let responder = Some(Responder::new(snapshot));
            thread::spawn(move || read_loop(server, tx, stop, responder))
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

        stop.store(true, Ordering::Relaxed);
        reader.join().unwrap();
    }
}
