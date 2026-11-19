//! 방송의 네트워크 쪽 로직(Tauri·캡처 없이 부를 수 있다): 송출 연결, 방송 정보 갱신, 끝내기 순서.
//!
//! 방송 명령(`broadcast.rs`)과 통합 시험이 같은 함수를 쓴다.

use std::sync::{Arc, Mutex};

use dongsi_core::proto::{ClientMsg, PublishInfo, ServerMsg};
use dongsi_net::publish::{PendingPublisher, Publisher};

use crate::embedded::Embedded;
use crate::error::{CmdError, CmdResult};
use crate::hub::{Hub, ServerConn};

/// 서버에 송출 연결을 만든다: `Publish` 요청 → answer → [`Publisher`].
///
/// 방송할 수 없는 접속(내장 서버에서 호스트가 아닌 연결 등)이면 `Publish`를 보내지 않고 실패한다.
pub async fn connect_publisher(
    conn: &ServerConn,
    info: PublishInfo,
    password: Option<String>,
) -> CmdResult<Arc<Publisher>> {
    if !conn.can_publish {
        return Err(CmdError::new("이 서버에서는 방송할 수 없습니다"));
    }
    let pending = PendingPublisher::new(conn.connected_ip, info.has_audio)
        .map_err(|e| CmdError::new(e.to_string()))?;
    let offer = pending.offer.clone();
    let reply = conn
        .request(|req| ClientMsg::Publish {
            req,
            offer,
            info,
            password,
        })
        .await?;
    let answer = match reply {
        ServerMsg::Published { answer, .. } => answer,
        ServerMsg::Error { message, .. } => return Err(CmdError::new(message)),
        _ => return Err(CmdError::new("서버 응답이 올바르지 않습니다")),
    };
    let publisher = pending
        .start(&answer)
        .map_err(|e| CmdError::new(e.to_string()))?;
    Ok(Arc::new(publisher))
}

/// 방송 정보를 `info`로 바꾸고, 달라졌으면 서버에 `UpdatePublish`를 보낸다. 달라졌는지를 돌려준다.
pub fn apply_publish_info(
    conn: &ServerConn,
    current: &Mutex<PublishInfo>,
    info: PublishInfo,
) -> CmdResult<bool> {
    let changed = {
        let mut current = current.lock().unwrap_or_else(|e| e.into_inner());
        let changed = *current != info;
        *current = info;
        changed
    };
    if changed {
        conn.send(ClientMsg::UpdatePublish { info })?;
    }
    Ok(changed)
}

/// 방송을 끝낸다. 순서:
/// 1. 서버에 `Unpublish`
/// 2. `stop_media`(캡처·인코딩 정지 등 앱 쪽 정리)를 실행하고 송출 연결을 닫는다(`publisher`를 놓는다)
/// 3. 내장 서버면: Hub의 `host` 연결을 닫고(`방송을 끝냈습니다`) 대상을 지운 뒤, 서버를 닫고 UPnP 매핑을 지운다
///
/// `publisher`는 마지막 참조여야 송출 스레드가 끝난다(`stop_media`가 다른 참조를 놓는다).
pub async fn stop_publishing(
    hub: &Hub,
    conn: &ServerConn,
    publisher: Arc<Publisher>,
    embedded: Option<Embedded>,
    stop_media: impl FnOnce() + Send + 'static,
) {
    let _ = conn.send(ClientMsg::Unpublish);
    let _ = tokio::task::spawn_blocking(move || {
        stop_media();
        drop(publisher);
    })
    .await;
    if let Some(embedded) = embedded {
        embedded.teardown(hub).await;
    }
}
