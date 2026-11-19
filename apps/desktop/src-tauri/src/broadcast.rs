//! 방송: 캡처·인코딩(dongsi-media) → WebRTC 송출(dongsi-net) → 서버.
//!
//! 호스팅 방식(D-017, D-018):
//! - `server`: 추가한 서버(전용 서버)에 송출한다.
//! - `lan`: 앱에 내장된 서버를 열고 거기에 송출한다. 같은 LAN·tailnet에서 감지된다.
//! - `direct`: `lan`과 같고, UPnP로 공유기 포트를 열어 인터넷에서도 접속하게 한다.
//!
//! 내장 서버는 호스트 토큰을 보낸 이 앱만 방송할 수 있고, 방송을 끝내면 닫힌다.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dongsi_core::proto::{ClientMsg, MAX_PASSWORD_CHARS, PublishInfo};
use dongsi_core::{DEFAULT_PORT, InstallId, QualityPreset, VideoCodec, VideoSettings};
use dongsi_media::{
    AudioPipeline, AudioSource, AudioStats, EncodedAudio, EncodedVideo, SourceId, SourceInfo,
    SourceKind, VideoPipeline, VideoStats,
};
use dongsi_net::ifaces::{ipv4_interfaces, primary_lan_ip};
use dongsi_net::publish::{PublishStats, Publisher};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tracing::{info, warn};

use crate::commands::AppState;
pub use crate::embedded::HostingKind;
use crate::embedded::{
    Embedded, EmbeddedParams, UpnpMapper, UpnpView, broadcast_invite, embedded_broadcast_invite,
    start_embedded,
};
use crate::error::{CmdError, CmdResult};
use crate::hub::{HOST_KEY, Hub, REASON_LOST, REASON_USER, ServerConn};
use crate::paths::{AppEnv, AppPaths};
use crate::session::{apply_publish_info, connect_publisher, stop_publishing};

const OPUS_KBPS: u32 = 128;

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
    /// 서버에 알린 방송 정보. 감시 스레드가 실제 출력 크기에 맞춰 고친다.
    info: Arc<Mutex<PublishInfo>>,
    source: SourceInfo,
    audio_mode: AudioMode,
    invite: String,
    invite_without_password: Option<String>,
    started_at: u64,
    stop_flag: Arc<AtomicBool>,
    monitor: Option<JoinHandle<()>>,
}

impl Active {
    fn info(&self) -> PublishInfo {
        *self.info.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[derive(Default)]
pub struct Broadcaster {
    active: tokio::sync::Mutex<Option<Active>>,
    /// 방송 중이거나 시작하는 중인 서버의 키(전용 서버 ID 또는 `host`).
    /// `active`는 방송을 시작하는 동안 잠겨 있으므로, 연결 끊기가 막히지 않게 따로 둔다.
    busy_key: Mutex<Option<String>>,
}

impl Broadcaster {
    /// 그 키가 방송 중(시작하는 중 포함)이 아니면 연결을 끊는다. 맞으면 `broadcasting` 오류.
    /// 확인하는 동안 `busy_key`를 잡고 있어서, 방송이 막 시작되는 것과 겹쳐도 안전하다.
    pub fn disconnect_unless_broadcasting(&self, hub: &Hub, key: &str) -> CmdResult<()> {
        let busy = self.busy_key.lock().unwrap_or_else(|e| e.into_inner());
        ensure_not_broadcasting(busy.as_deref(), key)?;
        hub.disconnect(key, REASON_USER);
        Ok(())
    }

    /// 그 키가 방송 중(시작하는 중 포함)이 아니면 `remove`(서버 삭제)를 실행한다. 맞으면 실행하지 않고
    /// `broadcasting` 오류. 확인하는 동안 `busy_key`를 잡고 있어서 방송이 막 시작되는 것과 겹쳐도 안전하다.
    pub fn remove_unless_broadcasting(
        &self,
        key: &str,
        remove: impl FnOnce() -> CmdResult<()>,
    ) -> CmdResult<()> {
        let busy = self.busy_key.lock().unwrap_or_else(|e| e.into_inner());
        ensure_removable(busy.as_deref(), key)?;
        remove()
    }

    pub(crate) fn set_busy(&self, key: Option<String>) {
        *self.busy_key.lock().unwrap_or_else(|e| e.into_inner()) = key;
    }
}

/// 방송을 시작하는 도중 실패하면 `busy_key`를 비운다. 방송이 시작되면 `keep`으로 남긴다.
struct BusyGuard<'a> {
    broadcaster: &'a Broadcaster,
    keep: bool,
}

impl BusyGuard<'_> {
    fn keep(mut self) {
        self.keep = true;
    }
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        if !self.keep {
            self.broadcaster.set_busy(None);
        }
    }
}

