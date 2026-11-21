//! 시청 쪽 로직(Tauri 없이 부를 수 있다): 서버 접속 결과 보기, 시청 시작과 비밀번호 처리,
//! 로컬 감지 결과를 화면 목록으로 바꾸기.
//!
//! 시청 명령·감지 명령(`live.rs`)과 통합 시험이 같은 함수를 쓴다. 상태 저장소는 `AppState`를 그대로 쓴다.

use dongsi_core::proto::{BroadcastInfo, ClientMsg, ErrorCode, MAX_SDP_BYTES, ServerMsg};
use dongsi_core::{Fingerprint, Host, InstallId, Invite, ServerAddr, ServerId, VideoCodec};
use dongsi_net::discovery::{Discovered, Via};
use serde::Serialize;
use tracing::warn;

use crate::commands::{AppState, FingerprintView};
use crate::error::{CmdError, CmdResult};
use crate::hub::{Hub, ServerConn, Target, parse_broadcaster};

/// 로컬 감지로 찾은 서버의 허브 키 접두사.
pub const LOCAL_KEY_PREFIX: &str = "local-";

/// 서버 이름을 서버 목록에 쓸 때의 최대 글자 수(초대 코드 이름 한도).
const MAX_SAVED_NAME_CHARS: usize = dongsi_core::invite::MAX_NAME_CHARS;

/// 내 방송을 시청하려 할 때 돌려주는 문장(코드 `ownBroadcast`, D-065).
pub const OWN_BROADCAST_MESSAGE: &str = "내 방송은 이 PC에서 시청할 수 없습니다. 방송 소리가 이 PC에서 다시 재생되면 그 소리가 방송에 또 들어가 에코가 겹칩니다.";

