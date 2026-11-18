//! 서버 접속, 방송 목록, 시청(SDP 중계), 로컬 감지 명령.
//!
//! 시청은 WebView2가 한다. 화면이 RTCPeerConnection의 offer를 넘기면 서버에 Watch를 보내고
//! answer를 돌려준다. 미디어는 WebView2와 서버(SFU)가 직접 주고받는다.

use std::time::Duration;

use dongsi_core::proto::{BroadcastInfo, ClientMsg, ErrorCode, MAX_SDP_BYTES, ServerMsg};
use dongsi_core::{DEFAULT_PORT, Host, ServerAddr, VideoCodec};
use dongsi_net::discovery::{self, Via};
use serde::Serialize;
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::commands::{AppState, FingerprintView};
use crate::error::{CmdError, CmdResult};
use crate::hub::{Hub, ServerConn, Target, parse_broadcaster};

const SCAN_WAIT: Duration = Duration::from_millis(700);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BroadcastView {
    broadcaster_id: String,
    broadcaster_name: String,
    codec: VideoCodec,
    width: u32,
    height: u32,
    fps: u32,
    has_audio: bool,
    locked: bool,
    viewers: u32,
    started_at: u64,
    password_saved: bool,
    /// 내 방송(같은 설치 ID).
    mine: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerLive {
    key: String,
    server_name: String,
    can_publish: bool,
    host_only: bool,
    fingerprint: FingerprintView,
    broadcasts: Vec<BroadcastView>,
}

fn broadcast_views(app: &AppHandle, key: &str, list: Vec<BroadcastInfo>) -> Vec<BroadcastView> {
    let state = app.state::<AppState>();
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

fn live_view(app: &AppHandle, conn: &ServerConn) -> ServerLive {
    ServerLive {
        key: conn.key.clone(),
        server_name: conn.server_name.clone(),
        can_publish: conn.can_publish,
        host_only: conn.host_only,
        fingerprint: conn.fingerprint.into(),
        broadcasts: broadcast_views(app, &conn.key, conn.broadcasts()),
    }
}

/// 서버에 접속하고(이미 접속했으면 그대로) 방송 목록을 돌려준다.
#[tauri::command]
pub async fn connect_server(
    app: AppHandle,
    hub: State<'_, Hub>,
    key: String,
) -> CmdResult<ServerLive> {
    let conn = hub.connect(&app, &key).await?;
    Ok(live_view(&app, &conn))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchAnswer {
    session: u64,
    answer: String,
}

/// 시청을 시작한다. `password`가 없으면 기억한 비밀번호를 쓴다.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn watch_broadcast(
    app: AppHandle,
    hub: State<'_, Hub>,
    key: String,
    broadcaster_id: String,
    password: Option<String>,
    remember: bool,
    offer: String,
) -> CmdResult<WatchAnswer> {
    if offer.len() > MAX_SDP_BYTES {
        return Err(CmdError::new("SDP가 너무 깁니다"));
    }
    let broadcaster = parse_broadcaster(&broadcaster_id)?;
    let conn = hub.connect(&app, &key).await?;
    let saved_id = key.parse().ok();
    let typed = password.filter(|p| !p.is_empty());
    let saved_password = match (&typed, saved_id) {
        (None, Some(id)) => app
            .state::<AppState>()
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
            if remember && let (Some(id), Some(p)) = (saved_id, typed) {
                let state = app.state::<AppState>();
                let mut store = state.lock();
                let _ = store.update(|s| Ok(s.servers.remember_password(id, broadcaster, &p)?));
            }
            Ok(WatchAnswer { session, answer })
        }
        ServerMsg::Error { code, message, .. } => Err(match code {
            ErrorCode::PasswordRequired => CmdError::with_code(message, "passwordRequired"),
            ErrorCode::WrongPassword => {
                if using_saved && let Some(id) = saved_id {
                    // 방송자가 비밀번호를 바꿨다. 기억한 값을 지운다.
                    let state = app.state::<AppState>();
                    let mut store = state.lock();
                    let _ = store.update(|s| Ok(s.servers.forget_password(id, &broadcaster)?));
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

#[tauri::command]
pub async fn stop_watch(hub: State<'_, Hub>, key: String, session: u64) -> CmdResult<()> {
    if let Some(conn) = hub.get(&key).await {
        let _ = conn.send(ClientMsg::StopWatch { session });
    }
    Ok(())
}

#[tauri::command]
pub async fn request_keyframe(hub: State<'_, Hub>, key: String, session: u64) -> CmdResult<()> {
    if let Some(conn) = hub.get(&key).await {
        conn.send(ClientMsg::RequestKeyframe { session })?;
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalBroadcastItem {
    broadcaster_id: String,
    broadcaster_name: String,
    codec: VideoCodec,
    width: u32,
    height: u32,
    fps: u32,
    locked: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalServerView {
    /// 시청할 때 쓰는 키(저장한 서버와 지문이 같으면 그 서버 ID).
    key: String,
    name: String,
    addr: String,
    /// `"lan"`, `"tailscale"`, `"local"`(이 PC)
    via: &'static str,
    host_only: bool,
    saved: bool,
    broadcasts: Vec<LocalBroadcastItem>,
}

/// 같은 LAN·tailnet에서 진행 중인 방송을 찾는다(D-025, D-030).
#[tauri::command]
pub async fn scan_local(app: AppHandle, hub: State<'_, Hub>) -> CmdResult<Vec<LocalServerView>> {
    let found = discovery::scan(DEFAULT_PORT, SCAN_WAIT).await;
    let (me, saved): (_, Vec<_>) = {
        let state = app.state::<AppState>();
        let store = state.lock();
        (
            store.state().install_id,
            store
                .state()
                .servers
                .iter()
                .filter_map(|s| s.fingerprint.map(|fp| (fp, s.id)))
                .collect(),
        )
    };
    let mut out = Vec::new();
    for d in found {
        let fp = d.fingerprint();
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
            })
            .collect();
        // 방송이 없는 전용 서버는 목록에 띄우지 않는다(방송 감지가 목적).
        if broadcasts.is_empty() {
            continue;
        }
        let addr = ServerAddr::new(Host::Ip(d.ip), d.response.port);
        let saved_id = saved.iter().find(|(f, _)| *f == fp).map(|(_, id)| *id);
        let key = match saved_id {
            Some(id) => id.to_string(),
            None => {
                let key = format!("local-{}", fp.to_base64url());
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
    Ok(out)
}

/// 시청 화면을 새 창으로 연다. 같은 방송의 창이 이미 있으면 앞으로 가져온다.
#[tauri::command]
pub async fn open_watch_window(
    app: AppHandle,
    key: String,
    broadcaster_id: String,
    title: String,
) -> CmdResult<()> {
    let broadcaster = parse_broadcaster(&broadcaster_id)?;
    let label: String = format!("watch-{key}-{broadcaster}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.unminimize();
        let _ = w.set_focus();
        return Ok(());
    }
    // 메인 창과 같은 출처(개발: vite 서버, 배포: 앱 내장)에서 해시 경로만 바꾼다.
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| CmdError::new("메인 창이 없습니다"))?;
    let mut url = main.url().map_err(|e| CmdError::new(e.to_string()))?;
    url.set_fragment(Some(&format!("/watch/{key}/{broadcaster_id}?popout=1")));
    let title = if title.trim().is_empty() {
        "dongsi 시청".to_owned()
    } else {
        format!("{} - dongsi", title.trim())
    };
    WebviewWindowBuilder::new(&app, label, WebviewUrl::External(url))
        .title(title)
        .inner_size(1280.0, 760.0)
        .min_inner_size(480.0, 300.0)
        .build()
        .map_err(|e| CmdError::new(format!("창을 열지 못했습니다: {e}")))?;
    Ok(())
}
