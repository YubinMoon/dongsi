//! 초대 코드 (D-029): `dongsi1:` + base64url(JSON, 패딩 없음).
//!
//! | 키 | 내용 | 필수 여부 |
//! |---|---|---|
//! | `a` | 주소 후보 목록 (기본 포트 7884는 생략) | 필수 |
//! | `f` | 서버 인증서 SHA-256 지문 | 필수 |
//! | `b` | 방송자 설치 ID | 방송 초대일 때 |
//! | `p` | 방송 비밀번호 (`b`가 있을 때만) | 선택 |
//! | `n` | 서버 목록에 쓸 기본 이름 | 선택 |
//!
//! 붙여 넣을 때 섞여 들어오는 공백, 줄바꿈, Discord 코드 블록 표시(`` ` ``)는 무시한다.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{Fingerprint, InstallId, ServerAddr, b64};

pub const PREFIX: &str = "dongsi1:";
const MAGIC: &str = "dongsi";
const VERSION: u32 = 1;

pub const MAX_ADDRS: usize = 8;
pub const MAX_NAME_CHARS: usize = 64;
pub const MAX_PASSWORD_CHARS: usize = 128;
/// 정상적인 코드는 수백 자를 넘지 않는다.
const MAX_CODE_LEN: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InviteError {
    #[error("초대 코드가 아닙니다")]
    NotInvite,
    #[error("지원하지 않는 초대 코드 버전입니다(v{0}). 앱을 업데이트해 주세요")]
    UnsupportedVersion(u32),
    #[error("초대 코드가 손상되었습니다. 코드 전체를 다시 복사해 주세요")]
    Malformed,
    #[error("초대 코드가 너무 깁니다")]
    TooLong,
    #[error("초대 코드에 접속 주소가 없습니다")]
    NoAddress,
    #[error("접속 주소가 너무 많습니다(최대 {}개)", MAX_ADDRS)]
    TooManyAddrs,
    #[error("이름이 너무 깁니다(최대 {}자)", MAX_NAME_CHARS)]
    NameTooLong,
    #[error("비밀번호가 너무 깁니다(최대 {}자)", MAX_PASSWORD_CHARS)]
    PasswordTooLong,
    #[error("비밀번호는 방송 초대 코드에만 넣을 수 있습니다")]
    PasswordWithoutBroadcaster,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    addrs: Vec<ServerAddr>,
    fingerprint: Fingerprint,
    broadcaster: Option<InstallId>,
    password: Option<String>,
    name: Option<String>,
}

#[derive(Serialize)]
struct WireOut<'a> {
    a: &'a [ServerAddr],
    f: &'a Fingerprint,
    #[serde(skip_serializing_if = "Option::is_none")]
    b: Option<&'a InstallId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    p: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<&'a str>,
}

/// 알 수 없는 키는 무시한다(같은 버전 안에서 필드를 추가할 수 있도록).
#[derive(Deserialize)]
struct WireIn {
    a: Vec<ServerAddr>,
    f: Fingerprint,
    #[serde(default)]
    b: Option<InstallId>,
    #[serde(default)]
    p: Option<String>,
    #[serde(default)]
    n: Option<String>,
}

impl Invite {
    /// 서버 초대 코드. 전용 서버가 시작할 때 출력한다.
    pub fn for_server(
        addrs: Vec<ServerAddr>,
        fingerprint: Fingerprint,
        name: Option<String>,
    ) -> Result<Self, InviteError> {
        Self::validated(addrs, fingerprint, None, None, name)
    }

    /// 방송 초대 코드. 방송을 시작하면 방송자 앱이 보여 준다.
    pub fn for_broadcast(
        addrs: Vec<ServerAddr>,
        fingerprint: Fingerprint,
        name: Option<String>,
        broadcaster: InstallId,
        password: Option<String>,
    ) -> Result<Self, InviteError> {
        Self::validated(addrs, fingerprint, Some(broadcaster), password, name)
    }

    /// 비밀번호를 뺀 코드("비밀번호 없이 복사" 버튼용).
    pub fn without_password(&self) -> Self {
        Self {
            password: None,
            ..self.clone()
        }
    }

    pub fn addrs(&self) -> &[ServerAddr] {
        &self.addrs
    }

