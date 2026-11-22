//! Prometheus 지표(D-083).
//!
//! 서버 코어(시그널링·SFU)가 세고, 전용 서버가 HTTP `GET /metrics`로 내보낸다(`apps/server`의 `metrics.rs`).
//! 내장 서버도 같이 세지만 내보내지는 않는다.
//!
//! - 값은 모두 서버 전체의 합이다. 방송자·시청자 이름이나 방송마다 새로 생기는 라벨은 넣지 않는다.
//! - UDP 바이트는 UDP 페이로드(IP·UDP 헤더 제외), 시그널링 바이트는 WebSocket 메시지 내용(TLS·WebSocket
//!   헤더 제외)이다. 회선에 실제로 흐르는 양은 이보다 조금 많다.
//! - 텍스트는 Prometheus exposition format 0.0.4다.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

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
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("# TYPE ") {
                let (name, kind) = rest.split_once(' ').unwrap();
                assert!(matches!(kind, "gauge" | "counter"), "{line}");
                assert!(!families.contains(&name), "TYPE가 두 번: {name}");
                assert!(
                    text.contains(&format!("# HELP {name} ")),
                    "HELP가 없습니다: {name}"
                );
                if kind == "counter" {
                    assert!(name.ends_with("_total"), "{name}");
                }
                families.push(name);
            } else if !line.starts_with('#') {
                let name = line.split(['{', ' ']).next().unwrap();
                assert!(families.contains(&name), "TYPE 전에 나온 값: {line}");
                assert!(name.starts_with("dongsi_"), "{line}");
            }
        }
        assert!(text.ends_with('\n'));
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
    }

    #[test]
    fn error_codes_have_distinct_labels() {
        let mut labels: Vec<_> = ERROR_CODES.iter().map(|c| error_label(*c)).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), ERROR_CODES.len());
    }
}
