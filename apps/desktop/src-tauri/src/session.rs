//! 방송의 네트워크 쪽 로직(Tauri·캡처 없이 부를 수 있다): 송출 연결, 방송 정보 갱신, 끝내기 순서.
//!
//! 방송 명령(`broadcast.rs`)과 통합 시험이 같은 함수를 쓴다.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dongsi_core::proto::{ClientMsg, PublishInfo, ServerMsg, encode_thumbnail};
use dongsi_net::publish::{PendingPublisher, Publisher};
use dongsi_net::rate::RateConfig;

use crate::embedded::Embedded;
use crate::error::{CmdError, CmdResult};
use crate::hub::{Hub, ServerConn};

/// 송출 오디오(Opus) 비트레이트(kbps).
pub const OPUS_KBPS: u32 = 128;

/// 서버에 송출 연결을 만든다: `Publish` 요청 → answer → [`Publisher`].
///
/// `max_video_kbps`는 방송 설정의 비디오 비트레이트다. 송출은 네트워크 추정값에 맞춰 이보다 낮은 목표를 정할 수
/// 있다(D-081, [`Publisher::video_target_kbps`]). 비디오는 rid 없는 트랙 하나다(D-104). 방송할 수 없는 접속(내장
/// 서버에서 호스트가 아닌 연결 등)이면 `Publish`를 보내지 않고 실패한다.
pub async fn connect_publisher(
    conn: &ServerConn,
    info: PublishInfo,
    password: Option<String>,
    max_video_kbps: u32,
) -> CmdResult<Arc<Publisher>> {
    if !conn.can_publish {
        return Err(CmdError::new("이 서버에서는 방송할 수 없습니다"));
    }
    let rate = RateConfig {
        max_video_kbps,
        audio_kbps: info.has_audio.then_some(OPUS_KBPS),
    };
    let pending =
        PendingPublisher::new(conn.connected_ip, rate).map_err(|e| CmdError::new(e.to_string()))?;
    let offer = pending.offer.clone();
    // 지난 방송을 끝내는 순간 오고 있던 `ViewerCount`는 [`stop_publishing`]이 0으로 되돌린 뒤에 도착할 수 있다.
    // `Publish`를 보내기 전에 한 번 더 비워 새 방송이 옛 값으로 시작하지 않게 한다.
    conn.viewer_count.store(0, Ordering::Relaxed);
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

// ---------- 방송 목록 썸네일(D-076) ----------

/// 썸네일 크기 한도(기기 픽셀). 방송 타일(폭 약 280~400 CSS px)에 화면 배율 1.25를 곱한 정도이고,
/// 방송 시작 화면의 캡처 대상 미리 보기(D-068)와 같다.
pub const THUMBNAIL_MAX: (u32, u32) = (480, 270);
/// 썸네일 JPEG 목표 크기. 서버 상한(`MAX_THUMBNAIL_BYTES` 64KB)보다 작게 둔다.
pub const THUMBNAIL_TARGET_BYTES: usize = 48 * 1024;
/// 썸네일을 올리는 간격(사용자 결정: 1분에 한 번).
pub const THUMBNAIL_INTERVAL: Duration = Duration::from_secs(60);
/// 그림을 못 만들었을 때 다시 해 보는 간격. 첫 그림(방송 시작 직후 첫 프레임)은 더 자주 본다.
const THUMBNAIL_RETRY: Duration = Duration::from_secs(5);
const FIRST_THUMBNAIL_RETRY: Duration = Duration::from_millis(500);
const UPLOADER_TICK: Duration = Duration::from_millis(100);

/// 썸네일 한 장을 서버에 올린다.
pub fn send_thumbnail(conn: &ServerConn, jpeg: &[u8]) -> CmdResult<()> {
    conn.send(ClientMsg::Thumbnail {
        jpeg: encode_thumbnail(jpeg),
    })
}

/// 썸네일 올리기 스레드를 띄운다. `grab`은 지금 송출 화면을 JPEG 한 장으로 만들어 돌려준다(블로킹, 못 만들면 `None`).
///
/// 방송을 시작하면 첫 그림이 나올 때까지 0.5초마다 해 보고, 올린 뒤에는 `interval`마다 올린다. 못 만들면 5초 뒤에
/// 다시 한다. `stop`이 서거나 연결이 닫히면 끝난다. 비밀번호 방송이면 부르지 않는다(사용자 결정: 비밀번호 방송은 보내지 않는다).
pub fn spawn_thumbnail_uploader(
    conn: Arc<ServerConn>,
    stop: Arc<AtomicBool>,
    interval: Duration,
    mut grab: impl FnMut() -> Option<Vec<u8>> + Send + 'static,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("dongsi-thumbnail".into())
        .spawn(move || {
            let running = || !stop.load(Ordering::Relaxed) && !conn.is_closed();
            let mut next = Instant::now();
            let mut sent = false;
            while running() {
                if Instant::now() >= next {
                    let shot = grab();
                    // 그림을 만드는 동안 방송이 끝났으면 보내지 않는다(Unpublish 뒤에 가면 서버가 NotFound로 답한다).
                    if !running() {
                        break;
                    }
                    next = match shot {
                        Some(jpeg) => match send_thumbnail(&conn, &jpeg) {
                            Ok(()) => {
                                tracing::debug!(bytes = jpeg.len(), "썸네일 올림");
                                sent = true;
                                Instant::now() + interval
                            }
                            Err(_) => break,
                        },
                        None if sent => Instant::now() + THUMBNAIL_RETRY,
                        None => Instant::now() + FIRST_THUMBNAIL_RETRY,
                    };
                }
                std::thread::sleep(UPLOADER_TICK);
            }
        })
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
/// 3. 연결의 시청자 수를 0으로 되돌린다. 전용 서버 연결은 방송 뒤에도 열려 있고 서버는 끝난 방송자에게 0을 보내지
///    않아서, 그대로 두면 같은 서버의 다음 방송이 옛 값으로 시작한다
/// 4. 내장 서버면: Hub의 `host` 연결을 닫고(`방송을 끝냈습니다`) 대상을 지운 뒤, 서버를 닫고 UPnP 매핑을 지운다
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
    conn.viewer_count.store(0, Ordering::Relaxed);
    if let Some(embedded) = embedded {
        embedded.teardown(hub).await;
    }
}
