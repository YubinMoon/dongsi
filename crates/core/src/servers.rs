//! 클라이언트가 저장하는 서버 목록 (D-020, D-021, D-027, D-029).
//!
//! 규칙
//! - 주소로 추가: 같은 주소가 이미 있으면 그대로 둔다. 지문은 첫 접속 때 확인한다(TOFU).
//! - 초대 코드로 추가:
//!   1. 같은 지문의 서버가 있으면 주소 후보만 갱신하고, 사용자가 붙인 이름은 유지한다.
//!   2. 없으면, 같은 주소로 저장되어 있지만 아직 지문을 확인하지 않은 서버를 찾아 합친다.
//!   3. 그것도 없으면 새로 추가한다. 같은 주소라도 지문이 다르면 다른 서버로 본다.
//! - 방송 초대에 비밀번호가 있으면 (서버, 방송자) 단위로 기억한다.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::invite::MAX_PASSWORD_CHARS;
use crate::{Fingerprint, InstallId, Invite, ServerAddr, ServerId};

pub const MAX_SERVER_NAME_CHARS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedServer {
    pub id: ServerId,
    pub name: String,
    /// 접속 주소 후보. 앞에 있을수록 먼저 시도한다.
    pub addrs: Vec<ServerAddr>,
    /// 인증서 지문. 초대 코드로 추가했거나 첫 접속에서 기억한 값이다. `None`이면 아직 확인 전이다.
    pub fingerprint: Option<Fingerprint>,
    /// 맞힌 방송 비밀번호(방송자별, D-027).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub passwords: BTreeMap<InstallId, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddOutcome {
    Added(ServerId),
    /// 이미 있던 서버의 주소 후보나 지문을 갱신했다.
    Updated(ServerId),
    /// 같은 주소가 이미 저장되어 있어 아무것도 바꾸지 않았다.
    AlreadyExists(ServerId),
}