/// 내 방송(같은 설치 ID)이면 `ownBroadcast` 오류를 돌려준다(D-065).
///
/// 이 PC에서 내 방송을 소리와 함께 틀면 오디오 루프백(시스템 소리 − Discord)이 그 소리를 다시 담아
/// 방송에 섞고, 그것이 또 재생되어 에코가 쌓인다. 그래서 방송자 본인의 시청은 시작하지 않는다.
pub fn ensure_not_mine(state: &AppState, broadcaster: InstallId) -> CmdResult<()> {
    if broadcaster == state.identity().0 {
        return Err(CmdError::with_code(OWN_BROADCAST_MESSAGE, "ownBroadcast"));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BroadcastView {
    pub broadcaster_id: String,
    pub broadcaster_name: String,
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub has_audio: bool,
    pub locked: bool,
    pub viewers: u32,
    pub started_at: u64,
    pub password_saved: bool,
    /// 내 방송(같은 설치 ID).
    pub mine: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerLive {
    pub key: String,
    pub server_name: String,
    pub can_publish: bool,
    pub host_only: bool,
    pub fingerprint: FingerprintView,
    pub broadcasts: Vec<BroadcastView>,
}

pub fn broadcast_views(
    state: &AppState,
    key: &str,
    list: Vec<BroadcastInfo>,
) -> Vec<BroadcastView> {
    let store = state.lock();
    let me = store.state().install_id;
    let saved = key
        .parse()
        .ok()
        .and_then(|id| store.state().servers.get(id));
    list.into_iter()
        .map(|b| BroadcastView {
            broadcaster_id: b.broadcaster.to_string(),
            password_saved: saved.is_some_and(|s| s.passwords.contains_key(&b.broadcaster)),
            mine: b.broadcaster == me,
            broadcaster_name: b.name,
            codec: b.codec,
            width: b.width,
            height: b.height,
            fps: b.fps,
            has_audio: b.has_audio,
            locked: b.locked,
            viewers: b.viewers,
            started_at: b.started_at,
        })
        .collect()
}

/// `connect_server`의 응답.
pub fn server_live(state: &AppState, conn: &ServerConn) -> ServerLive {
    ServerLive {
        key: conn.key.clone(),
        server_name: conn.server_name.clone(),
        can_publish: conn.can_publish,
        host_only: conn.host_only,
        fingerprint: conn.fingerprint.into(),
        broadcasts: broadcast_views(state, &conn.key, conn.broadcasts()),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchAnswer {
    pub session: u64,
    pub answer: String,
    /// 이번 시청으로 로컬 감지 서버를 서버 목록에 저장했거나, 같은 지문의 저장한 서버에 비밀번호를
    /// 기억했을 때만 있다(그 저장한 서버의 ID).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_server_id: Option<String>,
}

/// [`watch`]의 결과.
#[derive(Debug)]
pub struct WatchResult {
    pub answer: WatchAnswer,
    /// 저장한 서버 목록(서버 또는 기억한 비밀번호)이 바뀌었다. 명령은 `servers-changed` 이벤트를 보낸다.
    pub servers_changed: bool,
}

/// 시청을 시작한다(`Watch` 요청). `password`가 없으면 기억한 비밀번호를 쓴다.
///
/// - 내 방송이면 서버에 묻지 않고 `ownBroadcast`로 거부한다([`ensure_not_mine`]).
/// - 비밀번호 오류는 `passwordRequired`·`wrongPassword`·`notFound` 코드로 바꾼다. 기억한 비밀번호가
///   틀리면 지우고 "비밀번호가 바뀌었습니다. 다시 입력해 주세요."(`passwordRequired`)를 돌려준다.
/// - 비밀번호가 맞고 `remember`면 기억한다. 저장한 서버 키면 그 서버에, 로컬 감지 키(`local-…`)면
///   그 서버를 서버 목록에 저장하고 기억한다(이름=서버 이름, 주소=감지된 주소, 지문 고정).
///   같은 지문의 저장한 서버가 이미 있으면 새로 만들지 않고 그 서버에 기억한다(D-057).
///   `remember`가 아니면 아무것도 저장하지 않는다.
pub async fn watch(
    hub: &Hub,
    state: &AppState,
    key: &str,
    broadcaster_id: &str,
    password: Option<String>,
    remember: bool,
    offer: String,
) -> CmdResult<WatchResult> {
    if offer.len() > MAX_SDP_BYTES {
        return Err(CmdError::new("SDP가 너무 깁니다"));
    }
    let broadcaster = parse_broadcaster(broadcaster_id)?;
    ensure_not_mine(state, broadcaster)?;
    let conn = hub.connect(key).await?;
    let saved_id: Option<ServerId> = key.parse().ok();
    let typed = password.filter(|p| !p.is_empty());
    let saved_password = match (&typed, saved_id) {
        (None, Some(id)) => state
            .lock()
            .state()
            .servers
            .password_for(id, &broadcaster)
            .map(str::to_owned),
        _ => None,
    };
    let using_saved = typed.is_none() && saved_password.is_some();
    let password = typed.clone().or(saved_password);

    let reply = conn
        .request(|req| ClientMsg::Watch {
            req,
            broadcaster,
            password: password.clone(),
            offer,
        })
        .await?;
    match reply {
        ServerMsg::Watching {
            session, answer, ..
        } => {
            let mut saved_server_id = None;
            let mut servers_changed = false;
            if remember && let Some(p) = typed {
                match saved_id {
                    Some(id) => {
                        servers_changed = state
                            .lock()
                            .update(|s| Ok(s.servers.remember_password(id, broadcaster, &p)?))
                            .is_ok();
                    }
                    None if key.starts_with(LOCAL_KEY_PREFIX) => {
                        let addrs = hub
                            .registered_target(key)
                            .map(|t| t.addrs)
                            .unwrap_or_default();
                        saved_server_id = save_local_server(state, &conn, addrs, broadcaster, &p)
                            .map(|id| id.to_string());
                        servers_changed = saved_server_id.is_some();
                    }
                    None => {}
                }
            }
            Ok(WatchResult {
                answer: WatchAnswer {
                    session,
                    answer,
                    saved_server_id,
                },
                servers_changed,
            })
        }
        ServerMsg::Error { code, message, .. } => Err(match code {
            ErrorCode::PasswordRequired => CmdError::with_code(message, "passwordRequired"),
            ErrorCode::WrongPassword => {
                if using_saved && let Some(id) = saved_id {
                    // 방송자가 비밀번호를 바꿨다. 기억한 값을 지운다.
                    let _ = state
                        .lock()
                        .update(|s| Ok(s.servers.forget_password(id, &broadcaster)?));
                    CmdError::with_code(
                        "비밀번호가 바뀌었습니다. 다시 입력해 주세요.",
                        "passwordRequired",
                    )
                } else {
                    CmdError::with_code(message, "wrongPassword")
                }
            }
            ErrorCode::NotFound => CmdError::with_code(message, "notFound"),
            _ => CmdError::new(message),
        }),
        _ => Err(CmdError::new("서버 응답이 올바르지 않습니다")),
    }
}

/// 로컬 감지로 찾은 서버에서 비밀번호가 맞았을 때 서버 목록에 저장하고 비밀번호를 기억한다.
/// 같은 지문의 저장한 서버가 있으면 그 서버에만 기억한다. 저장하지 못하면 `None`(시청은 계속된다).
fn save_local_server(
    state: &AppState,
    conn: &ServerConn,
    addrs: Vec<ServerAddr>,
    broadcaster: InstallId,
    password: &str,
) -> Option<ServerId> {
    let fingerprint = conn.fingerprint;
    let mut store = state.lock();
    let existing = store
        .state()
        .servers
        .iter()
        .find(|s| s.fingerprint == Some(fingerprint))
        .map(|s| s.id);
    if let Some(id) = existing {
        return store
            .update(|s| Ok(s.servers.remember_password(id, broadcaster, password)?))
            .ok()
            .map(|_| id);
    }
    let name: String = conn
        .server_name
        .chars()
        .take(MAX_SAVED_NAME_CHARS)
        .collect();
    let invite = match Invite::for_broadcast(
        addrs,
        fingerprint,
        Some(name),
        broadcaster,
        Some(password.to_owned()),
    ) {
        Ok(invite) => invite,
        Err(e) => {
            warn!("감지한 서버를 저장하지 못했습니다: {e}");
            return None;
        }
    };
    store
        .update(|s| Ok(s.servers.add_invite(&invite)))
        .ok()
        .map(|outcome| outcome.id())
}

// ---------- 로컬 감지 ----------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalBroadcastItem {
    pub broadcaster_id: String,
    pub broadcaster_name: String,
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub locked: bool,
    /// 그 감지 서버가 저장한 서버이고 이 방송자의 비밀번호를 기억해 뒀다(`BroadcastView::password_saved`와 같은 규칙).
    pub password_saved: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalServerView {
    /// 시청할 때 쓰는 키(저장한 서버와 지문이 같으면 그 서버 ID).
    pub key: String,
    pub name: String,
    pub addr: String,
    /// `"lan"`, `"tailscale"`, `"local"`(이 PC)
    pub via: &'static str,
    pub host_only: bool,
    pub saved: bool,
    pub broadcasts: Vec<LocalBroadcastItem>,
}

/// 감지 결과와 맞춰 볼 저장한 서버(지문을 확인한 서버만).
#[derive(Debug, Clone)]
pub struct SavedFingerprint {
    pub fingerprint: Fingerprint,
    pub id: ServerId,
    /// 비밀번호를 기억해 둔 방송자.
    pub remembered: Vec<InstallId>,
}

/// 저장한 서버의 지문·ID·비밀번호를 기억한 방송자. 지문을 아직 확인하지 않은 서버는 뺀다.
pub fn saved_fingerprints(state: &AppState) -> Vec<SavedFingerprint> {
    state
        .lock()
        .state()
        .servers
        .iter()
        .filter_map(|s| {
            Some(SavedFingerprint {
                fingerprint: s.fingerprint?,
                id: s.id,
                remembered: s.passwords.keys().copied().collect(),
            })
        })
        .collect()
}

/// 감지 결과를 화면 목록으로 바꾸고, 시청할 때 접속할 대상을 Hub에 등록한다.
///
/// - 내 방송(`me`)과 방송이 없는 서버는 뺀다.
/// - 저장한 서버와 지문이 같으면 키는 그 서버 ID다. 접속은 감지된 주소를 먼저, 저장한 주소를 뒤에 둔다
///   (메모리에서만. 저장한 서버 목록은 바꾸지 않는다).
/// - 아니면 키는 `local-<지문>`이고 감지된 주소로 접속하는 대상을 등록한다.
pub fn local_server_views(
    hub: &Hub,
    found: Vec<Discovered>,
    me: InstallId,
    saved: &[SavedFingerprint],
) -> Vec<LocalServerView> {
    let mut out = Vec::new();
    for d in found {
        let fp = d.fingerprint();
        let saved_server = saved.iter().find(|s| s.fingerprint == fp);
        let broadcasts: Vec<LocalBroadcastItem> = d
            .response
            .broadcasts
            .iter()
            .filter(|b| b.broadcaster != me)
            .map(|b| LocalBroadcastItem {
                broadcaster_id: b.broadcaster.to_string(),
                broadcaster_name: b.name.clone(),
                codec: b.codec,
                width: b.width,
                height: b.height,
                fps: b.fps,
                locked: b.locked,
                password_saved: saved_server.is_some_and(|s| s.remembered.contains(&b.broadcaster)),
            })
            .collect();
        // 방송이 없는 전용 서버는 목록에 띄우지 않는다(방송 감지가 목적).
        if broadcasts.is_empty() {
            continue;
        }
        let addr = ServerAddr::new(Host::Ip(d.ip), d.response.port);
        let saved_id = saved_server.map(|s| s.id);
        let key = match saved_id {
            Some(id) => {
                let key = id.to_string();
                hub.set_preferred_addr(&key, addr.clone());
                key
            }
            None => {
                let key = format!("{LOCAL_KEY_PREFIX}{}", fp.to_base64url());
                hub.set_target(
                    &key,
                    Target {
                        addrs: vec![addr.clone()],
                        fingerprint: Some(fp),
                        saved: None,
                        host_token: None,
                    },
                );
                key
            }
        };
        out.push(LocalServerView {
            key,
            name: d.response.name.clone(),
            addr: addr.to_string(),
            via: match d.via {
                Via::Lan => "lan",
                Via::Tailscale => "tailscale",
                Via::Local => "local",
            },
            host_only: d.response.host_only,
            saved: saved_id.is_some(),
            broadcasts,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_answer_adds_saved_server_id_only_when_present() {
        let plain = WatchAnswer {
            session: 3,
            answer: "sdp".into(),
            saved_server_id: None,
        };
        assert_eq!(
            serde_json::to_value(&plain).unwrap(),
            serde_json::json!({ "session": 3, "answer": "sdp" })
        );
        let saved = WatchAnswer {
            saved_server_id: Some("abc".into()),
            ..plain
        };
        assert_eq!(
            serde_json::to_value(&saved).unwrap(),
            serde_json::json!({ "session": 3, "answer": "sdp", "savedServerId": "abc" })
        );
    }
}