/// 방송 중인 서버(`busy`)의 키와 같으면 끊지 못한다.
fn ensure_not_broadcasting(busy: Option<&str>, key: &str) -> CmdResult<()> {
    if busy == Some(key) {
        return Err(CmdError::with_code(
            "방송 중인 서버는 연결을 끊을 수 없습니다. 방송을 먼저 끝내 주세요.",
            "broadcasting",
        ));
    }
    Ok(())
}

/// 방송 중인 서버(`busy`)의 키와 같으면 지울 수 없다.
fn ensure_removable(busy: Option<&str>, key: &str) -> CmdResult<()> {
    if busy == Some(key) {
        return Err(CmdError::with_code(
            "방송 중인 서버는 삭제할 수 없습니다. 방송을 먼저 끝내 주세요.",
            "broadcasting",
        ));
    }
    Ok(())
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
    /// 내장 서버가 열려고 한 포트(`DONGSI_PORT` 또는 7884). 전용 서버 방송이면 `None`.
    requested_port: Option<u16>,
    /// 요청한 포트를 못 써서 다른 포트로 열었다.
    port_fallback: bool,
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
    start_impl(&app, &hub, &broadcaster, options).await
}

/// 개발용 시험 방송 옵션(`dev_start_test_broadcast`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevTestOptions {
    pub hosting: HostingKind,
    pub server_id: Option<String>,
    pub password: Option<String>,
    /// 기본은 가장 낮은 프리셋(720p 30fps).
    pub preset: Option<QualityPreset>,
    /// 기본은 H.264.
    pub codec: Option<VideoCodec>,
}

impl DevTestOptions {
    /// 테스트 패턴 + 테스트 톤으로 `start_broadcast`와 같은 옵션을 만든다.
    fn into_start_options(self) -> StartOptions {
        let preset = self.preset.unwrap_or(QualityPreset::ALL[0]);
        let codec = self.codec.unwrap_or(VideoCodec::H264);
        StartOptions {
            hosting: self.hosting,
            server_id: self.server_id,
            source: SourceId::TestPattern.to_string(),
            settings: preset.settings(codec),
            audio: AudioMode::TestTone,
            password: self.password,
        }
    }
}

/// 개발용: 테스트 패턴과 테스트 톤으로 방송을 시작한다(자동 시험용). 배포 빌드에서는 쓸 수 없다.
#[tauri::command]
pub async fn dev_start_test_broadcast(
    app: AppHandle,
    hub: State<'_, Hub>,
    broadcaster: State<'_, Broadcaster>,
    options: DevTestOptions,
) -> CmdResult<BroadcastStatus> {
    if !cfg!(debug_assertions) {
        return Err(CmdError::new("개발 빌드에서만 쓸 수 있습니다"));
    }
    start_impl(&app, &hub, &broadcaster, options.into_start_options()).await
}

