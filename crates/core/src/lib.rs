//! dongsi 공통 타입.
//!
//! - [`addr`]: 서버 주소 (`host[:port]`, 기본 포트 7884)
//! - [`discovery`]: 로컬 방송 감지 패킷 (UDP 7884)
//! - [`fingerprint`]: 서버 인증서 SHA-256 지문
//! - [`id`]: 설치 ID(방송자 구분)와 저장된 서버의 로컬 ID
//! - [`invite`]: 초대 코드 (`dongsi1:` + base64url(JSON))
//! - [`media`]: 비디오 코덱, 화질 설정, 화질 프리셋, simulcast 계층(D-098)
//! - [`priority`]: 캡처·인코딩·송출 스레드와 프로세스 우선순위(Windows, D-081)
//! - [`proto`]: 시그널링 메시지 (TLS 위 WebSocket, JSON)
//! - [`qp_scale`]: QP 기반 자동 해상도 판정 상태 기계(D-099)
//! - [`servers`]: 클라이언트가 저장하는 서버 목록

pub mod addr;
mod b64;
pub mod discovery;
pub mod fingerprint;
pub mod id;
pub mod invite;
pub mod media;
pub mod priority;
pub mod proto;
pub mod qp_scale;
pub mod servers;

pub use addr::{AddrError, DEFAULT_PORT, Host, ServerAddr};
pub use fingerprint::{Fingerprint, FingerprintError};
pub use id::{IdError, InstallId, ServerId};
pub use invite::{Invite, InviteError};
pub use media::{Layer, LayerRates, UnknownCodec, VideoCodec, VideoSettings};
pub use servers::{AddOutcome, SavedServer, ServerList, ServerListError};
