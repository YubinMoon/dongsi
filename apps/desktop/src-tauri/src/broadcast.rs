//! 방송: 캡처·인코딩(dongsi-media) → WebRTC 송출(dongsi-net) → 서버.
//!
//! 호스팅 방식(D-017, D-018):
//! - `server`: 추가한 서버(전용 서버)에 송출한다.
//! - `lan`: 앱에 내장된 서버를 열고 거기에 송출한다. 같은 LAN·tailnet에서 감지된다.
//! - `direct`: `lan`과 같고, UPnP로 공유기 포트를 열어 인터넷에서도 접속하게 한다.
//!
//! 내장 서버는 호스트 토큰을 보낸 이 앱만 방송할 수 있고, 방송을 끝내면 닫힌다.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dongsi_core::proto::{ClientMsg, MAX_PASSWORD_CHARS, PublishInfo, ServerMsg};
use dongsi_core::{
    DEFAULT_PORT, Host, InstallId, Invite, QualityPreset, ServerAddr, VideoCodec, VideoSettings,
};
use dongsi_media::{
    AudioPipeline, AudioSource, AudioStats, EncodedAudio, EncodedVideo, SourceId, SourceInfo,
    SourceKind, VideoPipeline, VideoStats,
};
use dongsi_net::ifaces::{IfaceKind, ipv4_interfaces, primary_lan_ip};
use dongsi_net::publish::{PendingPublisher, PublishStats, Publisher};
use dongsi_net::tls::Identity;
use dongsi_net::upnp::{self, UpnpMapping};
use dongsi_server::{RunningServer, ServerConfig};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tracing::{info, warn};

use crate::commands::AppState;
use crate::error::{CmdError, CmdResult};
use crate::hub::{Hub, ServerConn, Target};