    pub fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }

    pub fn broadcaster(&self) -> Option<&InstallId> {
        self.broadcaster.as_ref()
    }

    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn encode(&self) -> String {
        let wire = WireOut {
            a: &self.addrs,
            f: &self.fingerprint,
            b: self.broadcaster.as_ref(),
            p: self.password.as_deref(),
            n: self.name.as_deref(),
        };
        let json =
            serde_json::to_vec(&wire).expect("문자열과 배열만 있어 직렬화는 실패하지 않는다");
        format!("{PREFIX}{}", b64::encode(json))
    }

    pub fn decode(input: &str) -> Result<Self, InviteError> {
        let payload = extract_payload(input)?;
        let json = b64::decode(&payload).ok_or(InviteError::Malformed)?;
        let wire: WireIn = serde_json::from_slice(&json).map_err(|_| InviteError::Malformed)?;
        Self::validated(wire.a, wire.f, wire.b, wire.p, wire.n)
    }

    /// 입력이 초대 코드처럼 보이는지(`dongsi<숫자>:`로 시작하는지).
    /// 손상되었거나 버전이 다른 코드도 `true`다. 서버 추가 입력란에서 주소와 구분하는 데 쓴다.
    pub fn looks_like(input: &str) -> bool {
        !matches!(extract_payload(input), Err(InviteError::NotInvite))
    }

    fn validated(
        addrs: Vec<ServerAddr>,
        fingerprint: Fingerprint,
        broadcaster: Option<InstallId>,
        password: Option<String>,
        name: Option<String>,
    ) -> Result<Self, InviteError> {
        let mut unique: Vec<ServerAddr> = Vec::with_capacity(addrs.len());
        for addr in addrs {
            if !unique.contains(&addr) {
                unique.push(addr);
            }
        }
        if unique.is_empty() {
            return Err(InviteError::NoAddress);
        }
        if unique.len() > MAX_ADDRS {
            return Err(InviteError::TooManyAddrs);
        }

        let name = name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty());
        if name
            .as_ref()
            .is_some_and(|n| n.chars().count() > MAX_NAME_CHARS)
        {
            return Err(InviteError::NameTooLong);
        }

        // 비밀번호는 앞뒤 공백도 의미가 있을 수 있으므로 다듬지 않는다.
        let password = password.filter(|p| !p.is_empty());
        if password
            .as_ref()
            .is_some_and(|p| p.chars().count() > MAX_PASSWORD_CHARS)
        {
            return Err(InviteError::PasswordTooLong);
        }
        if password.is_some() && broadcaster.is_none() {
            return Err(InviteError::PasswordWithoutBroadcaster);
        }

        Ok(Self {
            addrs: unique,
            fingerprint,
            broadcaster,
            password,
            name,
        })
    }
}

/// 공백·백틱을 걷어 내고 `dongsi<버전>:` 뒤의 내용을 돌려준다.
fn extract_payload(input: &str) -> Result<String, InviteError> {
    let compact: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '`')
        .collect();
    // 코드 블록의 언어 표시(예: ```text)가 앞에 붙어 있을 수 있어 처음 나오는 위치를 찾는다.
    let start = compact.find(MAGIC).ok_or(InviteError::NotInvite)?;
    let rest = &compact[start + MAGIC.len()..];
    let digits = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if digits == 0 || !rest[digits..].starts_with(':') {
        return Err(InviteError::NotInvite);
    }
    if rest.len() > MAX_CODE_LEN {
        return Err(InviteError::TooLong);
    }
    let version: u32 = rest[..digits].parse().map_err(|_| InviteError::Malformed)?;
    if version != VERSION {
        return Err(InviteError::UnsupportedVersion(version));
    }
    Ok(rest[digits + 1..].to_owned())
}

impl fmt::Display for Invite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode())
    }
}

