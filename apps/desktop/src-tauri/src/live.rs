//! 서버 접속, 방송 목록, 시청(SDP 중계), 로컬 감지 명령.
//!
//! 시청은 WebView2가 한다. 화면이 RTCPeerConnection의 offer를 넘기면 서버에 Watch를 보내고
//! answer를 돌려준다. 미디어는 WebView2와 서버(SFU)가 직접 주고받는다.

use std::time::Duration;

use dongsi_core::proto::ClientMsg;
use dongsi_net::discovery;
use tauri::{AppHandle, Emitter, EventTarget, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::broadcast::Broadcaster;
use crate::commands::AppState;
use crate::error::{CmdError, CmdResult};
use crate::hub::{Hub, parse_broadcaster};
use crate::paths::{AppEnv, AppPaths};
use crate::viewing::{self, BroadcastThumbnail, LocalServerView, ServerLive, WatchAnswer};
use crate::wifi::{self, NetworkPath};

const SCAN_WAIT: Duration = Duration::from_millis(700);

/// 서버에 접속하고(이미 접속했으면 그대로) 방송 목록을 돌려준다.
#[tauri::command]
pub async fn connect_server(
    hub: State<'_, Hub>,
    state: State<'_, AppState>,
    key: String,
) -> CmdResult<ServerLive> {
    let conn = hub.connect(&key).await?;
    Ok(viewing::server_live(&state, &conn))
}

/// 서버 연결을 끊는다. 그 연결로 열린 시청은 서버가 세션을 정리하고, 화면은 `server-closed`로 안다.
/// 방송 중(또는 방송을 시작하는 중)인 서버는 끊지 않는다(오류 `broadcasting`).
#[tauri::command]
pub fn disconnect_server(
    hub: State<'_, Hub>,
    broadcaster: State<'_, Broadcaster>,
    key: String,
) -> CmdResult<()> {
    let result = broadcaster.disconnect_unless_broadcasting(&hub, &key);
    if result.is_ok() {
        // 그 연결로 하던 시청이 잡은 WLAN media streaming mode를 놓는다(D-088).
        tauri::async_runtime::spawn_blocking(move || {
            wifi::release_watch_key(&key, "서버 연결 끊음")
        });
    }
    result
}

/// 지금 열려 있는 서버 연결의 키(처음 연결된 순서). 내장 서버(`host`)는 뺀다.
#[tauri::command]
pub fn connected_servers(hub: State<'_, Hub>) -> Vec<String> {
    hub.connected_keys()
}

/// 시청을 시작한다. `password`가 없으면 기억한 비밀번호를 쓴다. 로직은 [`viewing::watch`].
/// 저장한 서버 목록이 바뀌었으면(감지한 서버 저장, 비밀번호 기억) `servers-changed` 이벤트를 보낸다.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn watch_broadcast(
    app: AppHandle,
    hub: State<'_, Hub>,
    state: State<'_, AppState>,
    key: String,
    broadcaster_id: String,
    password: Option<String>,
    remember: bool,
    offer: String,
) -> CmdResult<WatchAnswer> {
    let result = viewing::watch(
        &hub,
        &state,
        &key,
        &broadcaster_id,
        password,
        remember,
        offer,
    )
    .await?;
    if result.servers_changed {
        let _ = app.emit("servers-changed", serde_json::json!({}));
    }
    // Wi-Fi 경로를 보고 시청 동안 media streaming mode를 켠다(D-088). 시험이 지나는 `viewing::watch`가 아니라 명령 층에서만 한다.
    let mut answer = result.answer;
    if let Some(conn) = hub.get(&key).await {
        answer.network = Some(wifi::watch_started(key, answer.session, conn.connected_ip).await);
    }
    Ok(answer)
}

#[tauri::command]
pub async fn stop_watch(hub: State<'_, Hub>, key: String, session: u64) -> CmdResult<()> {
    let mut target = None;
    if let Some(conn) = hub.get(&key).await {
        target = Some(conn.connected_ip);
        let _ = conn.send(ClientMsg::StopWatch { session });
    }
    wifi::watch_stopped(key, session, target).await;
    Ok(())
}

/// 그 서버 연결로 가는 지금 경로(Wi-Fi 신호 갱신용, 화면이 10초마다 부른다). 연결이 없으면 `null`.
#[tauri::command]
pub async fn network_path(hub: State<'_, Hub>, key: String) -> CmdResult<Option<NetworkPath>> {
    let Some(conn) = hub.get(&key).await else {
        return Ok(None);
    };
    Ok(Some(wifi::current(conn.connected_ip).await))
}

#[tauri::command]
pub async fn request_keyframe(hub: State<'_, Hub>, key: String, session: u64) -> CmdResult<()> {
    if let Some(conn) = hub.get(&key).await {
        conn.send(ClientMsg::RequestKeyframe { session })?;
    }
    Ok(())
}

/// 방송 타일의 썸네일(D-076). 없으면 `null`. 그림이 바뀌면 `broadcast-thumbnail` 이벤트가 온다.
/// 로직은 [`viewing::thumbnail`].
#[tauri::command]
pub async fn broadcast_thumbnail(
    hub: State<'_, Hub>,
    key: String,
    broadcaster_id: String,
) -> CmdResult<Option<BroadcastThumbnail>> {
    viewing::thumbnail(&hub, &key, &broadcaster_id).await
}

/// 같은 LAN·tailnet에서 진행 중인 방송을 찾는다(D-025, D-030). 로직은 [`viewing::local_server_views`].
#[tauri::command]
pub async fn scan_local(
    app: AppHandle,
    hub: State<'_, Hub>,
    state: State<'_, AppState>,
) -> CmdResult<Vec<LocalServerView>> {
    let port = app.state::<AppEnv>().port;
    let found = discovery::scan(port, SCAN_WAIT).await;
    let me = state.lock().state().install_id;
    let saved = viewing::saved_fingerprints(&state);
    Ok(viewing::local_server_views(&hub, found, me, &saved))
}

/// 시청 화면을 새 창으로 연다. 같은 방송의 창이 이미 있으면 앞으로 가져온다.
/// 내 방송은 열지 않는다(`ownBroadcast`, D-065).
#[tauri::command]
pub async fn open_watch_window(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    broadcaster_id: String,
    title: String,
) -> CmdResult<()> {
    let broadcaster = parse_broadcaster(&broadcaster_id)?;
    viewing::ensure_not_mine(&state, broadcaster)?;
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
        // 방송이 끝난 채로 남아 있던 창이면 다시 접속하게 한다.
        let _ = app.emit_to(
            EventTarget::webview_window(label.as_str()),
            "watch-reopen",
            (),
        );
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
    let mut builder = WebviewWindowBuilder::new(&app, label, WebviewUrl::External(url))
        .title(title)
        .inner_size(1280.0, 760.0)
        .min_inner_size(480.0, 300.0)
        // app.css의 --bg. WebView2가 준비되는 동안 흰 화면 대신 앱 바탕색이 보인다(메인 창은 tauri.conf.json).
        .background_color(tauri::window::Color(0x0d, 0x0e, 0x11, 0xff));
    // 프로필이 있으면 메인 창과 같은 WebView2 데이터 폴더를 쓴다.
    if let Some(dir) = app.state::<AppPaths>().webview_dir.clone() {
        builder = builder.data_directory(dir);
    }
    builder
        .build()
        .map_err(|e| CmdError::new(format!("창을 열지 못했습니다: {e}")))?;
    Ok(())
}
