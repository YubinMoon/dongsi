//! Prometheus 지표(D-083).
//!
//! 서버 코어(시그널링·SFU)가 세고, 전용 서버가 HTTP `GET /metrics`로 내보낸다(`apps/server`의 `metrics.rs`).
//! 내장 서버도 같이 세지만 내보내지는 않는다.
//!
//! - 값은 모두 서버 전체의 합이다. 방송자·시청자 이름이나 방송마다 새로 생기는 라벨은 넣지 않는다.
//! - UDP 바이트는 UDP 페이로드(IP·UDP 헤더 제외), 시그널링 바이트는 WebSocket 메시지 내용(TLS·WebSocket
//!   헤더 제외)이다. 회선에 실제로 흐르는 양은 이보다 조금 많다.
//! - 텍스트는 Prometheus exposition format 0.0.4다.
//! - `dongsi_sfu_*`는 SFU의 끊김·키프레임·재전송 진단 값이다(D-087). 방송자 프레임이 끊겼는지, SFU가 프레임을
//!   붙잡았다 몰아 보내는지, 시청자 쪽 손실로 NACK·재전송이 늘었는지를 서버 전체 합으로 본다. 붙잡은 시간과
//!   몰아 보낸 프레임 수는 histogram이다. 방송·시청자별 값은 라벨이 아니라 서버 로그(`sfu.rs`, 10초 요약)에 남긴다.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dongsi_core::proto::{ErrorCode, PROTOCOL_VERSION};

use crate::ServerStats;

/// UDP 패킷의 상대(라벨 `kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Peer {
    /// 방송자의 WebRTC 세션
    Publisher,
    /// 시청자의 WebRTC 세션
    Viewer,
    /// 로컬 감지 질의·응답(D-030)
    Discovery,
    /// 어느 세션의 것도 아닌 패킷(끝난 세션에 늦게 온 패킷, 형식이 틀린 패킷). 받은 쪽만 있다.
    Unknown,
}

impl Peer {
    const ALL: [Peer; 4] = [
        Peer::Publisher,
        Peer::Viewer,
        Peer::Discovery,
        Peer::Unknown,
    ];

    fn label(self) -> &'static str {
        match self {
            Peer::Publisher => "publisher",
            Peer::Viewer => "viewer",
            Peer::Discovery => "discovery",
            Peer::Unknown => "unknown",
        }
    }
}

/// Welcome 전에 끝난 접속의 까닭(라벨 `reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnFailure {
    /// 접속 수 상한에 걸림
    Limit,
    /// TLS 핸드셰이크 실패·시간 초과. 포트 검사, `--healthcheck`(TCP만 열고 닫음), 지문이 달라 클라이언트가
    /// 끊은 경우도 여기에 든다.
    Tls,
    /// WebSocket 업그레이드나 Hello 실패·시간 초과
    Handshake,
    /// 프로토콜 버전이 다름(앱과 서버 버전이 맞지 않음, D-078)
    Protocol,
}

impl ConnFailure {
    const ALL: [ConnFailure; 4] = [
        ConnFailure::Limit,
        ConnFailure::Tls,
        ConnFailure::Handshake,
        ConnFailure::Protocol,
    ];

    fn label(self) -> &'static str {
        match self {
            ConnFailure::Limit => "limit",
            ConnFailure::Tls => "tls",
            ConnFailure::Handshake => "handshake",
            ConnFailure::Protocol => "protocol",
        }
    }
}

/// 오류 응답을 세는 자리. 오류 코드를 더하면 여기와 [`error_label`]에 같이 더한다.
const ERROR_CODES: [ErrorCode; 10] = [
    ErrorCode::BadRequest,
    ErrorCode::ProtocolMismatch,
    ErrorCode::NotAllowed,
    ErrorCode::AlreadyPublishing,
    ErrorCode::NotFound,
    ErrorCode::PasswordRequired,
    ErrorCode::WrongPassword,
    ErrorCode::RateLimited,
    ErrorCode::ServerFull,
    ErrorCode::Internal,
];

/// 오류 코드 라벨(라벨 `code`).
fn error_label(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::BadRequest => "bad_request",
        ErrorCode::ProtocolMismatch => "protocol_mismatch",
        ErrorCode::NotAllowed => "not_allowed",
        ErrorCode::AlreadyPublishing => "already_publishing",
        ErrorCode::NotFound => "not_found",
        ErrorCode::PasswordRequired => "password_required",
        ErrorCode::WrongPassword => "wrong_password",
        ErrorCode::RateLimited => "rate_limited",
        ErrorCode::ServerFull => "server_full",
        ErrorCode::Internal => "internal",
    }
}