async fn start_impl(
    app: &AppHandle,
    hub: &Hub,
    broadcaster: &Broadcaster,
    options: StartOptions,
) -> CmdResult<BroadcastStatus> {
    let mut slot = broadcaster.active.lock().await;
    if slot.is_some() {
        return Err(CmdError::new("이미 방송 중입니다"));
    }
    // 연결 끊기가 이 서버를 건드리지 않게 방송 키를 먼저 표시한다.
    let busy = BusyGuard {
        broadcaster,
        keep: false,
    };
    broadcaster.set_busy(match options.hosting {
        HostingKind::Server => options.server_id.clone(),
        HostingKind::Lan | HostingKind::Direct => Some(HOST_KEY.to_owned()),
    });
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
            let dir = app.state::<AppPaths>().host_dir.clone();
            let embedded = start_embedded(
                hub,
                EmbeddedParams {
                    dir: &dir,
                    name: &display_name,
                    hosting: options.hosting,
                    port: app.state::<AppEnv>().port,
                    lan_ip: primary_lan_ip(),
                },
                &UpnpMapper,
            )
            .await?;
            (HOST_KEY.to_owned(), Some(embedded))
        }
    };

    let result = async {
        let conn = hub.connect(&key).await?;
        // 3) 송출 연결
        let publisher = connect_publisher(&conn, info, password.clone()).await?;
        Ok::<_, CmdError>((conn, publisher))
    }
    .await;

    let (conn, publisher) = match result {
        Ok(x) => x,
        Err(e) => {
            if let Some(emb) = embedded {
                emb.teardown(hub).await;
            }
            drop_pipelines(video, audio).await;
            return Err(e);
        }
    };
    *publisher_slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(publisher.clone());

    // 4) 초대 코드
    let (invite, invite_without_password) = make_invite(
        app,
        &key,
        &conn,
        embedded.as_ref(),
        install_id,
        password.as_deref(),
    )?;

    // 5) 감시 스레드: 키프레임 요청 전달, 송출·캡처가 끝나면 방송 종료
    let stop_flag = Arc::new(AtomicBool::new(false));
    let info = Arc::new(Mutex::new(info));
    let monitor = {
        let watch = Monitor {
            app: app.clone(),
            publisher: publisher.clone(),
            video: video.clone(),
            conn: conn.clone(),
            info: info.clone(),
            stop: stop_flag.clone(),
        };
        std::thread::Builder::new()
            .name("dongsi-broadcast".into())
            .spawn(move || watch.run())
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
    busy.keep();
    Ok(status_of(slot.as_ref().expect("방금 넣었다")))
}

/// 방송 감시 스레드: 키프레임 요청 전달, 방송 정보의 크기 맞추기, 송출·캡처가 끝나면 방송 종료.
struct Monitor {
    app: AppHandle,
    publisher: Arc<Publisher>,
    video: Arc<VideoPipeline>,
    conn: Arc<ServerConn>,
    info: Arc<Mutex<PublishInfo>>,
    stop: Arc<AtomicBool>,
}

/// 연결이 닫혔는데 이유가 아직 없을 때 기다리는 틱 수(약 0.15초).
const CONN_REASON_WAIT_TICKS: u32 = 10;

/// 감시 스레드가 한 번 본 상태.
#[derive(Debug, Default)]
struct Observed {
    publisher_closed: bool,
    publisher_reason: Option<String>,
    capture_running: bool,
    capture_error: Option<String>,
    /// 서버 시그널링 연결이 닫혔다.
    conn_closed: bool,
    conn_reason: Option<String>,
}

/// 방송을 끝내야 하면 화면에 보일 이유.
///
/// 우선순위: 서버 연결 닫힘(서버에서 방송이 사라졌다. 서버가 알려 준 이유, 없으면 `REASON_LOST`)
/// → 송출 연결 닫힘 → 캡처 멈춤. 모두 정상이면 `None`.
fn end_reason(o: &Observed) -> Option<String> {
    if o.conn_closed {
        return Some(
            o.conn_reason
                .clone()
                .unwrap_or_else(|| REASON_LOST.to_owned()),
        );
    }
    if o.publisher_closed {
        return o.publisher_reason.clone();
    }
    if !o.capture_running {
        return Some(
            o.capture_error
                .clone()
                .unwrap_or_else(|| "캡처가 멈췄습니다".into()),
        );
    }
    None
}

