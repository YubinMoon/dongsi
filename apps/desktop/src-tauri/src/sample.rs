//! 개발용 예시 값. 서버 추가 대화 상자의 "예시 초대 코드 넣기"(개발 빌드 전용)에서 쓴다.

use dongsi_core::{Fingerprint, InstallId, Invite, ServerAddr};

/// 개발용 예시 방송 초대 코드. 지문이 고정이라 두 번 넣으면 "갱신"으로 처리된다.
/// 실제로 접속되는 서버는 없다.
pub fn invite_code() -> String {
    let addrs: Vec<ServerAddr> = ["192.168.0.23", "100.101.102.103", "203.0.113.7"]
        .iter()
        .map(|a| a.parse().expect("고정된 예시 주소"))
        .collect();
    Invite::for_broadcast(
        addrs,
        Fingerprint::from_bytes([0x5A; Fingerprint::LEN]),
        Some("예시 서버 (개발용)".into()),
        InstallId::from_bytes([0xA5; 16]),
        Some("1234".into()),
    )
    .expect("고정된 예시 값")
    .encode()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_invite_decodes() {
        let invite = Invite::decode(&invite_code()).unwrap();
        assert_eq!(invite.addrs().len(), 3);
        assert_eq!(invite.password(), Some("1234"));
    }
}