/// 바이트와 패킷 수.
#[derive(Debug, Default)]
struct Traffic {
    bytes: AtomicU64,
    packets: AtomicU64,
}

impl Traffic {
    fn add(&self, bytes: usize) {
        self.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
        self.packets.fetch_add(1, Ordering::Relaxed);
    }
}

/// 무시한 키프레임 요청의 출처(라벨 `source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyframeSource {
    /// 시청자의 RTCP PLI/FIR
    Rtcp,
    /// 앱의 멈춤 감지가 시그널링으로 보낸 `RequestKeyframe`
    Signaling,
}

impl KeyframeSource {
    const ALL: [KeyframeSource; 2] = [KeyframeSource::Rtcp, KeyframeSource::Signaling];

    fn label(self) -> &'static str {
        match self {
            KeyframeSource::Rtcp => "rtcp",
            KeyframeSource::Signaling => "signaling",
        }
    }
}

/// 프레임을 붙잡은 시간 histogram의 버킷 상한(초). 끝에 +Inf가 더해진다.
const HOLD_BUCKETS: [f64; 9] = [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5];
/// 한 번에 내보낸 프레임 수 histogram의 버킷 상한.
const BURST_BUCKETS: [f64; 6] = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0];

/// Prometheus histogram. 값을 넣는 쪽(SFU 스레드)은 버킷 하나와 합계에 원자적 덧셈 두 번만 한다.
#[derive(Debug)]
struct Histogram {
    /// 버킷 상한(오름차순). `buckets`는 이보다 하나 길다(마지막이 +Inf).
    bounds: &'static [f64],
    /// 버킷마다의 개수(누적이 아님. 렌더할 때 누적한다).
    buckets: Vec<AtomicU64>,
    /// 합계를 `scale`배 한 정수(실수를 원자적으로 더하지 않으려고).
    sum_scaled: AtomicU64,
    scale: f64,
}

impl Histogram {
    fn new(bounds: &'static [f64], scale: f64) -> Self {
        Self {
            bounds,
            buckets: (0..=bounds.len()).map(|_| AtomicU64::new(0)).collect(),
            sum_scaled: AtomicU64::new(0),
            scale,
        }
    }

    fn observe(&self, value: f64) {
        let i = self
            .bounds
            .iter()
            .position(|b| value <= *b)
            .unwrap_or(self.bounds.len());
        self.buckets[i].fetch_add(1, Ordering::Relaxed);
        self.sum_scaled
            .fetch_add((value * self.scale).round() as u64, Ordering::Relaxed);
    }
}

/// SFU 진단 지표(D-087). 모두 서버 전체의 합이다.
#[derive(Debug)]
struct SfuCounters {
    broken_frames: AtomicU64,
    dropped_frames: AtomicU64,
    pli_sent: AtomicU64,
    keyframes_received: AtomicU64,
    publisher_nacks_sent: AtomicU64,
    viewer_nacks_received: AtomicU64,
    viewer_pli_fir_received: AtomicU64,
    rtx_packets: AtomicU64,
    keyframe_requests_ignored: [AtomicU64; KeyframeSource::ALL.len()],
    frame_hold: Histogram,
    frames_per_drive: Histogram,
}

impl SfuCounters {
    fn new() -> Self {
        Self {
            broken_frames: AtomicU64::new(0),
            dropped_frames: AtomicU64::new(0),
            pli_sent: AtomicU64::new(0),
            keyframes_received: AtomicU64::new(0),
            publisher_nacks_sent: AtomicU64::new(0),
            viewer_nacks_received: AtomicU64::new(0),
            viewer_pli_fir_received: AtomicU64::new(0),
            rtx_packets: AtomicU64::new(0),
            keyframe_requests_ignored: Default::default(),
            // 합계는 초를 마이크로초로, 프레임 수는 그대로 센다.
            frame_hold: Histogram::new(&HOLD_BUCKETS, 1_000_000.0),
            frames_per_drive: Histogram::new(&BURST_BUCKETS, 1.0),
        }
    }
}