impl AddOutcome {
    pub fn id(&self) -> ServerId {
        match *self {
            AddOutcome::Added(id) | AddOutcome::Updated(id) | AddOutcome::AlreadyExists(id) => id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServerListError {
    #[error("서버를 찾을 수 없습니다")]
    NotFound,
    #[error("서버 이름을 입력해 주세요")]
    EmptyName,
    #[error("서버 이름이 너무 깁니다(최대 {}자)", MAX_SERVER_NAME_CHARS)]
    NameTooLong,
    #[error("비밀번호를 입력해 주세요")]
    EmptyPassword,
    #[error("비밀번호가 너무 깁니다(최대 {}자)", MAX_PASSWORD_CHARS)]
    PasswordTooLong,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ServerList {
    servers: Vec<SavedServer>,
}

impl ServerList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn iter(&self) -> impl Iterator<Item = &SavedServer> {
        self.servers.iter()
    }

    pub fn len(&self) -> usize {
        self.servers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    pub fn get(&self, id: ServerId) -> Option<&SavedServer> {
        self.servers.iter().find(|s| s.id == id)
    }

    pub fn find_by_addr(&self, addr: &ServerAddr) -> Option<&SavedServer> {
        self.servers.iter().find(|s| s.addrs.contains(addr))
    }

    /// 초대 코드를 추가하면 갱신될 기존 서버. 없으면 새로 추가된다.
    pub fn invite_target(&self, invite: &Invite) -> Option<&SavedServer> {
        self.invite_target_index(invite).map(|i| &self.servers[i])
    }

    /// 주소로 추가한다. `name`이 없으면 주소를 이름으로 쓴다.
    pub fn add_manual(
        &mut self,
        addr: ServerAddr,
        name: Option<&str>,
    ) -> Result<AddOutcome, ServerListError> {
        if let Some(existing) = self.find_by_addr(&addr) {
            return Ok(AddOutcome::AlreadyExists(existing.id));
        }
        let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
            Some(name) => validate_name(name)?,
            None => addr.to_string(),
        };
        let id = ServerId::generate();
        self.servers.push(SavedServer {
            id,
            name,
            addrs: vec![addr],
            fingerprint: None,
            passwords: BTreeMap::new(),
        });
        Ok(AddOutcome::Added(id))
    }

    /// 초대 코드로 추가하거나 기존 서버를 갱신한다.
    pub fn add_invite(&mut self, invite: &Invite) -> AddOutcome {
        let (index, outcome) = match self.invite_target_index(invite) {
            Some(index) => {
                let server = &mut self.servers[index];
                server.addrs = invite.addrs().to_vec();
                server.fingerprint = Some(*invite.fingerprint());
                (index, AddOutcome::Updated(server.id))
            }
            None => {
                let id = ServerId::generate();
                let name = invite
                    .name()
                    .map(str::to_owned)
                    .unwrap_or_else(|| invite.addrs()[0].to_string());
                self.servers.push(SavedServer {
                    id,
                    name,
                    addrs: invite.addrs().to_vec(),
                    fingerprint: Some(*invite.fingerprint()),
                    passwords: BTreeMap::new(),
                });
                (self.servers.len() - 1, AddOutcome::Added(id))
            }
        };
        if let (Some(broadcaster), Some(password)) = (invite.broadcaster(), invite.password()) {
            self.servers[index]
                .passwords
                .insert(*broadcaster, password.to_owned());
        }
        outcome
    }

    pub fn rename(&mut self, id: ServerId, name: &str) -> Result<(), ServerListError> {
        let name = validate_name(name.trim())?;
        self.get_mut(id)?.name = name;
        Ok(())
    }

    pub fn remove(&mut self, id: ServerId) -> Result<SavedServer, ServerListError> {
        let index = self
            .servers
            .iter()
            .position(|s| s.id == id)
            .ok_or(ServerListError::NotFound)?;
        Ok(self.servers.remove(index))
    }

    /// 맞힌 방송 비밀번호를 기억한다.
    pub fn remember_password(
        &mut self,
        id: ServerId,
        broadcaster: InstallId,
        password: &str,
    ) -> Result<(), ServerListError> {
        if password.is_empty() {
            return Err(ServerListError::EmptyPassword);
        }
        if password.chars().count() > MAX_PASSWORD_CHARS {
            return Err(ServerListError::PasswordTooLong);
        }
        self.get_mut(id)?
            .passwords
            .insert(broadcaster, password.to_owned());
        Ok(())
    }

    pub fn password_for(&self, id: ServerId, broadcaster: &InstallId) -> Option<&str> {
        self.get(id)?.passwords.get(broadcaster).map(String::as_str)
    }

    fn get_mut(&mut self, id: ServerId) -> Result<&mut SavedServer, ServerListError> {
        self.servers
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(ServerListError::NotFound)
    }

    fn invite_target_index(&self, invite: &Invite) -> Option<usize> {
        let fp = invite.fingerprint();
        self.servers
            .iter()
            .position(|s| s.fingerprint.as_ref() == Some(fp))
            .or_else(|| {
                self.servers.iter().position(|s| {
                    s.fingerprint.is_none() && s.addrs.iter().any(|a| invite.addrs().contains(a))
                })
            })
    }
}

fn validate_name(name: &str) -> Result<String, ServerListError> {
    if name.is_empty() {
        return Err(ServerListError::EmptyName);
    }
    if name.chars().count() > MAX_SERVER_NAME_CHARS {
        return Err(ServerListError::NameTooLong);
    }
    Ok(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(text: &str) -> ServerAddr {
        text.parse().unwrap()
    }

    fn fp(byte: u8) -> Fingerprint {
        Fingerprint::from_bytes([byte; Fingerprint::LEN])
    }

    fn server_invite(addrs: &[&str], fp_byte: u8, name: Option<&str>) -> Invite {
        Invite::for_server(
            addrs.iter().map(|a| addr(a)).collect(),
            fp(fp_byte),
            name.map(str::to_owned),
        )
        .unwrap()
    }

    #[test]
    fn manual_add_uses_address_as_default_name_and_dedupes() {
        let mut list = ServerList::new();
        let first = list.add_manual(addr("10.0.0.5"), None).unwrap();
        assert!(matches!(first, AddOutcome::Added(_)));
        assert_eq!(list.get(first.id()).unwrap().name, "10.0.0.5");
        assert_eq!(list.get(first.id()).unwrap().fingerprint, None);

        let again = list
            .add_manual(addr("10.0.0.5:7884"), Some("다른 이름"))
            .unwrap();
        assert_eq!(again, AddOutcome::AlreadyExists(first.id()));
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn manual_add_validates_name() {
        let mut list = ServerList::new();
        let long = "가".repeat(MAX_SERVER_NAME_CHARS + 1);
        assert_eq!(
            list.add_manual(addr("a.com"), Some(&long)),
            Err(ServerListError::NameTooLong)
        );
        let blank = list.add_manual(addr("a.com"), Some("   ")).unwrap();
        assert_eq!(list.get(blank.id()).unwrap().name, "a.com");
    }

    #[test]
    fn invite_with_same_fingerprint_updates_addresses_and_keeps_name() {
        let mut list = ServerList::new();
        let added = list.add_invite(&server_invite(&["1.2.3.4"], 1, Some("친구 서버")));
        list.rename(added.id(), "내가 붙인 이름").unwrap();

        let updated = list.add_invite(&server_invite(
            &["5.6.7.8", "friend.ddns.net"],
            1,
            Some("새 이름"),
        ));
        assert_eq!(updated, AddOutcome::Updated(added.id()));
        let server = list.get(added.id()).unwrap();
        assert_eq!(server.name, "내가 붙인 이름");
        assert_eq!(server.addrs, vec![addr("5.6.7.8"), addr("friend.ddns.net")]);
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn invite_merges_into_unverified_manual_entry() {
        let mut list = ServerList::new();
        let manual = list.add_manual(addr("192.168.0.23"), Some("거실")).unwrap();
        let outcome = list.add_invite(&server_invite(&["192.168.0.23", "100.64.0.9"], 3, None));
        assert_eq!(outcome, AddOutcome::Updated(manual.id()));
        let server = list.get(manual.id()).unwrap();
        assert_eq!(server.fingerprint, Some(fp(3)));
        assert_eq!(server.name, "거실");
    }

    #[test]
    fn same_address_with_different_fingerprint_is_a_different_server() {
        let mut list = ServerList::new();
        let first = list.add_invite(&server_invite(&["1.2.3.4"], 1, None));
        let second = list.add_invite(&server_invite(&["1.2.3.4"], 2, None));
        assert!(matches!(second, AddOutcome::Added(_)));
        assert_ne!(first.id(), second.id());
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn broadcast_invite_remembers_password() {
        let mut list = ServerList::new();
        let broadcaster = InstallId::from_bytes([5u8; 16]);
        let invite = Invite::for_broadcast(
            vec![addr("1.2.3.4")],
            fp(1),
            None,
            broadcaster,
            Some("pw".into()),
        )
        .unwrap();
        let outcome = list.add_invite(&invite);
        assert_eq!(list.password_for(outcome.id(), &broadcaster), Some("pw"));

        // 비밀번호를 뺀 코드를 다시 붙여 넣어도 기억한 비밀번호는 지우지 않는다.
        list.add_invite(&invite.without_password());
        assert_eq!(list.password_for(outcome.id(), &broadcaster), Some("pw"));
    }

    #[test]
    fn remember_password_and_errors() {
        let mut list = ServerList::new();
        let id = list.add_manual(addr("a.com"), None).unwrap().id();
        let who = InstallId::from_bytes([2u8; 16]);
        assert_eq!(
            list.remember_password(id, who, ""),
            Err(ServerListError::EmptyPassword)
        );
        list.remember_password(id, who, " 공백 포함 ").unwrap();
        assert_eq!(list.password_for(id, &who), Some(" 공백 포함 "));
        assert_eq!(
            list.remember_password(ServerId::generate(), who, "x"),
            Err(ServerListError::NotFound)
        );
    }

    #[test]
    fn rename_and_remove() {
        let mut list = ServerList::new();
        let id = list.add_manual(addr("a.com"), None).unwrap().id();
        assert_eq!(list.rename(id, "  "), Err(ServerListError::EmptyName));
        list.rename(id, " 새 이름 ").unwrap();
        assert_eq!(list.get(id).unwrap().name, "새 이름");
        assert_eq!(list.remove(id).unwrap().id, id);
        assert_eq!(list.remove(id), Err(ServerListError::NotFound));
    }

    #[test]
    fn serde_roundtrip() {
        let mut list = ServerList::new();
        list.add_manual(addr("a.com"), None).unwrap();
        let id = list
            .add_invite(&server_invite(&["[::1]:9000"], 4, Some("v6")))
            .id();
        list.remember_password(id, InstallId::from_bytes([3u8; 16]), "pw")
            .unwrap();
        let json = serde_json::to_string(&list).unwrap();
        assert_eq!(serde_json::from_str::<ServerList>(&json).unwrap(), list);
    }
}
