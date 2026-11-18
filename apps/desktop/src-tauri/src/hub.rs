//! 서버 연결 관리: 서버마다 시그널링 연결 하나를 열어 두고 방송 목록을 받는다.
//!
//! - 키: 저장한 서버는 서버 ID, 로컬 감지로 찾은 서버는 `local-<지문>`, 내장 서버는 `host`.
//! - 서버가 보내는 변화(방송 목록, 방송 종료, 시청자 수, 연결 끊김)는 화면에 이벤트로 알린다.
//!   화면은 이벤트를 받으면 필요한 명령을 다시 불러 최신 상태를 가져온다.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dongsi_core::proto::{BroadcastInfo, ClientMsg, ErrorCode, PROTOCOL_VERSION, ServerMsg};
use dongsi_core::{Fingerprint, InstallId, ServerAddr, ServerId};
use dongsi_net::NetError;
use dongsi_net::signal::{self, SignalSender};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

use crate::commands::AppState;
use crate::error::{CmdError, CmdResult};

const WELCOME_TIMEOUT: Duration = Duration::from_secs(8);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 접속 대상.
#[derive(Debug, Clone)]
pub struct Target {
    pub addrs: Vec<ServerAddr>,
    /// `None`이면 처음 받은 지문을 믿는다(TOFU). 저장한 서버면 그 지문을 기억한다.
    pub fingerprint: Option<Fingerprint>,
    pub saved: Option<ServerId>,
    /// 내장 서버에서 방송하려면 필요한 토큰.
    pub host_token: Option<String>,
}

/// 열린 서버 연결.
pub struct ServerConn {
    pub key: String,
    sender: SignalSender,
    pub connected_ip: IpAddr,
    pub fingerprint: Fingerprint,
    pub server_name: String,
    pub can_publish: bool,
    pub host_only: bool,
    broadcasts: Mutex<Vec<BroadcastInfo>>,
    pending: Mutex<HashMap<u32, oneshot::Sender<ServerMsg>>>,
    next_req: AtomicU32,
    closed: AtomicBool,
    pub viewer_count: AtomicU32,
}

impl ServerConn {
    pub fn broadcasts(&self) -> Vec<BroadcastInfo> {
        self.broadcasts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed) || self.sender.is_closed()
    }

    pub fn send(&self, msg: ClientMsg) -> CmdResult<()> {
        self.sender
            .send(msg)
            .map_err(|_| CmdError::new("서버와 연결이 끊어졌습니다"))
    }

    /// 요청을 보내고 같은 `req`의 응답(또는 오류)을 기다린다.
    pub async fn request(&self, build: impl FnOnce(u32) -> ClientMsg) -> CmdResult<ServerMsg> {
        let req = self.next_req.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(req, tx);
        self.send(build(req))?;
        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(msg)) => Ok(msg),
            Ok(Err(_)) => Err(CmdError::new("서버와 연결이 끊어졌습니다")),
            Err(_) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&req);
                Err(CmdError::new("서버가 응답하지 않습니다"))
            }
        }
    }
}

type Slot = Arc<tokio::sync::Mutex<Option<Arc<ServerConn>>>>;

#[derive(Default)]
pub struct Hub {
    slots: Mutex<HashMap<String, Slot>>,
    /// 저장하지 않은 대상(로컬 감지, 내장 서버).
    targets: Mutex<HashMap<String, Target>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct KeyEvent<'a> {
    key: &'a str,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BroadcastEndedEvent<'a> {
    key: &'a str,
    broadcaster_id: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClosedEvent<'a> {
    key: &'a str,
    reason: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ViewerCountEvent<'a> {
    key: &'a str,
    count: u32,
}

impl Hub {
    pub fn set_target(&self, key: &str, target: Target) {
        self.targets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_owned(), target);
    }

    pub fn remove_target(&self, key: &str) {
        self.targets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
    }

    fn target(&self, app: &AppHandle, key: &str) -> CmdResult<Target> {
        if let Some(t) = self
            .targets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
        {
            return Ok(t.clone());
        }
        let id: ServerId = key
            .parse()
            .map_err(|_| CmdError::new("서버를 찾을 수 없습니다"))?;
        let state = app.state::<AppState>();
        let store = state.lock();
        let server = store
            .state()
            .servers
            .get(id)
            .ok_or_else(|| CmdError::new("서버를 찾을 수 없습니다"))?;
        Ok(Target {
            addrs: server.addrs.clone(),
            fingerprint: server.fingerprint,
            saved: Some(id),
            host_token: None,
        })
    }

    fn slot(&self, key: &str) -> Slot {
        self.slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(key.to_owned())
            .or_default()
            .clone()
    }

    /// 열린 연결을 돌려주고, 없으면 접속한다.
    pub async fn connect(&self, app: &AppHandle, key: &str) -> CmdResult<Arc<ServerConn>> {
        let slot = self.slot(key);
        let mut guard = slot.lock().await;
        if let Some(conn) = guard.as_ref()
            && !conn.is_closed()
        {
            return Ok(conn.clone());
        }
        let target = self.target(app, key)?;
        let conn = open(app, key, target).await?;
        *guard = Some(conn.clone());
        Ok(conn)
    }

    /// 이미 열린 연결만 돌려준다.
    pub async fn get(&self, key: &str) -> Option<Arc<ServerConn>> {
        let slot = self.slot(key);
        let guard = slot.lock().await;
        guard.as_ref().filter(|c| !c.is_closed()).cloned()
    }

    /// 연결을 닫는다(서버 삭제, 내장 서버 종료).
    pub async fn disconnect(&self, key: &str) {
        let slot = self.slot(key);
        slot.lock().await.take();
        self.slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
    }
}