impl Monitor {
    const TICK: Duration = Duration::from_millis(15);
    /// 크기 확인 간격(틱 수, 약 0.5초).
    const SIZE_CHECK_TICKS: u32 = 33;

    fn run(self) {
        let Monitor {
            app,
            publisher,
            video,
            conn,
            info,
            stop,
        } = self;
        let mut tick = 0u32;
        let mut conn_wait_ticks = 0u32;
        // 직전 확인에서 본 출력 크기. 두 번 연속 같아야 고친다(설정을 바꾼 직후의 옛 크기 무시).
        let mut seen: Option<(u32, u32)> = None;
        while !stop.load(Ordering::Relaxed) {
            if publisher.take_keyframe_request() {
                video.request_keyframe();
            }
            tick = tick.wrapping_add(1);
            if tick.is_multiple_of(Self::SIZE_CHECK_TICKS) {
                sync_output_size(&video, &conn, &info, &mut seen);
            }
            // 연결이 닫혔는데 이유를 아직 못 받았으면 잠깐(약 0.15초) 기다린다(서버가 알려 준 이유가 더 정확하다).
            let conn_reason = conn.close_reason();
            let conn_closed = conn.is_closed();
            if conn_closed && conn_reason.is_none() {
                conn_wait_ticks += 1;
            }
            let observed = Observed {
                publisher_closed: publisher.is_closed(),
                publisher_reason: publisher.stats().closed,
                capture_running: video.is_running(),
                capture_error: video.last_error(),
                conn_closed: conn_closed
                    && (conn_reason.is_some() || conn_wait_ticks >= CONN_REASON_WAIT_TICKS),
                conn_reason,
            };
            if let Some(reason) = end_reason(&observed) {
                // 사용자가 방송을 끝내는 중이면(감시 스레드가 합류하기 전) 다른 이유를 알리지 않는다.
                if stop.load(Ordering::Relaxed) {
                    return;
                }
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
            std::thread::sleep(Self::TICK);
        }
    }
}

/// 인코더의 실제 출력 크기가 서버에 알린 값과 다르면 고쳐 알린다.
/// 창 크기가 바뀌거나, 원본 크기를 미리 알 수 없는 소스(테스트 패턴 등)에서 생긴다.
fn sync_output_size(
    video: &VideoPipeline,
    conn: &ServerConn,
    info: &Mutex<PublishInfo>,
    seen: &mut Option<(u32, u32)>,
) {
    let stats = video.stats();
    if stats.width == 0 || stats.height == 0 {
        return;
    }
    let size = (stats.width, stats.height);
    let stable = *seen == Some(size);
    *seen = Some(size);
    let mut info = info.lock().unwrap_or_else(|e| e.into_inner());
    if !stable || stats.codec != Some(info.codec) || (info.width, info.height) == size {
        return;
    }
    info.width = size.0;
    info.height = size.1;
    let updated = *info;
    drop(info);
    info!(
        width = size.0,
        height = size.1,
        "방송 정보의 출력 크기를 고침"
    );
    let _ = conn.send(ClientMsg::UpdatePublish { info: updated });
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

async fn drop_pipelines(video: Arc<VideoPipeline>, audio: Option<AudioPipeline>) {
    let _ = tauri::async_runtime::spawn_blocking(move || {
        if let Some(a) = audio {
            a.stop();
        }
        drop(video);
    })
    .await;
}

/// 방송 초대 코드(비밀번호 있음, 비밀번호를 뺀 것). 주소 후보는 내장 서버면 외부 주소·LAN·Tailscale,
/// 전용 서버면 저장한 서버의 주소다.
fn make_invite(
    app: &AppHandle,
    key: &str,
    conn: &ServerConn,
    embedded: Option<&Embedded>,
    install_id: InstallId,
    password: Option<&str>,
) -> CmdResult<(String, Option<String>)> {
    if let Some(emb) = embedded {
        return embedded_broadcast_invite(
            emb,
            &ipv4_interfaces(),
            &conn.server_name,
            install_id,
            password,
        );
    }
    let id = key
        .parse()
        .map_err(|_| CmdError::new("서버를 찾을 수 없습니다"))?;
    let (addrs, name) = {
        let state = app.state::<AppState>();
        let store = state.lock();
        let server = store
            .state()
            .servers
            .get(id)
            .ok_or_else(|| CmdError::new("서버를 찾을 수 없습니다"))?;
        (server.addrs.clone(), server.name.clone())
    };
    broadcast_invite(
        addrs,
        DEFAULT_PORT,
        conn.fingerprint,
        Some(name),
        install_id,
        password,
    )
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
            a.info().width
        },
        height: if video.height > 0 {
            video.height
        } else {
            a.info().height
        },
        locked: a.invite_without_password.is_some(),
        viewers: a.conn.viewer_count.load(Ordering::Relaxed),
        started_at: a.started_at,
        invite: a.invite.clone(),
        invite_without_password: a.invite_without_password.clone(),
        port: a.embedded.as_ref().map(Embedded::port),
        requested_port: a.embedded.as_ref().map(Embedded::requested_port),
        port_fallback: a.embedded.as_ref().is_some_and(Embedded::port_fallback),
        upnp: a.embedded.as_ref().and_then(Embedded::upnp_view),
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
        has_audio: active.info().has_audio,
    };
    apply_publish_info(&active.conn, &active.info, info)?;
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
    broadcaster.set_busy(None);
    info!(key = active.key, "방송 종료");
    active.stop_flag.store(true, Ordering::Relaxed);
    let monitor = active.monitor.take();
    let video = active.video;
    let audio = active.audio;
    // 캡처·인코딩 정지는 앱 쪽 일이다. 네트워크 쪽 끝내기 순서는 `stop_publishing`이 맡는다.
    let stop_media = move || {
        if let Some(m) = monitor
            && m.thread().id() != std::thread::current().id()
        {
            let _ = m.join();
        }
        if let Some(a) = audio {
            a.stop();
        }
        // 감시 스레드와 콜백이 놓으면 송출 참조가 하나만 남아, `stop_publishing`이 놓을 때 송출 스레드가 끝난다.
        drop(video);
    };
    stop_publishing(
        hub,
        &active.conn,
        active.publisher,
        active.embedded,
        stop_media,
    )
    .await;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> Observed {
        Observed {
            capture_running: true,
            ..Default::default()
        }
    }

