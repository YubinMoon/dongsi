//! 화면에서 호출하는 Tauri 명령. 응답 구조체는 camelCase로 직렬화한다
//! (화면 쪽 타입은 `src/lib/types.ts`).

use std::sync::{Mutex, MutexGuard};

use dongsi_core::{AddOutcome, Fingerprint, InstallId, Invite, SavedServer, ServerAddr, ServerId};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::error::{CmdError, CmdResult};
use crate::hub::Hub;
use crate::sample;
use crate::store::Store;

pub struct AppState(Mutex<Store>);

impl AppState {
    pub fn new(store: Store) -> Self {
        Self(Mutex::new(store))
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Store> {
        // 다른 명령이 패닉해도 저장된 상태는 일관되므로 계속 쓴다.
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

// ---------- 응답 구조체 ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    display_name: String,
    install_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FingerprintView {
    short: String,
    full: String,
}

impl From<Fingerprint> for FingerprintView {
    fn from(fp: Fingerprint) -> Self {
        Self::from(&fp)
    }
}

impl From<&Fingerprint> for FingerprintView {
    fn from(fp: &Fingerprint) -> Self {
        Self {
            short: fp.short(),
            full: fp.to_hex_colon(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerView {
    id: String,
    name: String,
    addrs: Vec<String>,
    /// `None`이면 아직 첫 접속 전이라 지문을 확인하지 않았다.
    fingerprint: Option<FingerprintView>,
    saved_passwords: usize,
}

impl From<&SavedServer> for ServerView {
    fn from(s: &SavedServer) -> Self {
        Self {
            id: s.id.to_string(),
            name: s.name.clone(),
            addrs: s.addrs.iter().map(ToString::to_string).collect(),
            fingerprint: s.fingerprint.as_ref().map(FingerprintView::from),
            saved_passwords: s.passwords.len(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExistingServer {
    id: String,
    name: String,
}

impl From<&SavedServer> for ExistingServer {
    fn from(s: &SavedServer) -> Self {
        Self {
            id: s.id.to_string(),
            name: s.name.clone(),
        }
    }
}

/// 서버 추가 입력란의 해석 결과(추가하기 전 미리보기).
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ParsedInput {
    #[serde(rename_all = "camelCase")]
    Address {
        addr: String,
        /// 같은 주소로 이미 저장된 서버.
        existing: Option<ExistingServer>,
    },
    #[serde(rename_all = "camelCase")]
    Invite {
        name: Option<String>,
        addrs: Vec<String>,
        fingerprint: FingerprintView,
        broadcast: bool,
        has_password: bool,
        /// 추가하면 갱신될 기존 서버.
        existing: Option<ExistingServer>,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    Added,
    Updated,
    AlreadyExists,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddResult {
    outcome: Outcome,
    server: ServerView,
}

// ---------- 명령 ----------

fn settings_view(store: &Store) -> SettingsView {
    let state = store.state();
    SettingsView {
        display_name: state.display_name.clone(),
        install_id: state.install_id.to_string(),
    }
}

fn parse_server_id(id: &str) -> CmdResult<ServerId> {
    id.parse()
        .map_err(|_| CmdError::new("서버를 찾을 수 없습니다"))
}

fn server_view(store: &Store, id: ServerId) -> CmdResult<ServerView> {
    store
        .state()
        .servers
        .get(id)
        .map(ServerView::from)
        .ok_or_else(|| CmdError::new("서버를 찾을 수 없습니다"))
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> SettingsView {
    settings_view(&state.lock())
}

#[tauri::command]
pub fn set_display_name(state: State<'_, AppState>, name: String) -> CmdResult<SettingsView> {
    let mut store = state.lock();
    store.update(|s| s.set_display_name(&name))?;
    Ok(settings_view(&store))
}

#[tauri::command]
pub fn list_servers(state: State<'_, AppState>) -> Vec<ServerView> {
    state
        .lock()
        .state()
        .servers
        .iter()
        .map(ServerView::from)
        .collect()
}

/// 서버 추가 입력란에 넣은 값을 해석한다. 초대 코드처럼 보이면 초대 코드로, 아니면 주소로 읽는다.
#[tauri::command]
pub fn parse_server_input(state: State<'_, AppState>, text: String) -> CmdResult<ParsedInput> {
    let store = state.lock();
    let servers = &store.state().servers;
    if Invite::looks_like(&text) {
        let invite = Invite::decode(&text)?;
        return Ok(ParsedInput::Invite {
            name: invite.name().map(str::to_owned),
            addrs: invite.addrs().iter().map(ToString::to_string).collect(),
            fingerprint: invite.fingerprint().into(),
            broadcast: invite.broadcaster().is_some(),
            has_password: invite.password().is_some(),
            existing: servers.invite_target(&invite).map(ExistingServer::from),
        });
    }
    let addr: ServerAddr = text.parse()?;
    Ok(ParsedInput::Address {
        existing: servers.find_by_addr(&addr).map(ExistingServer::from),
        addr: addr.to_string(),
    })
}

/// 주소나 초대 코드로 서버를 추가한다. `name`은 주소로 추가할 때만 쓴다.
#[tauri::command]
pub fn add_server(
    state: State<'_, AppState>,
    text: String,
    name: Option<String>,
) -> CmdResult<AddResult> {
    let mut store = state.lock();
    let outcome = if Invite::looks_like(&text) {
        let invite = Invite::decode(&text)?;
        store.update(|s| Ok(s.servers.add_invite(&invite)))?
    } else {
        let addr: ServerAddr = text.parse()?;
        store.update(|s| Ok(s.servers.add_manual(addr, name.as_deref())?))?
    };
    let (outcome, id) = match outcome {
        AddOutcome::Added(id) => (Outcome::Added, id),
        AddOutcome::Updated(id) => (Outcome::Updated, id),
        AddOutcome::AlreadyExists(id) => (Outcome::AlreadyExists, id),
    };
    Ok(AddResult {
        outcome,
        server: server_view(&store, id)?,
    })
}

#[tauri::command]
pub fn rename_server(
    state: State<'_, AppState>,
    id: String,
    name: String,
) -> CmdResult<ServerView> {
    let id = parse_server_id(&id)?;
    let mut store = state.lock();
    store.update(|s| Ok(s.servers.rename(id, &name)?))?;
    server_view(&store, id)
}

#[tauri::command]
pub fn remove_server(app: AppHandle, state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let server_id = parse_server_id(&id)?;
    state
        .lock()
        .update(|s| Ok(s.servers.remove(server_id).map(drop)?))?;
    tauri::async_runtime::spawn(async move {
        app.state::<Hub>().disconnect(&id).await;
    });
    Ok(())
}

/// 방송 비밀번호를 기억한다(D-027).
#[tauri::command]
pub fn remember_password(
    state: State<'_, AppState>,
    server_id: String,
    broadcaster_id: String,
    password: String,
) -> CmdResult<()> {
    let id = parse_server_id(&server_id)?;
    let broadcaster: InstallId = broadcaster_id
        .parse()
        .map_err(|_| CmdError::new("방송자를 찾을 수 없습니다"))?;
    state
        .lock()
        .update(|s| Ok(s.servers.remember_password(id, broadcaster, &password)?))
}

/// 개발용: 화면에서 측정한 값을 앱 로그에 남긴다(시청 통계 등). 배포 빌드에서는 무시한다.
#[tauri::command]
pub fn dev_log(message: String) {
    if cfg!(debug_assertions) {
        tracing::info!(target: "dongsi_desktop_lib::webview", "{message}");
    }
}

/// 개발용 예시 초대 코드. 배포 빌드에서는 쓸 수 없다.
#[tauri::command]
pub fn dev_sample_invite() -> CmdResult<String> {
    if cfg!(debug_assertions) {
        Ok(sample::invite_code())
    } else {
        Err(CmdError::new("개발 빌드에서만 쓸 수 있습니다"))
    }
}