/// 서버 지표. [`crate::RunningServer::metrics`]로 얻고 [`Metrics::render`]로 텍스트를 만든다.
///
/// 접속·방송·시청 수는 시그널링이 갱신하는 [`ServerStats`]를 그대로 읽고, 나머지는 누적값이다.
#[derive(Debug)]
pub struct Metrics {
    name: String,
    started_at: u64,
    stats: Arc<RwLock<ServerStats>>,
    webrtc_publishers: AtomicU64,
    webrtc_viewers: AtomicU64,
    connections: AtomicU64,
    connection_failures: [AtomicU64; ConnFailure::ALL.len()],
    broadcasts: AtomicU64,
    watches: AtomicU64,
    request_errors: [AtomicU64; ERROR_CODES.len()],
    signaling_rx: AtomicU64,
    signaling_tx: AtomicU64,
    udp_rx: [Traffic; Peer::ALL.len()],
    udp_tx: [Traffic; Peer::ALL.len()],
    udp_dropped: AtomicU64,
    udp_send_errors: AtomicU64,
    sfu: SfuCounters,
}

impl Metrics {
    pub(crate) fn new(name: String, stats: Arc<RwLock<ServerStats>>) -> Self {
        Self {
            name,
            started_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            stats,
            webrtc_publishers: AtomicU64::new(0),
            webrtc_viewers: AtomicU64::new(0),
            connections: AtomicU64::new(0),
            connection_failures: Default::default(),
            broadcasts: AtomicU64::new(0),
            watches: AtomicU64::new(0),
            request_errors: Default::default(),
            signaling_rx: AtomicU64::new(0),
            signaling_tx: AtomicU64::new(0),
            udp_rx: Default::default(),
            udp_tx: Default::default(),
            udp_dropped: AtomicU64::new(0),
            udp_send_errors: AtomicU64::new(0),
            sfu: SfuCounters::new(),
        }
    }