impl FromStr for Invite {
    type Err = InviteError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::decode(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp() -> Fingerprint {
        Fingerprint::from_bytes([7u8; Fingerprint::LEN])
    }

    fn addrs(list: &[&str]) -> Vec<ServerAddr> {
        list.iter().map(|a| a.parse().unwrap()).collect()
    }

    fn broadcast_invite() -> Invite {
        Invite::for_broadcast(
            addrs(&["192.168.0.23", "100.101.102.103", "203.0.113.7:7885"]),
            fp(),
            Some("거실 PC".into()),
            InstallId::from_bytes([9u8; 16]),
            Some("비밀 1234".into()),
        )
        .unwrap()
    }

    fn payload_json(code: &str) -> String {
        let payload = code.strip_prefix(PREFIX).unwrap();
        String::from_utf8(b64::decode(payload).unwrap()).unwrap()
    }

    #[test]
    fn server_invite_roundtrip() {
        let invite = Invite::for_server(addrs(&["example.com"]), fp(), None).unwrap();
        let code = invite.encode();
        assert!(code.starts_with(PREFIX));
        assert_eq!(Invite::decode(&code), Ok(invite));
    }

    #[test]
    fn broadcast_invite_roundtrip_and_stays_short() {
        let invite = broadcast_invite();
        let code = invite.encode();
        assert_eq!(Invite::decode(&code), Ok(invite));
        assert!(code.len() < 300, "코드가 너무 길다: {} 자", code.len());
    }

    #[test]
    fn default_port_and_empty_fields_are_omitted() {
        let invite = Invite::for_server(addrs(&["10.0.0.1:7884"]), fp(), None).unwrap();
        let json = payload_json(&invite.encode());
        assert!(json.contains("\"10.0.0.1\""), "{json}");
        assert!(!json.contains("\"b\"") && !json.contains("\"p\"") && !json.contains("\"n\""));
    }

    #[test]
    fn without_password_keeps_broadcaster() {
        let stripped = broadcast_invite().without_password();
        let decoded = Invite::decode(&stripped.encode()).unwrap();
        assert_eq!(decoded.password(), None);
        assert_eq!(
            decoded.broadcaster(),
            Some(&InstallId::from_bytes([9u8; 16]))
        );
    }

    #[test]
    fn decode_tolerates_chat_formatting() {
        let code = broadcast_invite().encode();
        let (head, tail) = code.split_at(20);
        let pasted = [
            format!("  {code}\n"),
            format!("`{code}`"),
            format!("```\n{code}\n```"),
            format!("```text\n{code}\n```"),
            format!("{head}\n{tail}"),
        ];
        for text in pasted {
            assert_eq!(
                Invite::decode(&text),
                Ok(broadcast_invite()),
                "입력: {text:?}"
            );
        }
    }

    #[test]
    fn detects_non_invites_and_versions() {
        assert_eq!(Invite::decode("192.168.0.10"), Err(InviteError::NotInvite));
        assert_eq!(Invite::decode("dongsi:abc"), Err(InviteError::NotInvite));
        assert_eq!(
            Invite::decode("dongsi2:abc"),
            Err(InviteError::UnsupportedVersion(2))
        );
        assert!(!Invite::looks_like("example.com:7884"));
        assert!(Invite::looks_like("dongsi1:손상됨"));
        assert!(Invite::looks_like("dongsi9:"));
    }

    #[test]
    fn rejects_malformed_payloads() {
        assert_eq!(Invite::decode("dongsi1:!!!"), Err(InviteError::Malformed));
        let not_json = format!("{PREFIX}{}", b64::encode("hello"));
        assert_eq!(Invite::decode(&not_json), Err(InviteError::Malformed));
        let bad_addr = format!(
            "{PREFIX}{}",
            b64::encode(format!(
                r#"{{"a":["bad host"],"f":"{}"}}"#,
                fp().to_base64url()
            ))
        );
        assert_eq!(Invite::decode(&bad_addr), Err(InviteError::Malformed));
        let long = format!("{PREFIX}{}", "A".repeat(MAX_CODE_LEN + 1));
        assert_eq!(Invite::decode(&long), Err(InviteError::TooLong));
    }

    #[test]
    fn validates_fields() {
        assert_eq!(
            Invite::for_server(vec![], fp(), None),
            Err(InviteError::NoAddress)
        );
        let many: Vec<String> = (1..=9).map(|i| format!("10.0.0.{i}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(
            Invite::for_server(addrs(&many), fp(), None),
            Err(InviteError::TooManyAddrs)
        );
        assert_eq!(
            Invite::for_server(addrs(&["a.com"]), fp(), Some("가".repeat(65))),
            Err(InviteError::NameTooLong)
        );
        let json = format!(
            r#"{{"a":["a.com"],"f":"{}","p":"pw"}}"#,
            fp().to_base64url()
        );
        let code = format!("{PREFIX}{}", b64::encode(json));
        assert_eq!(
            Invite::decode(&code),
            Err(InviteError::PasswordWithoutBroadcaster)
        );
    }

    #[test]
    fn normalizes_duplicates_and_blank_fields() {
        let invite = Invite::for_broadcast(
            addrs(&["a.com", "a.com:7884", "b.com"]),
            fp(),
            Some("   ".into()),
            InstallId::from_bytes([1u8; 16]),
            Some(String::new()),
        )
        .unwrap();
        assert_eq!(invite.addrs(), addrs(&["a.com", "b.com"]).as_slice());
        assert_eq!(invite.name(), None);
        assert_eq!(invite.password(), None);
    }
}