    #[test]
    fn end_reason_is_none_while_everything_is_fine() {
        assert_eq!(end_reason(&healthy()), None);
        // 연결 이유만 있고 닫히지 않았으면 무시한다.
        let o = Observed {
            conn_reason: Some("x".into()),
            ..healthy()
        };
        assert_eq!(end_reason(&o), None);
    }

    #[test]
    fn end_reason_for_a_closed_connection_uses_the_server_reason_or_the_default() {
        let o = Observed {
            conn_closed: true,
            conn_reason: Some("서버가 닫힙니다".into()),
            ..healthy()
        };
        assert_eq!(end_reason(&o).as_deref(), Some("서버가 닫힙니다"));
        let o = Observed {
            conn_closed: true,
            ..healthy()
        };
        assert_eq!(
            end_reason(&o).as_deref(),
            Some("서버와 연결이 끊어졌습니다")
        );
    }

    #[test]
    fn end_reason_for_a_closed_publisher_and_a_stopped_capture() {
        let o = Observed {
            publisher_closed: true,
            publisher_reason: Some("시간 초과".into()),
            ..healthy()
        };
        assert_eq!(end_reason(&o).as_deref(), Some("시간 초과"));
        let o = Observed {
            capture_running: false,
            capture_error: Some("창이 닫혔습니다".into()),
            ..Default::default()
        };
        assert_eq!(end_reason(&o).as_deref(), Some("창이 닫혔습니다"));
        let o = Observed::default();
        assert_eq!(end_reason(&o).as_deref(), Some("캡처가 멈췄습니다"));
    }

