//! 서버 인증서 SHA-256 지문.
//!
//! 초대 코드에는 base64url로 들어가고(D-029), 화면에서는 사람이 비교하기 쉬운
//! 콜론 구분 16진수로 보여 준다(TOFU 확인, D-021).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::b64;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Fingerprint([u8; Fingerprint::LEN]);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("인증서 지문 형식이 올바르지 않습니다")]
pub struct FingerprintError;

impl Fingerprint {
    pub const LEN: usize = 32;

    pub fn from_bytes(bytes: [u8; Self::LEN]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; Self::LEN] {
        &self.0
    }

    pub fn to_base64url(&self) -> String {
        b64::encode(self.0)
    }

    pub fn from_base64url(text: &str) -> Result<Self, FingerprintError> {
        b64::decode_array(text).map(Self).ok_or(FingerprintError)
    }

    /// `AB:CD:EF:...` 형식(32바이트 전체).
    pub fn to_hex_colon(&self) -> String {
        hex_colon(&self.0)
    }

    /// 목록에 보여 줄 짧은 형식(앞 8바이트).
    pub fn short(&self) -> String {
        hex_colon(&self.0[..8])
    }
}

fn hex_colon(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({})", self.short())
    }
}

impl TryFrom<String> for Fingerprint {
    type Error = FingerprintError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::from_base64url(&value)
    }
}

impl From<Fingerprint> for String {
    fn from(fp: Fingerprint) -> Self {
        fp.to_base64url()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Fingerprint {
        let mut bytes = [0u8; Fingerprint::LEN];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = i as u8;
        }
        Fingerprint::from_bytes(bytes)
    }

    #[test]
    fn base64url_roundtrip() {
        let fp = sample();
        let text = fp.to_base64url();
        assert_eq!(text.len(), 43); // 32바이트, 패딩 없음
        assert_eq!(Fingerprint::from_base64url(&text), Ok(fp));
    }

    #[test]
    fn rejects_wrong_length_or_alphabet() {
        assert_eq!(Fingerprint::from_base64url("AAAA"), Err(FingerprintError));
        assert_eq!(Fingerprint::from_base64url("!!!"), Err(FingerprintError));
    }

    #[test]
    fn hex_formats() {
        let fp = sample();
        assert_eq!(fp.short(), "00:01:02:03:04:05:06:07");
        assert_eq!(fp.to_hex_colon().len(), 32 * 3 - 1);
    }
}
