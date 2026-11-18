//! dongsi 네트워크 계층.
//!
//! - [`tls`]: 서버 TLS 신원(자체 서명), 지문으로 확인하는 클라이언트 설정
//! - [`signal`]: 시그널링 클라이언트(TLS 위 WebSocket)
//! - [`publish`]: WebRTC 송출 클라이언트(str0m)
//! - [`discovery`]: 로컬 방송 감지(LAN 브로드캐스트, Tailscale 피어)
//! - [`upnp`]: UPnP 포트 매핑
//! - [`ifaces`]: 네트워크 인터페이스 목록

pub mod discovery;
pub mod ifaces;
pub mod publish;
pub mod signal;
pub mod tls;
pub mod upnp;

pub use publish::tune_socket as tune_udp_socket;

use dongsi_core::Fingerprint;

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("입출력 오류: {0}")]
    Io(#[from] std::io::Error),
    #[error("TLS 오류: {0}")]
    Tls(String),
    #[error(
        "서버 인증서 지문이 다릅니다. 저장된 지문 {}…, 실제 지문 {}…. 서버가 바뀌었거나 다른 서버일 수 있습니다",
        expected.short(),
        actual.short()
    )]
    FingerprintMismatch {
        expected: Fingerprint,
        actual: Fingerprint,
    },
    #[error("{0}")]
    Connect(String),
    #[error("{0}")]
    Timeout(String),
    #[error("연결이 닫혔습니다")]
    Closed,
    #[error("WebRTC 오류: {0}")]
    Rtc(String),
    #[error("{0}")]
    Upnp(String),
}