    /// 접속이 Welcome까지 갔다.
    pub(crate) fn connection_accepted(&self) {
        self.connections.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn connection_failed(&self, reason: ConnFailure) {
        self.connection_failures[reason as usize].fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn broadcast_started(&self) {
        self.broadcasts.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn watch_started(&self) {
        self.watches.fetch_add(1, Ordering::Relaxed);
    }

    /// 클라이언트에게 오류 응답을 보냈다.
    pub(crate) fn request_error(&self, code: ErrorCode) {
        if let Some(i) = ERROR_CODES.iter().position(|c| *c == code) {
            self.request_errors[i].fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn signaling_received(&self, bytes: usize) {
        self.signaling_rx.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    pub(crate) fn signaling_sent(&self, bytes: usize) {
        self.signaling_tx.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    pub(crate) fn udp_received(&self, peer: Peer, bytes: usize) {
        self.udp_rx[peer as usize].add(bytes);
    }

    pub(crate) fn udp_sent(&self, peer: Peer, bytes: usize) {
        self.udp_tx[peer as usize].add(bytes);
    }

    /// SFU 스레드가 밀려 받은 패킷을 버렸다.
    pub(crate) fn udp_dropped(&self) {
        self.udp_dropped.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn udp_send_failed(&self) {
        self.udp_send_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// 방송자의 비디오 프레임이 `contiguous=false`로 왔다(앞 프레임을 잃음).
    pub(crate) fn sfu_broken_frame(&self) {
        self.sfu.broken_frames.fetch_add(1, Ordering::Relaxed);
    }

    /// 끊긴 뒤 키프레임까지 시청자에게 보내지 않고 버린 비디오 프레임.
    pub(crate) fn sfu_dropped_frame(&self) {
        self.sfu.dropped_frames.fetch_add(1, Ordering::Relaxed);
    }

    /// 방송자에게 PLI를 실제로 보냈다.
    pub(crate) fn sfu_pli_sent(&self) {
        self.sfu.pli_sent.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn sfu_keyframe_received(&self) {
        self.sfu.keyframes_received.fetch_add(1, Ordering::Relaxed);
    }

    /// 방송자에게 보낸 NACK이 `n`개 늘었다(세션의 누적 통계에서 구한 차이).
    pub(crate) fn sfu_publisher_nacks_sent(&self, n: u64) {
        self.sfu
            .publisher_nacks_sent
            .fetch_add(n, Ordering::Relaxed);
    }

    /// 시청자에게서 받은 NACK·PLI/FIR이 늘었다(세션의 누적 통계에서 구한 차이).
    pub(crate) fn sfu_viewer_feedback(&self, nacks: u64, pli_fir: u64) {
        self.sfu
            .viewer_nacks_received
            .fetch_add(nacks, Ordering::Relaxed);
        self.sfu
            .viewer_pli_fir_received
            .fetch_add(pli_fir, Ordering::Relaxed);
    }

    /// 시청자에게 RTX SSRC로 패킷 하나를 보냈다.
    pub(crate) fn sfu_rtx_packet(&self) {
        self.sfu.rtx_packets.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn sfu_keyframe_request_ignored(&self, source: KeyframeSource) {
        self.sfu.keyframe_requests_ignored[source as usize].fetch_add(1, Ordering::Relaxed);
    }

    /// 방송자 비디오 프레임을 꺼낼 때까지 붙잡은 시간.
    pub(crate) fn sfu_frame_hold(&self, hold: Duration) {
        self.sfu.frame_hold.observe(hold.as_secs_f64());
    }

    /// 방송자 세션 한 번의 `drive`에서 나온 비디오 프레임 수(1 이상일 때만 넣는다).
    pub(crate) fn sfu_frames_per_drive(&self, frames: u32) {
        self.sfu.frames_per_drive.observe(frames as f64);
    }

    /// WebRTC 연결이 된 세션 수(SFU가 갱신한다).
    pub(crate) fn set_webrtc_sessions(&self, publishers: usize, viewers: usize) {
        self.webrtc_publishers
            .store(publishers as u64, Ordering::Relaxed);
        self.webrtc_viewers.store(viewers as u64, Ordering::Relaxed);
    }

    /// Prometheus 텍스트 형식(0.0.4)으로 쓴다.
    pub fn render(&self) -> String {
        let stats = self.stats.read().unwrap_or_else(|e| e.into_inner()).clone();
        let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let mut o = Out::default();

        o.head(
            "dongsi_info",
            "gauge",
            "서버 정보. 값은 늘 1이고 버전·프로토콜 버전·서버 이름은 라벨에 있다",
        );
        o.sample(
            "dongsi_info",
            &[
                ("version", env!("CARGO_PKG_VERSION")),
                ("protocol", &PROTOCOL_VERSION.to_string()),
                ("name", &self.name),
            ],
            1,
        );
        o.single(
            "dongsi_start_time_seconds",
            "gauge",
            "서버가 시작한 시각(유닉스 시간, 초)",
            self.started_at,
        );

        // 지금 상태
        o.single(
            "dongsi_connections",
            "gauge",
            "지금 접속한 클라이언트 수(시그널링 연결. 앱은 저장한 서버마다 하나씩 연결한다)",
            stats.connections as u64,
        );
        o.single(
            "dongsi_broadcasts",
            "gauge",
            "진행 중인 방송 수",
            stats.broadcasts as u64,
        );
        o.single(
            "dongsi_viewers",
            "gauge",
            "시청 세션 수(모든 방송의 합)",
            stats.viewers as u64,
        );
        o.head(
            "dongsi_webrtc_sessions",
            "gauge",
            "WebRTC(ICE·DTLS) 연결이 된 미디어 세션 수. 시청 세션 수보다 작으면 UDP가 막혔을 수 있다",
        );
        o.sample(
            "dongsi_webrtc_sessions",
            &[("role", "publisher")],
            load(&self.webrtc_publishers),
        );
        o.sample(
            "dongsi_webrtc_sessions",
            &[("role", "viewer")],
            load(&self.webrtc_viewers),
        );

        // 누적: 시그널링
        o.single(
            "dongsi_connections_total",
            "counter",
            "받은 접속 수(Welcome까지 간 것)",
            load(&self.connections),
        );
        o.head(
            "dongsi_connections_failed_total",
            "counter",
            "Welcome 전에 끝난 접속 수. tls에는 포트 검사·--healthcheck·지문이 달라 끊은 클라이언트도 든다",
        );
        for r in ConnFailure::ALL {
            o.sample(
                "dongsi_connections_failed_total",
                &[("reason", r.label())],
                load(&self.connection_failures[r as usize]),
            );
        }
        o.single(
            "dongsi_broadcasts_started_total",
            "counter",
            "시작한 방송 수",
            load(&self.broadcasts),
        );
        o.single(
            "dongsi_watch_sessions_started_total",
            "counter",
            "시작한 시청 세션 수",
            load(&self.watches),
        );
        o.head(
            "dongsi_request_errors_total",
            "counter",
            "클라이언트에 보낸 오류 응답 수(오류 코드별)",
        );
        for (i, code) in ERROR_CODES.iter().enumerate() {
            o.sample(
                "dongsi_request_errors_total",
                &[("code", error_label(*code))],
                load(&self.request_errors[i]),
            );
        }
        o.single(
            "dongsi_signaling_received_bytes_total",
            "counter",
            "시그널링(WebSocket)으로 받은 메시지 바이트(TLS·WebSocket 헤더 제외)",
            load(&self.signaling_rx),
        );
        o.single(
            "dongsi_signaling_sent_bytes_total",
            "counter",
            "시그널링(WebSocket)으로 보낸 메시지 바이트(TLS·WebSocket 헤더 제외)",
            load(&self.signaling_tx),
        );

        // 누적: UDP(미디어, 로컬 감지)
        let sent_kinds = [Peer::Publisher, Peer::Viewer, Peer::Discovery];
        for (name, help, kinds, table, bytes) in [
            (
                "dongsi_udp_received_bytes_total",
                "UDP로 받은 바이트(페이로드, 상대별). publisher는 방송자가 올린 미디어다",
                &Peer::ALL[..],
                &self.udp_rx,
                true,
            ),
            (
                "dongsi_udp_received_packets_total",
                "UDP로 받은 패킷 수(상대별)",
                &Peer::ALL[..],
                &self.udp_rx,
                false,
            ),
            (
                "dongsi_udp_sent_bytes_total",
                "UDP로 보낸 바이트(페이로드, 상대별). viewer는 시청자에게 전달한 미디어다",
                &sent_kinds[..],
                &self.udp_tx,
                true,
            ),
            (
                "dongsi_udp_sent_packets_total",
                "UDP로 보낸 패킷 수(상대별)",
                &sent_kinds[..],
                &self.udp_tx,
                false,
            ),
        ] {
            o.head(name, "counter", help);
            for peer in kinds {
                let t = &table[*peer as usize];
                let v = if bytes { &t.bytes } else { &t.packets };
                o.sample(name, &[("kind", peer.label())], load(v));
            }
        }
        o.single(
            "dongsi_udp_dropped_packets_total",
            "counter",
            "처리가 밀려 버린 받은 UDP 패킷 수(dongsi_udp_received_*에는 들지 않는다)",
            load(&self.udp_dropped),
        );
        o.single(
            "dongsi_udp_send_errors_total",
            "counter",
            "UDP 보내기 실패 수",
            load(&self.udp_send_errors),
        );

        // 누적: SFU 진단(D-087)
        let sfu = &self.sfu;
        o.single(
            "dongsi_sfu_broken_video_frames_total",
            "counter",
            "방송자에게서 contiguous=false로 도착한 비디오 프레임 수(앞 프레임을 잃은 끊김, 방송자→서버 구간 손실)",
            load(&sfu.broken_frames),
        );
        o.single(
            "dongsi_sfu_dropped_video_frames_total",
            "counter",
            "끊긴 뒤 키프레임이 올 때까지 시청자에게 보내지 않고 버린 비디오 프레임 수",
            load(&sfu.dropped_frames),
        );
        o.single(
            "dongsi_sfu_pli_sent_total",
            "counter",
            "방송자에게 실제로 보낸 PLI(키프레임 요청) 수(재시도 포함)",
            load(&sfu.pli_sent),
        );
        o.single(
            "dongsi_sfu_keyframes_received_total",
            "counter",
            "방송자에게서 받은 비디오 키프레임 수",
            load(&sfu.keyframes_received),
        );
        o.single(
            "dongsi_sfu_publisher_nacks_sent_total",
            "counter",
            "방송자에게 보낸 NACK(재전송 요청) 패킷 수(방송자→서버 손실을 되살리려는 요청)",
            load(&sfu.publisher_nacks_sent),
        );
        o.single(
            "dongsi_sfu_viewer_nacks_received_total",
            "counter",
            "시청자에게서 받은 NACK(재전송 요청) 수(서버→시청자 손실). 세션이 끝나도 합계는 줄지 않는다",
            load(&sfu.viewer_nacks_received),
        );
        o.single(
            "dongsi_sfu_viewer_pli_fir_received_total",
            "counter",
            "시청자에게서 받은 PLI·FIR(키프레임 요청) 수(2초 제한 전의 RTCP 수신 수)",
            load(&sfu.viewer_pli_fir_received),
        );
        o.single(
            "dongsi_sfu_rtx_packets_sent_total",
            "counter",
            "시청자 비디오의 RTX SSRC로 보낸 패킷 수(재전송)",
            load(&sfu.rtx_packets),
        );
        o.head(
            "dongsi_sfu_keyframe_requests_ignored_total",
            "counter",
            "시청자 한 명당 2초 제한으로 방송자 요청에 잇지 않고 무시한 키프레임 요청 수(rtcp: PLI/FIR, signaling: 앱의 멈춤 감지)",
        );
        for source in KeyframeSource::ALL {
            o.sample(
                "dongsi_sfu_keyframe_requests_ignored_total",
                &[("source", source.label())],
                load(&sfu.keyframe_requests_ignored[source as usize]),
            );
        }
        o.histogram(
            "dongsi_sfu_frame_hold_seconds",
            "방송자 비디오 프레임을 시청자 전달 쪽에서 꺼낼 때까지 붙잡은 시간(초). 프레임의 첫 패킷이 도착한 때부터 SFU가 프레임을 꺼낸 때까지라 프레임 조립·재전송 대기·SFU 스레드 지연을 포함한다",
            &sfu.frame_hold,
        );
        o.histogram(
            "dongsi_sfu_frames_per_drive",
            "방송자 세션 한 번의 처리(poll_output을 Timeout까지 비움)에서 나온 비디오 프레임 수(1 이상인 경우만). 크면 SFU가 프레임을 몰아 보낸 것이다",
            &sfu.frames_per_drive,
        );
        o.0
    }
}

/// 텍스트 형식으로 쓰는 도우미.
#[derive(Default)]
struct Out(String);

impl Out {
    fn head(&mut self, name: &str, kind: &str, help: &str) {
        let _ = writeln!(self.0, "# HELP {name} {}", escape(help, false));
        let _ = writeln!(self.0, "# TYPE {name} {kind}");
    }

    fn sample(&mut self, name: &str, labels: &[(&str, &str)], value: u64) {
        self.0.push_str(name);
        if !labels.is_empty() {
            self.0.push('{');
            for (i, (k, v)) in labels.iter().enumerate() {
                if i > 0 {
                    self.0.push(',');
                }
                let _ = write!(self.0, "{k}=\"{}\"", escape(v, true));
            }
            self.0.push('}');
        }
        let _ = writeln!(self.0, " {value}");
    }

    /// histogram 하나: 누적 `_bucket{le=...}`(마지막은 `+Inf`), `_sum`(실수), `_count`.
    fn histogram(&mut self, name: &str, help: &str, h: &Histogram) {
        self.head(name, "histogram", help);
        let mut cumulative = 0u64;
        for (i, count) in h.buckets.iter().enumerate() {
            cumulative += count.load(Ordering::Relaxed);
            let le = match h.bounds.get(i) {
                Some(b) => b.to_string(),
                None => "+Inf".to_string(),
            };
            let _ = writeln!(self.0, "{name}_bucket{{le=\"{le}\"}} {cumulative}");
        }
        let sum = h.sum_scaled.load(Ordering::Relaxed) as f64 / h.scale;
        let _ = writeln!(self.0, "{name}_sum {sum}");
        let _ = writeln!(self.0, "{name}_count {cumulative}");
    }

    /// 라벨 없는 지표 하나.
    fn single(&mut self, name: &str, kind: &str, help: &str, value: u64) {
        self.head(name, kind, help);
        self.sample(name, &[], value);
    }
}

/// HELP 문구(`\\`, 줄바꿈)와 라벨 값(여기에 `"`까지)을 이스케이프한다.
fn escape(s: &str, label: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '"' if label => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out
}

/// 시험용: 텍스트에서 `series`(이름과 라벨, 예: `dongsi_viewers`, `x{kind="viewer"}`)의 값을 찾는다.
#[cfg(test)]
pub(crate) fn sample_value(text: &str, series: &str) -> Option<u64> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .find_map(|l| l.strip_prefix(series)?.strip_prefix(' ')?.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> Metrics {
        Metrics::new("시험 \"서버\"".into(), Arc::default())
    }

    #[test]
    fn renders_state_and_counters() {
        let stats = Arc::new(RwLock::new(ServerStats {
            connections: 3,
            broadcasts: 1,
            viewers: 2,
        }));
        let m = Metrics::new("시험".into(), stats);
        m.connection_accepted();
        m.connection_accepted();
        m.connection_failed(ConnFailure::Protocol);
        m.request_error(ErrorCode::WrongPassword);
        m.request_error(ErrorCode::WrongPassword);
        m.udp_received(Peer::Publisher, 1200);
        m.udp_received(Peer::Publisher, 300);
        m.udp_sent(Peer::Viewer, 1000);
        m.udp_dropped();
        m.set_webrtc_sessions(1, 2);
        let text = m.render();
        let v = |series| sample_value(&text, series);
        assert_eq!(v("dongsi_connections"), Some(3));
        assert_eq!(v("dongsi_broadcasts"), Some(1));
        assert_eq!(v("dongsi_viewers"), Some(2));
        assert_eq!(v("dongsi_webrtc_sessions{role=\"viewer\"}"), Some(2));
        assert_eq!(v("dongsi_connections_total"), Some(2));
        assert_eq!(
            v("dongsi_connections_failed_total{reason=\"protocol\"}"),
            Some(1)
        );
        assert_eq!(
            v("dongsi_connections_failed_total{reason=\"tls\"}"),
            Some(0)
        );
        assert_eq!(
            v("dongsi_request_errors_total{code=\"wrong_password\"}"),
            Some(2)
        );
        assert_eq!(
            v("dongsi_udp_received_bytes_total{kind=\"publisher\"}"),
            Some(1500)
        );
        assert_eq!(
            v("dongsi_udp_received_packets_total{kind=\"publisher\"}"),
            Some(2)
        );
        assert_eq!(
            v("dongsi_udp_sent_bytes_total{kind=\"viewer\"}"),
            Some(1000)
        );
        assert_eq!(v("dongsi_udp_dropped_packets_total"), Some(1));
        // 받기만 하는 상대는 보낸 쪽에 없다.
        assert_eq!(v("dongsi_udp_sent_bytes_total{kind=\"unknown\"}"), None);
    }

    #[test]
    fn every_family_has_help_and_type_once() {
        let text = metrics().render();
        let mut families = Vec::new();
        let mut histograms = Vec::new();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("# TYPE ") {
                let (name, kind) = rest.split_once(' ').unwrap();
                assert!(matches!(kind, "gauge" | "counter" | "histogram"), "{line}");
                assert!(!families.contains(&name), "TYPE가 두 번: {name}");
                assert!(
                    text.contains(&format!("# HELP {name} ")),
                    "HELP가 없습니다: {name}"
                );
                if kind == "counter" {
                    assert!(name.ends_with("_total"), "{name}");
                }
                if kind == "histogram" {
                    // histogram 이름 자체는 _total·_bucket·_sum·_count로 끝나지 않는다(시리즈 이름과 겹친다).
                    assert!(
                        !["_total", "_bucket", "_sum", "_count"]
                            .iter()
                            .any(|s| name.ends_with(s)),
                        "{name}"
                    );
                    histograms.push(name);
                }
                families.push(name);
            } else if !line.starts_with('#') {
                let name = line.split(['{', ' ']).next().unwrap();
                // histogram 시리즈는 `<이름>_bucket|_sum|_count`다.
                let family = ["_bucket", "_sum", "_count"]
                    .iter()
                    .find_map(|s| name.strip_suffix(s).filter(|f| histograms.contains(f)))
                    .unwrap_or(name);
                assert!(families.contains(&family), "TYPE 전에 나온 값: {line}");
                assert!(name.starts_with("dongsi_"), "{line}");
            }
        }
        assert!(text.ends_with('\n'));
        assert!(histograms.len() >= 2, "SFU histogram이 있어야 합니다");

        // histogram 형식: 버킷은 누적이고 +Inf로 끝나며 _count는 +Inf 버킷과 같다.
        for h in &histograms {
            let prefix = format!("{h}_bucket{{le=\"");
            let counts: Vec<(String, u64)> = text
                .lines()
                .filter_map(|l| {
                    let rest = l.strip_prefix(&prefix)?;
                    let (le, value) = rest.split_once("\"} ")?;
                    Some((le.to_string(), value.parse().ok()?))
                })
                .collect();
            assert!(counts.len() >= 2, "{h}");
            assert!(counts.windows(2).all(|w| w[0].1 <= w[1].1), "{h} 누적");
            let (last_le, last) = counts.last().unwrap();
            assert_eq!(last_le, "+Inf", "{h}");
            assert_eq!(sample_value(&text, &format!("{h}_count")), Some(*last));
            assert!(
                text.lines().any(|l| l
                    .strip_prefix(&format!("{h}_sum "))
                    .is_some_and(|v| v.parse::<f64>().is_ok())),
                "{h}_sum은 실수여야 합니다"
            );
        }
    }

    #[test]
    fn renders_sfu_counters() {
        let m = metrics();
        m.sfu_broken_frame();
        m.sfu_broken_frame();
        m.sfu_dropped_frame();
        m.sfu_pli_sent();
        m.sfu_keyframe_received();
        m.sfu_publisher_nacks_sent(5);
        m.sfu_viewer_feedback(7, 2);
        m.sfu_viewer_feedback(1, 0);
        m.sfu_rtx_packet();
        m.sfu_rtx_packet();
        m.sfu_rtx_packet();
        m.sfu_keyframe_request_ignored(KeyframeSource::Rtcp);
        m.sfu_keyframe_request_ignored(KeyframeSource::Signaling);
        m.sfu_keyframe_request_ignored(KeyframeSource::Signaling);
        let text = m.render();
        let v = |series| sample_value(&text, series);
        assert_eq!(v("dongsi_sfu_broken_video_frames_total"), Some(2));
        assert_eq!(v("dongsi_sfu_dropped_video_frames_total"), Some(1));
        assert_eq!(v("dongsi_sfu_pli_sent_total"), Some(1));
        assert_eq!(v("dongsi_sfu_keyframes_received_total"), Some(1));
        assert_eq!(v("dongsi_sfu_publisher_nacks_sent_total"), Some(5));
        assert_eq!(v("dongsi_sfu_viewer_nacks_received_total"), Some(8));
        assert_eq!(v("dongsi_sfu_viewer_pli_fir_received_total"), Some(2));
        assert_eq!(v("dongsi_sfu_rtx_packets_sent_total"), Some(3));
        assert_eq!(
            v("dongsi_sfu_keyframe_requests_ignored_total{source=\"rtcp\"}"),
            Some(1)
        );
        assert_eq!(
            v("dongsi_sfu_keyframe_requests_ignored_total{source=\"signaling\"}"),
            Some(2)
        );
    }

    #[test]
    fn histogram_buckets_are_cumulative() {
        let m = metrics();
        // 3ms(첫 버킷), 5ms(경계 값은 그 버킷), 30ms, 3초(+Inf 버킷만)
        for ms in [3u64, 5, 30] {
            m.sfu_frame_hold(Duration::from_millis(ms));
        }
        m.sfu_frame_hold(Duration::from_secs(3));
        for n in [1, 1, 3, 40] {
            m.sfu_frames_per_drive(n);
        }
        let text = m.render();
        let v = |series: &str| sample_value(&text, series);
        let hold = "dongsi_sfu_frame_hold_seconds";
        assert_eq!(v(&format!("{hold}_bucket{{le=\"0.005\"}}")), Some(2));
        assert_eq!(v(&format!("{hold}_bucket{{le=\"0.01\"}}")), Some(2));
        assert_eq!(v(&format!("{hold}_bucket{{le=\"0.05\"}}")), Some(3));
        assert_eq!(v(&format!("{hold}_bucket{{le=\"2.5\"}}")), Some(3));
        assert_eq!(v(&format!("{hold}_bucket{{le=\"+Inf\"}}")), Some(4));
        assert_eq!(v(&format!("{hold}_count")), Some(4));
        let sum: f64 = text
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{hold}_sum "))?.parse().ok())
            .unwrap();
        assert!((sum - 3.038).abs() < 1e-9, "{sum}");

        let burst = "dongsi_sfu_frames_per_drive";
        assert_eq!(v(&format!("{burst}_bucket{{le=\"1\"}}")), Some(2));
        assert_eq!(v(&format!("{burst}_bucket{{le=\"4\"}}")), Some(3));
        assert_eq!(v(&format!("{burst}_bucket{{le=\"32\"}}")), Some(3));
        assert_eq!(v(&format!("{burst}_bucket{{le=\"+Inf\"}}")), Some(4));
        assert_eq!(v(&format!("{burst}_sum")), Some(45));
        assert_eq!(v(&format!("{burst}_count")), Some(4));
    }

    #[test]
    fn label_values_are_escaped() {
        let text = metrics().render();
        assert!(
            text.contains(r#"name="시험 \"서버\"""#),
            "{}",
            text.lines().find(|l| l.starts_with("dongsi_info")).unwrap()
        );
        assert_eq!(escape("a\\b\nc\"d", true), r#"a\\b\nc\"d"#);
        assert_eq!(escape("a\"b\n", false), r#"a"b\n"#);
    }

    #[test]
    fn label_tables_follow_declaration_order() {
        // 값은 `as usize` 자리에 둔다.
        for (i, p) in Peer::ALL.iter().enumerate() {
            assert_eq!(*p as usize, i);
        }
        for (i, r) in ConnFailure::ALL.iter().enumerate() {
            assert_eq!(*r as usize, i);
        }
        for (i, s) in KeyframeSource::ALL.iter().enumerate() {
            assert_eq!(*s as usize, i);
        }
    }

    #[test]
    fn error_codes_have_distinct_labels() {
        let mut labels: Vec<_> = ERROR_CODES.iter().map(|c| error_label(*c)).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), ERROR_CODES.len());
    }
}