    #[test]
    fn end_reason_priority_is_connection_then_publisher_then_capture() {
        let all = Observed {
            publisher_closed: true,
            publisher_reason: Some("송출 닫힘".into()),
            capture_running: false,
            capture_error: Some("캡처 멈춤".into()),
            conn_closed: true,
            conn_reason: Some("연결 닫힘".into()),
        };
        assert_eq!(end_reason(&all).as_deref(), Some("연결 닫힘"));
        let no_conn = Observed {
            conn_closed: false,
            ..all
        };
        assert_eq!(end_reason(&no_conn).as_deref(), Some("송출 닫힘"));
        let capture_only = Observed {
            publisher_closed: false,
            ..no_conn
        };
        assert_eq!(end_reason(&capture_only).as_deref(), Some("캡처 멈춤"));
    }

    #[test]
    fn disconnect_is_refused_only_for_the_broadcast_key() {
        assert!(ensure_not_broadcasting(None, "abc").is_ok());
        assert!(ensure_not_broadcasting(Some("abc"), "other").is_ok());
        let err = ensure_not_broadcasting(Some("abc"), "abc").unwrap_err();
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            serde_json::json!({
                "message": "방송 중인 서버는 연결을 끊을 수 없습니다. 방송을 먼저 끝내 주세요.",
                "code": "broadcasting",
            })
        );
    }

    #[test]
    fn removing_a_server_is_refused_only_for_the_broadcast_key() {
        assert!(ensure_removable(None, "abc").is_ok());
        assert!(ensure_removable(Some("abc"), "other").is_ok());
        let err = ensure_removable(Some("abc"), "abc").unwrap_err();
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            serde_json::json!({
                "message": "방송 중인 서버는 삭제할 수 없습니다. 방송을 먼저 끝내 주세요.",
                "code": "broadcasting",
            })
        );
    }

    #[test]
    fn remove_runs_only_when_not_broadcasting() {
        let broadcaster = Broadcaster::default();
        let mut calls = 0;
        broadcaster
            .remove_unless_broadcasting("abc", || {
                calls += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(calls, 1);

        broadcaster.set_busy(Some("abc".into()));
        let err = broadcaster
            .remove_unless_broadcasting("abc", || {
                calls += 1;
                Ok(())
            })
            .unwrap_err();
        assert_eq!(calls, 1, "방송 중이면 삭제를 실행하지 않는다");
        assert_eq!(serde_json::to_value(&err).unwrap()["code"], "broadcasting");
        // 다른 서버는 지울 수 있다.
        broadcaster
            .remove_unless_broadcasting("other", || {
                calls += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(calls, 2);
        // 방송이 끝나면(busy 해제) 다시 지울 수 있다.
        broadcaster.set_busy(None);
        broadcaster
            .remove_unless_broadcasting("abc", || {
                calls += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(calls, 3);
    }

    #[test]
    fn dev_test_options_default_to_lowest_preset_and_h264() {
        let options: DevTestOptions =
            serde_json::from_value(serde_json::json!({ "hosting": "lan" })).unwrap();
        let start = options.into_start_options();
        assert_eq!(start.hosting, HostingKind::Lan);
        assert_eq!(start.source, "test");
        assert_eq!(start.audio, AudioMode::TestTone);
        assert_eq!(
            start.settings,
            QualityPreset::P720p30.settings(VideoCodec::H264)
        );
        assert_eq!(start.password, None);

        let options: DevTestOptions = serde_json::from_value(serde_json::json!({
            "hosting": "server", "serverId": "x", "password": "pw",
            "preset": "p1080p60", "codec": "vp9",
        }))
        .unwrap();
        let start = options.into_start_options();
        assert_eq!(start.server_id.as_deref(), Some("x"));
        assert_eq!(
            start.settings,
            QualityPreset::P1080p60.settings(VideoCodec::Vp9)
        );
        assert_eq!(start.password.as_deref(), Some("pw"));
    }
}
