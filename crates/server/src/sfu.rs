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

use std::collections::HashMap;
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
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use dongsi_net::publish::SR_INTERVAL;

use crate::discovery::{DiscoverySnapshot, Responder};

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
    last_keyframe_request: Option<Instant>,
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
        }
        for s in &mut self.sessions {
            s.rtc.disconnect();
        }
        let _ = self.drive_all();
        info!("SFU 종료");
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

        // 시청자는 offer에 있는 수신 트랙을 기억해 둔다(MediaAdded 이벤트로도 오지만 미리 채운다).
        self.sessions.push(Session {
            id,
            broadcast,
            role,
            rtc,
            local: candidate,
            connected: false,
            in_mids: HashMap::new(),
            out_mids: HashMap::new(),
            last_keyframe_request: None,
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
        if let Err(e) = session.rtc.handle_input(Input::Receive(p.at, receive)) {
            debug!(session = session.id, "입력 오류: {e}");
            session.rtc.disconnect();
        }
    }

    /// 모든 세션의 출력을 처리하고 가장 이른 타임아웃을 돌려준다.
    fn drive_all(&mut self) -> Instant {
        let mut fanout: Vec<Fanout> = Vec::new();
        let mut next = Instant::now() + Duration::from_millis(100);
        for s in &mut self.sessions {
            let t = drive(s, &self.socket, &mut fanout);
            next = next.min(t);
        }
        for f in fanout {
            match f {
                Fanout::Media(broadcast, data) => self.forward(broadcast, data),
                Fanout::Keyframe(broadcast) => self.request_keyframe(broadcast, false),
                Fanout::ViewerConnected(broadcast) => self.request_keyframe(broadcast, true),
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

    fn forward(&mut self, broadcast: u64, data: MediaData) {
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
            let Some(writer) = s.rtc.writer(mid) else {
                continue;
            };
            let Some(pt) = writer.match_params(data.params) else {
                continue;
            };
            if let Err(e) = writer.write(pt, wallclock, data.time, data.data.clone()) {
                debug!(session = s.id, "전달 실패: {e}");
            }
        }
    }

    /// 프레임의 캡처 시각을 이 서버의 Instant로 옮긴다(위 모듈 설명 참고).
    fn wallclock(&mut self, broadcast: u64, data: &MediaData) -> Instant {
        let Some(sr) = &data.last_sender_info else {
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
            if let Some(a) = self.anchors.get_mut(&broadcast)
                && let Some(i) = a.instant.checked_sub(shift)
            {
                a.instant = i;
            }
            w = data.network_time;
        }
        w
    }

    fn request_keyframe(&mut self, broadcast: u64, force: bool) {
        let Some(p) = self
            .sessions
            .iter_mut()
            .find(|s| s.broadcast == broadcast && s.role == Role::Publisher)
        else {
            return;
        };
        if !force
            && p.last_keyframe_request
                .is_some_and(|t| t.elapsed() < Duration::from_millis(500))
        {
            return;
        }
        let Some(mid) = p.in_mids.get(&MediaKind::Video).copied() else {
            return;
        };
        if let Some(mut writer) = p.rtc.writer(mid)
            && writer
                .request_keyframe(None, KeyframeRequestKind::Pli)
                .is_ok()
        {
            p.last_keyframe_request = Some(Instant::now());
        }
    }
}

fn drive(s: &mut Session, socket: &UdpSocket, fanout: &mut Vec<Fanout>) -> Instant {
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
                        fanout.push(Fanout::ViewerConnected(s.broadcast));
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
                        if !data.contiguous
                            && data.mid == *s.in_mids.get(&MediaKind::Video).unwrap_or(&data.mid)
                        {
                            fanout.push(Fanout::Keyframe(s.broadcast));
                        }
                        fanout.push(Fanout::Media(s.broadcast, data));
                    }
                }
                Event::KeyframeRequest(_) if s.role == Role::Viewer => {
                    fanout.push(Fanout::Keyframe(s.broadcast));
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