/// 내장 서버의 허브 키.
pub const HOST_KEY: &str = "host";
const OPUS_KBPS: u32 = 128;
const MAX_INVITE_ADDRS: usize = 6;
/// 내장 서버: 방송 하나, 시청자 최대 10명(D-007).
const EMBEDDED_MAX_VIEWERS: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostingKind {
    Server,
    Lan,
    Direct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioMode {
    /// 시스템 소리에서 Discord만 뺀다(기본, D-004).
    SystemExceptDiscord,
    /// 캡처하는 창의 프로세스 소리만.
    App,
    System,
    None,
    /// A/V 동기 측정용(정초마다 삐 소리). 테스트 패턴과 같이 쓴다.
    TestTone,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartOptions {
    pub hosting: HostingKind,
    /// `hosting == server`일 때 서버 ID.
    pub server_id: Option<String>,
    pub source: String,
    pub settings: VideoSettings,
    pub audio: AudioMode,
    pub password: Option<String>,
}

struct Embedded {
    server: RunningServer,
    upnp: Option<Arc<UpnpMapping>>,
    renew: Option<tauri::async_runtime::JoinHandle<()>>,
    upnp_error: Option<String>,
}

struct Active {
    key: String,
    broadcaster_id: InstallId,
    hosting: HostingKind,
    conn: Arc<ServerConn>,
    server_name: String,
    embedded: Option<Embedded>,
    publisher: Arc<Publisher>,
    video: Arc<VideoPipeline>,
    audio: Option<AudioPipeline>,
    info: PublishInfo,
    source: SourceInfo,
    audio_mode: AudioMode,
    invite: String,
    invite_without_password: Option<String>,
    started_at: u64,
    stop_flag: Arc<AtomicBool>,
    monitor: Option<JoinHandle<()>>,
}

#[derive(Default)]
pub struct Broadcaster {
    active: tokio::sync::Mutex<Option<Active>>,
}

// ---------- 화면에 보내는 구조체 ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceView {
    id: String,
    kind: SourceKind,
    name: String,
    width: u32,
    height: u32,
    primary: bool,
    process_name: Option<String>,
}

impl From<&SourceInfo> for SourceView {
    fn from(s: &SourceInfo) -> Self {
        Self {
            id: s.id.to_string(),
            kind: s.kind,
            name: s.name.clone(),
            width: s.width,
            height: s.height,
            primary: s.primary,
            process_name: s.process_name.clone(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncoderView {
    codec: VideoCodec,
    name: String,
    hardware: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpnpView {
    external: Option<String>,
    /// 외부 IP가 사설 대역(이중 NAT·CGNAT)이라 인터넷에서 접속하지 못할 수 있다.
    double_nat: bool,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BroadcastStatus {
    hosting: HostingKind,
    key: String,
    /// 내 설치 ID(시청 화면 주소에 쓴다).
    broadcaster_id: String,
    server_name: String,
    source_name: String,
    audio: AudioMode,
    settings: VideoSettings,
    width: u32,
    height: u32,
    locked: bool,
    viewers: u32,
    started_at: u64,
    invite: String,
    invite_without_password: Option<String>,
    port: Option<u16>,
    upnp: Option<UpnpView>,
    publish: PublishStats,
    video: VideoStats,
    audio_stats: Option<AudioStats>,
    video_error: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoppedEvent {
    reason: String,
}

fn media_err(e: dongsi_media::MediaError) -> CmdError {
    CmdError::new(e.to_string())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetView {
    id: QualityPreset,
    label: &'static str,
    width: u32,
    height: u32,
    fps: u32,
    /// 코덱별 기본 비트레이트(kbps)
    bitrates: std::collections::BTreeMap<&'static str, u32>,
}

// ---------- 명령 ----------

/// 화질 프리셋(코어의 [`QualityPreset`]).
#[tauri::command]
pub fn quality_presets() -> Vec<PresetView> {
    QualityPreset::ALL
        .iter()
        .map(|&p| {
            let (width, height, fps) = p.dimensions();
            PresetView {
                id: p,
                label: p.label(),
                width,
                height,
                fps,
                bitrates: [
                    VideoCodec::H264,
                    VideoCodec::H265,
                    VideoCodec::Vp8,
                    VideoCodec::Vp9,
                    VideoCodec::Av1,
                ]
                .into_iter()
                .map(|c| (c.as_str(), p.default_bitrate_kbps(c)))
                .collect(),
            }
        })
        .collect()
}

/// 캡처할 수 있는 화면·창 목록. 이 앱의 창은 뺀다.
#[tauri::command]
pub async fn list_capture_sources() -> CmdResult<Vec<SourceView>> {
    let list = tauri::async_runtime::spawn_blocking(dongsi_media::list_sources)
        .await
        .map_err(|e| CmdError::new(e.to_string()))?
        .map_err(media_err)?;
    let me = std::process::id();
    Ok(list
        .iter()
        .filter(|s| s.process_id != Some(me))
        .filter(|s| cfg!(debug_assertions) || s.kind != SourceKind::Test)
        .map(SourceView::from)
        .collect())
}

/// 쓸 수 있는 비디오 인코더(코덱마다 가장 좋은 것). 처음에는 몇 초 걸린다.
#[tauri::command]
pub async fn available_encoders() -> CmdResult<Vec<EncoderView>> {
    let list = tauri::async_runtime::spawn_blocking(dongsi_media::available_encoders)
        .await
        .map_err(|e| CmdError::new(e.to_string()))?;
    Ok(list
        .into_iter()
        .map(|e| EncoderView {
            codec: e.codec,
            name: e.name,
            hardware: e.hardware,
        })
        .collect())
}

#[tauri::command]
pub async fn start_broadcast(
    app: AppHandle,
    hub: State<'_, Hub>,
    broadcaster: State<'_, Broadcaster>,
    options: StartOptions,
) -> CmdResult<BroadcastStatus> {
    let mut slot = broadcaster.active.lock().await;
    if slot.is_some() {
        return Err(CmdError::new("이미 방송 중입니다"));
    }
    let password = options
        .password
        .clone()
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty());
    if password
        .as_ref()
        .is_some_and(|p| p.chars().count() > MAX_PASSWORD_CHARS)
    {
        return Err(CmdError::new(format!(
            "비밀번호가 너무 깁니다(최대 {MAX_PASSWORD_CHARS}자)"
        )));
    }
    let source_id: SourceId = options
        .source
        .parse()
        .map_err(|_| CmdError::new("캡처 대상을 찾을 수 없습니다"))?;
    let source = find_source(&source_id).await?;
    let settings = options.settings.clamped();

    // 1) 캡처·인코딩을 먼저 연다(실패하면 방송을 알리지 않는다).
    let publisher_slot: Arc<Mutex<Option<Arc<Publisher>>>> = Arc::default();
    let video = {
        let slot = publisher_slot.clone();
        let src = source_id.clone();
        tauri::async_runtime::spawn_blocking(move || {
            VideoPipeline::start(
                src,
                settings,
                Box::new(move |frame: EncodedVideo| {
                    if let Some(p) = slot.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                        p.send_video(frame.codec, frame.data, frame.capture);
                    }
                }),
            )
        })
        .await
        .map_err(|e| CmdError::new(e.to_string()))?
        .map_err(media_err)?
    };
    let video = Arc::new(video);

    let audio_source = match options.audio {
        AudioMode::SystemExceptDiscord => Some(AudioSource::SystemExceptDiscord),
        AudioMode::System => Some(AudioSource::System),
        AudioMode::App => match source.process_id {
            Some(pid) => Some(AudioSource::ProcessTree(pid)),
            None => Some(AudioSource::SystemExceptDiscord),
        },
        AudioMode::TestTone => Some(AudioSource::TestTone),
        AudioMode::None => None,
    };
    let audio = match audio_source {
        Some(src) => {
            let slot = publisher_slot.clone();
            let started = tauri::async_runtime::spawn_blocking(move || {
                AudioPipeline::start(
                    src,
                    OPUS_KBPS,
                    Box::new(move |packet: EncodedAudio| {
                        if let Some(p) = slot.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                            p.send_audio(packet.data, packet.capture, packet.sample_index);
                        }
                    }),
                )
            })
            .await
            .map_err(|e| CmdError::new(e.to_string()))?;
            match started {
                Ok(a) => Some(a),
                Err(e) => {
                    warn!("오디오 캡처를 시작하지 못했습니다: {e}");
                    None
                }
            }
        }
        None => None,
    };

    let (width, height) = settings.output_size(source.width, source.height);
    let info = PublishInfo {
        codec: settings.codec,
        width,
        height,
        fps: settings.fps,
        has_audio: audio.is_some(),
    };

    // 2) 서버 준비
    let (install_id, display_name) = {
        let state = app.state::<AppState>();
        let store = state.lock();
        (store.state().install_id, store.state().display_name.clone())
    };
    let (key, embedded) = match options.hosting {
        HostingKind::Server => {
            let key = options
                .server_id
                .clone()
                .ok_or_else(|| CmdError::new("방송할 서버를 골라 주세요"))?;
            (key, None)
        }
        HostingKind::Lan | HostingKind::Direct => {
            let embedded = start_embedded(&app, &hub, &display_name, options.hosting).await?;
            (HOST_KEY.to_owned(), Some(embedded))
        }
    };

    let result = async {
        let conn = hub.connect(&app, &key).await?;
        if !conn.can_publish {
            return Err(CmdError::new("이 서버에서는 방송할 수 없습니다"));
        }
        // 3) 송출 연결
        let pending = PendingPublisher::new(conn.connected_ip, info.has_audio)
            .map_err(|e| CmdError::new(e.to_string()))?;
        let offer = pending.offer.clone();
        let reply = conn
            .request(|req| ClientMsg::Publish {
                req,
                offer,
                info,
                password: password.clone(),
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
        Ok((conn, Arc::new(publisher)))
    }
    .await;

    let (conn, publisher) = match result {
        Ok(x) => x,
        Err(e) => {
            if let Some(emb) = embedded {
                hub.disconnect(HOST_KEY).await;
                shutdown_embedded(emb).await;
            }
            drop_pipelines(video, audio).await;
            return Err(e);
        }
    };
    *publisher_slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(publisher.clone());

    // 4) 초대 코드
    let (invite, invite_without_password) = make_invite(
        &app,
        &key,
        &conn,
        embedded.as_ref(),
        install_id,
        password.as_deref(),
    )?;

    // 5) 감시 스레드: 키프레임 요청 전달, 송출·캡처가 끝나면 방송 종료
    let stop_flag = Arc::new(AtomicBool::new(false));
    let monitor = {
        let publisher = publisher.clone();
        let video = video.clone();
        let stop_flag = stop_flag.clone();
        let app = app.clone();
        std::thread::Builder::new()
            .name("dongsi-broadcast".into())
            .spawn(move || monitor_loop(app, publisher, video, stop_flag))
            .map_err(|e| CmdError::new(e.to_string()))?
    };

    info!(key, hosting = ?options.hosting, source = %source.name, ?settings, "방송 시작");
    *slot = Some(Active {
        key,
        broadcaster_id: install_id,
        hosting: options.hosting,
        server_name: conn.server_name.clone(),
        conn,
        embedded,
        publisher,
        video,
        audio,
        info,
        source,
        audio_mode: options.audio,
        invite,
        invite_without_password,
        started_at: unix_now(),
        stop_flag,
        monitor: Some(monitor),
    });
    Ok(status_of(slot.as_ref().expect("방금 넣었다")))
}

fn monitor_loop(
    app: AppHandle,
    publisher: Arc<Publisher>,
    video: Arc<VideoPipeline>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Relaxed) {
        if publisher.take_keyframe_request() {
            video.request_keyframe();
        }
        let reason = if publisher.is_closed() {
            publisher.stats().closed
        } else if !video.is_running() {
            Some(
                video
                    .last_error()
                    .unwrap_or_else(|| "캡처가 멈췄습니다".into()),
            )
        } else {
            None
        };
        if let Some(reason) = reason {
            warn!("방송이 끝났습니다: {reason}");
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move {
                let broadcaster = app2.state::<Broadcaster>();
                let hub = app2.state::<Hub>();
                stop_active(&broadcaster, &hub).await;
                let _ = app2.emit("broadcast-stopped", StoppedEvent { reason });
            });
            return;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

async fn find_source(id: &SourceId) -> CmdResult<SourceInfo> {
    let list = tauri::async_runtime::spawn_blocking(dongsi_media::list_sources)
        .await
        .map_err(|e| CmdError::new(e.to_string()))?
        .map_err(media_err)?;
    list.into_iter()
        .find(|s| &s.id == id)
        .ok_or_else(|| CmdError::new("캡처 대상을 찾을 수 없습니다. 목록을 새로 고쳐 주세요."))
}

async fn start_embedded(
    app: &AppHandle,
    hub: &Hub,
    display_name: &str,
    hosting: HostingKind,
) -> CmdResult<Embedded> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| CmdError::new(e.to_string()))?
        .join("host");
    std::fs::create_dir_all(&dir).map_err(|e| CmdError::new(e.to_string()))?;
    let identity = Identity::load_or_generate(&dir).map_err(|e| CmdError::new(e.to_string()))?;
    let token = InstallId::generate().to_string();
    let server = dongsi_server::start(
        ServerConfig {
            name: display_name.to_owned(),
            port: DEFAULT_PORT,
            fallback_port: true,
            host_token: Some(token.clone()),
            max_broadcasts: 1,
            max_viewers_per_broadcast: EMBEDDED_MAX_VIEWERS,
            discovery: true,
            ..Default::default()
        },
        &identity,
    )
    .await
    .map_err(|e| CmdError::new(format!("내장 서버를 열지 못했습니다: {e}")))?;
    let port = server.port();
    hub.set_target(
        HOST_KEY,
        Target {
            addrs: vec![ServerAddr::new(
                Host::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                port,
            )],
            fingerprint: Some(server.fingerprint()),
            saved: None,
            host_token: Some(token),
        },
    );

    let mut embedded = Embedded {
        server,
        upnp: None,
        renew: None,
        upnp_error: None,
    };
    if hosting == HostingKind::Direct {
        match primary_lan_ip() {
            None => embedded.upnp_error = Some("LAN 주소를 찾지 못했습니다".into()),
            Some(lan) => match upnp::map_ports(lan, port).await {
                Ok(mapping) => {
                    if mapping.external_port != port {
                        embedded
                            .server
                            .set_external_port(Some(mapping.external_port));
                    }
                    let mapping = Arc::new(mapping);
                    let renew_mapping = mapping.clone();
                    embedded.renew = Some(tauri::async_runtime::spawn(async move {
                        loop {
                            tokio::time::sleep(UpnpMapping::renew_interval()).await;
                            if let Err(e) = renew_mapping.renew().await {
                                warn!("{e}");
                            }
                        }
                    }));
                    embedded.upnp = Some(mapping);
                }
                Err(e) => {
                    warn!("UPnP 실패: {e}");
                    embedded.upnp_error = Some(format!(
                        "{e}. 공유기에서 TCP·UDP {port} 포트를 이 PC({lan})로 직접 열어 주세요."
                    ));
                }
            },
        }
    }
    Ok(embedded)
}

async fn shutdown_embedded(embedded: Embedded) {
    if let Some(task) = embedded.renew {
        task.abort();
    }
    if let Some(mapping) = embedded.upnp {
        mapping.remove().await;
    }
    embedded.server.shutdown().await;
}

async fn drop_pipelines(video: Arc<VideoPipeline>, audio: Option<AudioPipeline>) {
    let _ = tauri::async_runtime::spawn_blocking(move || {
        if let Some(a) = audio {
            a.stop();
        }
        drop(video);
    })
    .await;
}

/// 이 PC의 Tailscale·LAN 주소(초대 코드용).
fn local_addrs(port: u16) -> Vec<ServerAddr> {
    let mut ifaces = ipv4_interfaces();
    ifaces.sort_by_key(|i| match i.kind {
        IfaceKind::Lan => 0,
        IfaceKind::Tailscale => 1,
        _ => 2,
    });
    ifaces
        .into_iter()
        .filter(|i| matches!(i.kind, IfaceKind::Lan | IfaceKind::Tailscale))
        .map(|i| ServerAddr::new(Host::Ip(IpAddr::V4(i.ip)), port))
        .collect()
}

fn make_invite(
    app: &AppHandle,
    key: &str,
    conn: &ServerConn,
    embedded: Option<&Embedded>,
    install_id: InstallId,
    password: Option<&str>,
) -> CmdResult<(String, Option<String>)> {
    let (mut addrs, name) = match embedded {
        Some(emb) => {
            let port = emb.server.port();
            let mut addrs = Vec::new();
            if let Some(m) = &emb.upnp {
                addrs.push(ServerAddr::new(Host::Ip(m.external_ip), m.external_port));
            }
            addrs.extend(local_addrs(port));
            (addrs, None)
        }
        None => {
            let id = key
                .parse()
                .map_err(|_| CmdError::new("서버를 찾을 수 없습니다"))?;
            let state = app.state::<AppState>();
            let store = state.lock();
            let server = store
                .state()
                .servers
                .get(id)
                .ok_or_else(|| CmdError::new("서버를 찾을 수 없습니다"))?;
            (server.addrs.clone(), Some(server.name.clone()))
        }
    };
    if addrs.is_empty() {
        addrs.push(ServerAddr::new(
            Host::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            embedded.map_or(DEFAULT_PORT, |e| e.server.port()),
        ));
    }
    addrs.truncate(MAX_INVITE_ADDRS);
    let invite = Invite::for_broadcast(
        addrs,
        conn.fingerprint,
        name.or_else(|| Some(conn.server_name.clone())),
        install_id,
        password.map(str::to_owned),
    )
    .map_err(|e| CmdError::new(format!("초대 코드를 만들지 못했습니다: {e}")))?;
    let without = password.map(|_| invite.without_password().encode());
    Ok((invite.encode(), without))
}

fn status_of(a: &Active) -> BroadcastStatus {
    let video = a.video.stats();
    BroadcastStatus {
        hosting: a.hosting,
        key: a.key.clone(),
        broadcaster_id: a.broadcaster_id.to_string(),
        server_name: a.server_name.clone(),
        source_name: a.source.name.clone(),
        audio: a.audio_mode,
        settings: a.video.settings(),
        width: if video.width > 0 {
            video.width
        } else {
            a.info.width
        },
        height: if video.height > 0 {
            video.height
        } else {
            a.info.height
        },
        locked: a.invite_without_password.is_some(),
        viewers: a.conn.viewer_count.load(Ordering::Relaxed),
        started_at: a.started_at,
        invite: a.invite.clone(),
        invite_without_password: a.invite_without_password.clone(),
        port: a.embedded.as_ref().map(|e| e.server.port()),
        upnp: a.embedded.as_ref().and_then(|e| {
            if a.hosting != HostingKind::Direct {
                return None;
            }
            Some(UpnpView {
                external: e
                    .upnp
                    .as_ref()
                    .map(|m| SocketAddr::new(m.external_ip, m.external_port).to_string()),
                double_nat: e
                    .upnp
                    .as_ref()
                    .is_some_and(|m| !upnp::is_public_ip(m.external_ip)),
                error: e.upnp_error.clone(),
            })
        }),
        publish: a.publisher.stats(),
        video,
        audio_stats: a.audio.as_ref().map(AudioPipeline::stats),
        video_error: a.video.last_error(),
    }
}

#[tauri::command]
pub async fn broadcast_status(
    broadcaster: State<'_, Broadcaster>,
) -> CmdResult<Option<BroadcastStatus>> {
    Ok(broadcaster.active.lock().await.as_ref().map(status_of))
}

/// 방송 중에 코덱·해상도·fps·비트레이트를 바꾼다.
#[tauri::command]
pub async fn update_broadcast(
    broadcaster: State<'_, Broadcaster>,
    settings: VideoSettings,
) -> CmdResult<BroadcastStatus> {
    let mut slot = broadcaster.active.lock().await;
    let active = slot
        .as_mut()
        .ok_or_else(|| CmdError::new("방송 중이 아닙니다"))?;
    let settings = settings.clamped();
    let video = active.video.clone();
    tauri::async_runtime::spawn_blocking(move || video.update(settings))
        .await
        .map_err(|e| CmdError::new(e.to_string()))?
        .map_err(media_err)?;
    let (width, height) = settings.output_size(active.source.width, active.source.height);
    let info = PublishInfo {
        codec: settings.codec,
        width,
        height,
        fps: settings.fps,
        has_audio: active.info.has_audio,
    };
    if info != active.info {
        active.info = info;
        active.conn.send(ClientMsg::UpdatePublish { info })?;
    }
    info!(?settings, "방송 설정 변경");
    Ok(status_of(active))
}

#[tauri::command]
pub async fn stop_broadcast(
    broadcaster: State<'_, Broadcaster>,
    hub: State<'_, Hub>,
) -> CmdResult<()> {
    stop_active(&broadcaster, &hub).await;
    Ok(())
}

async fn stop_active(broadcaster: &Broadcaster, hub: &Hub) {
    let Some(mut active) = broadcaster.active.lock().await.take() else {
        return;
    };
    info!(key = active.key, "방송 종료");
    active.stop_flag.store(true, Ordering::Relaxed);
    let _ = active.conn.send(ClientMsg::Unpublish);
    let monitor = active.monitor.take();
    let publisher = active.publisher;
    let video = active.video;
    let audio = active.audio;
    let _ = tauri::async_runtime::spawn_blocking(move || {
        if let Some(m) = monitor
            && m.thread().id() != std::thread::current().id()
        {
            let _ = m.join();
        }
        if let Some(a) = audio {
            a.stop();
        }
        drop(video);
        // 감시 스레드와 콜백이 놓으면 마지막 참조가 여기서 사라지며 송출 스레드가 끝난다.
        drop(publisher);
    })
    .await;
    if let Some(emb) = active.embedded {
        hub.disconnect(HOST_KEY).await;
        hub.remove_target(HOST_KEY);
        shutdown_embedded(emb).await;
    }
}

/// 앱을 닫을 때 방송을 정리한다(UPnP 매핑 제거 등).
pub fn shutdown_on_exit(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::block_on(async move {
        let broadcaster = app.state::<Broadcaster>();
        let hub = app.state::<Hub>();
        let _ = tokio::time::timeout(Duration::from_secs(3), stop_active(&broadcaster, &hub)).await;
    });
}