fn net_error(e: NetError) -> CmdError {
    match e {
        NetError::FingerprintMismatch { expected, actual } => CmdError::with_code(
            format!(
                "서버 인증서 지문이 저장된 것과 다릅니다(저장 {}…, 받은 값 {}…). \
                 서버를 다시 설치했다면 목록에서 삭제하고 새 초대 코드로 추가해 주세요.",
                expected.short(),
                actual.short()
            ),
            "fingerprintMismatch",
        ),
        other => CmdError::new(other.to_string()),
    }
}

async fn open(app: &AppHandle, key: &str, target: Target) -> CmdResult<Arc<ServerConn>> {
    let mut conn = signal::connect_any(&target.addrs, target.fingerprint)
        .await
        .map_err(net_error)?;
    let (install_id, display_name) = {
        let state = app.state::<AppState>();
        let store = state.lock();
        (store.state().install_id, store.state().display_name.clone())
    };
    conn.sender
        .send(ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
            app_version: env!("CARGO_PKG_VERSION").into(),
            install_id,
            display_name,
            connected_ip: Some(signal::connected_ip(&conn)),
            host_token: target.host_token.clone(),
        })
        .map_err(net_error)?;
    let first = tokio::time::timeout(WELCOME_TIMEOUT, conn.receiver.recv())
        .await
        .map_err(|_| CmdError::new("서버가 응답하지 않습니다"))?
        .ok_or_else(|| CmdError::new("서버가 연결을 끊었습니다"))?;
    let (server_name, can_publish, host_only, broadcasts) = match first {
        ServerMsg::Welcome {
            server_name,
            can_publish,
            host_only,
            broadcasts,
            ..
        } => (server_name, can_publish, host_only, broadcasts),
        ServerMsg::Error { code, message, .. } => {
            return Err(match code {
                ErrorCode::ProtocolMismatch => CmdError::new(
                    "서버와 앱의 버전이 맞지 않습니다. 앱이나 서버를 업데이트해 주세요.",
                ),
                _ => CmdError::new(message),
            });
        }
        _ => return Err(CmdError::new("서버 응답이 올바르지 않습니다")),
    };

    // 첫 접속: 받은 지문을 기억한다(TOFU, D-022).
    if let Some(id) = target.saved
        && target.fingerprint.is_none()
    {
        let state = app.state::<AppState>();
        let mut store = state.lock();
        if let Err(e) = store.update(|s| Ok(s.servers.confirm_fingerprint(id, conn.fingerprint)?)) {
            warn!("지문을 저장하지 못했습니다: {e}");
        }
    }

    info!(key, server = %server_name, peer = %conn.peer, "서버 연결");
    let sc = Arc::new(ServerConn {
        key: key.to_owned(),
        sender: conn.sender.clone(),
        connected_ip: signal::connected_ip(&conn),
        fingerprint: conn.fingerprint,
        server_name,
        can_publish,
        host_only,
        broadcasts: Mutex::new(broadcasts),
        pending: Mutex::new(HashMap::new()),
        next_req: AtomicU32::new(0),
        closed: AtomicBool::new(false),
        viewer_count: AtomicU32::new(0),
    });

    let weak = Arc::downgrade(&sc);
    let app = app.clone();
    let key = key.to_owned();
    let mut receiver = conn.receiver;
    tauri::async_runtime::spawn(async move {
        let mut reason = "서버와 연결이 끊어졌습니다".to_string();
        while let Some(msg) = receiver.recv().await {
            let Some(sc) = weak.upgrade() else { break };
            handle_server_msg(&app, &sc, msg, &mut reason);
        }
        if let Some(sc) = weak.upgrade() {
            sc.closed.store(true, Ordering::Relaxed);
            sc.pending.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
        debug!(key, "서버 연결 종료: {reason}");
        let _ = app.emit("server-closed", ClosedEvent { key: &key, reason });
    });
    Ok(sc)
}

fn deliver(sc: &ServerConn, req: u32, msg: ServerMsg) {
    if let Some(tx) = sc
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&req)
    {
        let _ = tx.send(msg);
    }
}

fn handle_server_msg(app: &AppHandle, sc: &ServerConn, msg: ServerMsg, reason: &mut String) {
    let key = sc.key.as_str();
    match msg {
        ServerMsg::Published { req, .. } | ServerMsg::Watching { req, .. } => deliver(sc, req, msg),
        ServerMsg::Error { req: Some(req), .. } => deliver(sc, req, msg),
        ServerMsg::Error {
            req: None, message, ..
        } => {
            warn!(key, "서버 오류: {message}");
        }
        ServerMsg::Broadcasts { broadcasts } => {
            *sc.broadcasts.lock().unwrap_or_else(|e| e.into_inner()) = broadcasts;
            let _ = app.emit("server-changed", KeyEvent { key });
        }
        ServerMsg::BroadcastEnded { broadcaster } => {
            let _ = app.emit(
                "broadcast-ended",
                BroadcastEndedEvent {
                    key,
                    broadcaster_id: broadcaster.to_string(),
                },
            );
        }
        ServerMsg::ViewerCount { count } => {
            sc.viewer_count.store(count, Ordering::Relaxed);
            let _ = app.emit("viewer-count", ViewerCountEvent { key, count });
        }
        ServerMsg::Closing { reason: r } => *reason = r,
        ServerMsg::Welcome { .. } | ServerMsg::Pong { .. } => {}
    }
}

/// 방송자 ID 문자열을 읽는다.
pub fn parse_broadcaster(id: &str) -> CmdResult<InstallId> {
    id.parse()
        .map_err(|_| CmdError::new("방송자를 찾을 수 없습니다"))
}
