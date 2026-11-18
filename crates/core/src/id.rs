//! 식별자.
//!
//! - [`InstallId`]: 앱 설치마다 한 번 만드는 고정 ID. 방송자를 구분하고, 방송 초대 코드에
//!   들어간다(D-027, D-029). 짧게 보이도록 base64url(22자)로 직렬화한다.
//! - [`ServerId`]: 클라이언트가 저장한 서버 항목의 로컬 ID. 이 PC 밖으로 나가지 않는다.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::b64;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("ID 형식이 올바르지 않습니다")]
pub struct IdError;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InstallId(Uuid);

impl InstallId {
    pub fn generate() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(Uuid::from_bytes(bytes))
    }
}

impl fmt::Display for InstallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&b64::encode(self.0.as_bytes()))
    }
}

impl fmt::Debug for InstallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "InstallId({self})")
    }
}

impl FromStr for InstallId {
    type Err = IdError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        b64::decode_array(text).map(Self::from_bytes).ok_or(IdError)
    }
}

impl TryFrom<String> for InstallId {
    type Error = IdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<InstallId> for String {
    fn from(id: InstallId) -> Self {
        id.to_string()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ServerId(Uuid);

impl ServerId {
    pub fn generate() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }
}

impl fmt::Display for ServerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl fmt::Debug for ServerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ServerId({})", self.0)
    }
}

impl FromStr for ServerId {
    type Err = IdError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(text).map(Self).map_err(|_| IdError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_id_is_short_and_roundtrips() {
        let id = InstallId::generate();
        let text = id.to_string();
        assert_eq!(text.len(), 22);
        assert_eq!(text.parse::<InstallId>(), Ok(id));
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<InstallId>(&json).unwrap(), id);
    }

    #[test]
    fn install_id_works_as_json_map_key() {
        let mut map = std::collections::BTreeMap::new();
        map.insert(InstallId::generate(), "pw".to_string());
        let json = serde_json::to_string(&map).unwrap();
        let back: std::collections::BTreeMap<InstallId, String> =
            serde_json::from_str(&json).unwrap();
        assert_eq!(back, map);
    }

    #[test]
    fn server_id_roundtrips() {
        let id = ServerId::generate();
        assert_eq!(id.to_string().parse::<ServerId>(), Ok(id));
        assert_eq!("nope".parse::<ServerId>(), Err(IdError));
    }
}
