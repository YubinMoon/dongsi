//! 방송: 캡처·인코딩(dongsi-media) → WebRTC 송출(dongsi-net) → 서버.
//!
//! 호스팅 방식(D-017, D-018):
//! - `server`: 추가한 서버(전용 서버)에 송출한다.
//! - `lan`: 앱에 내장된 서버를 열고 거기에 송출한다. 같은 LAN·tailnet에서 감지된다.
//! - `direct`: `lan`과 같고, UPnP로 공유기 포트를 열어 인터넷에서도 접속하게 한다.
//!
//! 내장 서버는 호스트 토큰을 보낸 이 앱만 방송할 수 있고, 방송을 끝내면 닫힌다.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dongsi_core::priority::{ProcessBoost, TimerResolution};
use dongsi_core::proto::{ClientMsg, MAX_PASSWORD_CHARS, PublishInfo};
use dongsi_core::{DEFAULT_PORT, InstallId, VideoCodec, VideoSettings};
use dongsi_media::{
    AudioPipeline, AudioSource, AudioStats, DISCORD_EXE_NAMES, EncodedAudio, EncodedVideo,
    PreviewFrame, RefreshRates, SourceId, SourceInfo, SourceKind, Thumbnail, VideoPipeline,
    VideoStats,
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
use crate::follow::{CurrentSource, FollowKind, Follower, HOLD, follow_kind, lock_source};
use crate::hub::{HOST_KEY, Hub, REASON_LOST, REASON_USER, ServerConn};
use crate::paths::{AppEnv, AppPaths};
use crate::session::{OPUS_KBPS, apply_publish_info, connect_publisher, stop_publishing};

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
    /// 지금 캡처 대상. 게임 창 따라가기(D-069)와 화면 전환(D-086)이 바꾼다.
    source: CurrentSource,
    /// 지금 소리 모드. 화면 전환(D-086)이 바꾼다.
    audio_mode: AudioMode,
    /// 창을 골라 "앱 소리만"이 되기 전 모드. 화면 전환으로 앱 소리를 못 쓰는 대상이 되면 이것으로 되돌린다.
    audio_before_app: AudioMode,
    /// 게임 창 따라가기(롤 창을 방송할 때만). 감시 스레드와 화면 전환이 같이 쓴다.
    follower: FollowerSlot,
    invite: String,
    invite_without_password: Option<String>,
    started_at: u64,
    stop_flag: Arc<AtomicBool>,
    monitor: Option<JoinHandle<()>>,
    /// 방송하는 동안 프로세스 우선순위 ABOVE_NORMAL(이미 바꿔 둔 등급이면 `None`)과 타이머 해상도 1ms(D-081).
    _process_boost: Option<ProcessBoost>,
    _timer: TimerResolution,
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
    /// Discord 창. "캡처하는 앱의 소리만"을 쓰면 Discord 소리가 방송에 들어가므로 화면이 그 모드를 막는다.
    discord: bool,
    /// 게임 창 따라가기 규칙(롤 클라이언트·게임 창이면 `lol`, D-069).
    follow: Option<FollowKind>,
    /// 모니터 주사율(Hz, 반올림한 정수). 창이면 지금 창이 있는 모니터의 값이다. 모르면 `None`.
    refresh_rate: Option<u32>,
}

impl SourceView {
    fn new(s: &SourceInfo, refresh_rate: Option<u32>) -> Self {
        Self {
            id: s.id.to_string(),
            kind: s.kind,
            name: s.name.clone(),
            width: s.width,
            height: s.height,
            primary: s.primary,
            process_name: s.process_name.clone(),
            discord: is_discord(s),
            follow: follow_kind(s),
            refresh_rate,
        }
    }
}

/// Discord 창(실행 파일 이름으로 가린다).
fn is_discord(s: &SourceInfo) -> bool {
    s.process_name
        .as_deref()
        .is_some_and(|n| DISCORD_EXE_NAMES.iter().any(|d| d.eq_ignore_ascii_case(n)))
}

/// 이 대상에 "캡처하는 앱의 소리만"을 쓸 수 있나: 창이고 Discord 창이 아니다(화면의 `appAudioAllowed`와 같다).
fn app_audio_allowed(s: &SourceInfo) -> bool {
    s.kind == SourceKind::Window && !is_discord(s)
}

/// 소리 모드와 캡처 대상에 맞는 오디오 캡처 대상. 앱 소리인데 프로세스를 모르면 시스템 − Discord. 소리 없음이면 `None`.
fn audio_source_for(mode: AudioMode, source: &SourceInfo) -> Option<AudioSource> {
    match mode {
        AudioMode::SystemExceptDiscord => Some(AudioSource::SystemExceptDiscord),
        AudioMode::System => Some(AudioSource::System),
        AudioMode::App => Some(match source.process_id {
            Some(pid) => AudioSource::ProcessTree(pid),
            None => AudioSource::SystemExceptDiscord,
        }),
        AudioMode::TestTone => Some(AudioSource::TestTone),
        AudioMode::None => None,
    }
}

/// 방송 중 캡처 대상을 바꿀 때 소리 모드(D-086, 시작 화면의 `audioForSource`와 같은 규칙). 앱 소리를 쓸 수 있는
/// 창이면 그 앱의 소리만, 그 밖의 대상인데 지금 "앱 소리만"이면 창을 고르기 전 모드(`before`)로 되돌린다. 나머지는
/// 그대로다. 소리 없음·테스트 톤은 바꾸지 않는다(방송 도중 오디오 트랙을 더하지 않는다).
fn audio_after_switch(target: &SourceInfo, current: AudioMode, before: AudioMode) -> AudioMode {
    match current {
        AudioMode::None | AudioMode::TestTone => current,
        _ if app_audio_allowed(target) => AudioMode::App,
        AudioMode::App if before == AudioMode::App => AudioMode::SystemExceptDiscord,
        AudioMode::App => before,
        _ => current,
    }
}

/// 게임 창 따라가기 칸(D-069). 감시 스레드가 틱마다 쓰고, 화면 전환(D-086)이 바꾼다.
type FollowerSlot = Arc<Mutex<Option<Follower>>>;

fn lock_follower(f: &FollowerSlot) -> MutexGuard<'_, Option<Follower>> {
    f.lock().unwrap_or_else(|e| e.into_inner())
}

/// 감시 스레드용: 화면 전환이 칸을 쥐고 있으면 이번 틱은 건너뛴다(감시 스레드가 기다리지 않게).
fn try_lock_follower(f: &FollowerSlot) -> Option<MutexGuard<'_, Option<Follower>>> {
    match f.try_lock() {
        Ok(g) => Some(g),
        Err(TryLockError::Poisoned(e)) => Some(e.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

/// 캡처 대상 미리 보기 하나.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailView {
    id: String,
    /// `data:image/jpeg;base64,…`. 찍지 못했으면 `None`.
    image: Option<String>,
    /// 최소화된 창(찍지 않는다).
    minimized: bool,
}

impl ThumbnailView {
    fn new(id: String, t: Thumbnail) -> Self {
        use base64::Engine as _;
        let (image, minimized) = match t {
            Thumbnail::Jpeg { data, .. } => (
                Some(format!(
                    "data:image/jpeg;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(data)
                )),
                false,
            ),
            Thumbnail::Minimized => (None, true),
            Thumbnail::Unavailable => (None, false),
        };
        Self {
            id,
            image,
            minimized,
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
    /// 지금 캡처 대상의 ID(`list_capture_sources`의 `id`). 게임 창 따라가기·화면 전환으로 바뀐다.
    source_id: String,
    source_name: String,
    /// 캡처 대상이 닫혀 다음 창을 기다리는 중(게임 창 따라가기).
    source_waiting: bool,
    audio: AudioMode,
    /// 요청한 설정(주사율 원본이면 `fps`가 0).
    settings: VideoSettings,
    width: u32,
    height: u32,
    /// 지금 쓰는 fps. 주사율 원본이면 캡처 대상 모니터의 주사율로 풀어 쓴 값이다(D-074).
    fps: u32,
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

// ---------- 명령 ----------

/// 캡처할 수 있는 화면·창 목록(모니터 주사율 포함). 이 앱의 창은 뺀다.
#[tauri::command]
pub async fn list_capture_sources() -> CmdResult<Vec<SourceView>> {
    let me = std::process::id();
    tauri::async_runtime::spawn_blocking(move || {
        let list = dongsi_media::list_sources().map_err(media_err)?;
        let rates = RefreshRates::now();
        Ok(list
            .iter()
            .filter(|s| s.process_id != Some(me))
            .filter(|s| cfg!(debug_assertions) || s.kind != SourceKind::Test)
            .map(|s| SourceView::new(s, rates.get(&s.id)))
            .collect())
    })
    .await
    .map_err(|e| CmdError::new(e.to_string()))?
}

/// 미리 보기 최대 크기(물리 픽셀). 타일 폭(약 170~300 CSS px)에 화면 배율 1.5를 곱해도 넉넉하다.
const THUMB_MAX: (u32, u32) = (480, 270);
/// 미리 보기 프레임을 기다리는 시간. 보통 0.1~0.3초 안에 온다.
const THUMB_TIMEOUT: Duration = Duration::from_millis(500);
/// 한 번에 찍는 대상 수 상한.
const THUMB_MAX_IDS: usize = 64;

/// 캡처 대상 미리 보기(방송 시작 화면이 몇 초마다 부른다). 결과는 `ids` 순서이고,
/// 잘못된 ID·테스트 패턴·찍지 못한 대상은 `image`가 비어 있다.
#[tauri::command]
pub async fn capture_source_thumbnails(ids: Vec<String>) -> CmdResult<Vec<ThumbnailView>> {
    let ids: Vec<String> = ids.into_iter().take(THUMB_MAX_IDS).collect();
    let parsed: Vec<SourceId> = ids
        .iter()
        .map(|s| s.parse().unwrap_or(SourceId::TestPattern))
        .collect();
    let shots = tauri::async_runtime::spawn_blocking(move || {
        dongsi_media::capture_thumbnails(&parsed, THUMB_MAX.0, THUMB_MAX.1, THUMB_TIMEOUT)
    })
    .await
    .map_err(|e| CmdError::new(e.to_string()))?
    .map_err(media_err)?;
    Ok(ids
        .into_iter()
        .zip(shots)
        .map(|(id, t)| ThumbnailView::new(id, t))
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
    /// 기본은 [`VideoSettings::DEFAULT`](1920×1080, 60fps, H.264, 7000kbps).
    pub settings: Option<VideoSettings>,
    /// 있으면 `settings`의 코덱을 이것으로 바꾼다.
    pub codec: Option<VideoCodec>,
}

impl DevTestOptions {
    /// 테스트 패턴 + 테스트 톤으로 `start_broadcast`와 같은 옵션을 만든다.
    fn into_start_options(self) -> StartOptions {
        let mut settings = self.settings.unwrap_or(VideoSettings::DEFAULT);
        if let Some(codec) = self.codec {
            settings.codec = codec;
        }
        StartOptions {
            hosting: self.hosting,
            server_id: self.server_id,
            source: SourceId::TestPattern.to_string(),
            settings,
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
    // 게임 다운로드처럼 CPU·네트워크를 많이 쓰는 앱과 같이 돌 때 밀리지 않게(D-081). 타이머 해상도는 str0m pacer가
    // 패킷을 제때 내보내는 데 필요하다(기본 15.6ms면 대역폭 추정이 낮게 나온다). 방송이 끝나거나 시작에 실패하면 놓는다.
    let process_boost = ProcessBoost::above_normal();
    let timer = TimerResolution::request_1ms();

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
    // 롤 창이면 게임 창 따라가기: 창이 바뀌는 사이에 대상이 닫혀도 방송을 끝내지 않는다.
    let follow = follow_kind(&source);
    if follow.is_some() {
        video.set_hold_on_close(Some(crate::follow::HOLD));
    }

    let audio = match audio_source_for(options.audio, &source) {
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
        // 주사율 원본(0)이면 파이프라인이 모니터 주사율로 풀어 쓴 값을 알린다.
        fps: video.fps(),
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
        let publisher =
            connect_publisher(&conn, info, password.clone(), settings.bitrate_kbps).await?;
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
    let current: CurrentSource = Arc::new(Mutex::new(source.clone()));
    let follower: FollowerSlot = Arc::new(Mutex::new(follow.map(|kind| {
        let app_audio = (options.audio == AudioMode::App)
            .then(|| audio.as_ref().map(AudioPipeline::control))
            .flatten();
        Follower::new(kind, current.clone(), app_audio)
    })));
    let monitor = {
        let watch = Monitor {
            app: app.clone(),
            publisher: publisher.clone(),
            video: video.clone(),
            conn: conn.clone(),
            info: info.clone(),
            stop: stop_flag.clone(),
            follower: follower.clone(),
        };
        std::thread::Builder::new()
            .name("dongsi-broadcast".into())
            .spawn(move || watch.run())
            .map_err(|e| CmdError::new(e.to_string()))?
    };
    // 방송 목록 썸네일(D-076): 첫 프레임에 한 장, 그 뒤 1분마다 올린다. 비밀번호 방송은 올리지 않는다.
    if password.is_none() {
        let grab = thumbnail_grabber(Arc::downgrade(&video), stop_flag.clone());
        if let Err(e) = crate::session::spawn_thumbnail_uploader(
            conn.clone(),
            stop_flag.clone(),
            crate::session::THUMBNAIL_INTERVAL,
            grab,
        ) {
            warn!("썸네일 스레드를 띄우지 못했습니다: {e}");
        }
    }

    info!(key, hosting = ?options.hosting, source = %source.name, ?follow, ?settings, "방송 시작");
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
        source: current,
        audio_mode: options.audio,
        audio_before_app: if options.audio == AudioMode::App {
            AudioMode::SystemExceptDiscord
        } else {
            options.audio
        },
        follower,
        invite,
        invite_without_password,
        started_at: unix_now(),
        stop_flag,
        monitor: Some(monitor),
        _process_boost: process_boost,
        _timer: timer,
    });
    busy.keep();
    Ok(status_of(slot.as_ref().expect("방금 넣었다")))
}

/// 방송 감시 스레드: 키프레임 요청 전달, 네트워크에 맞춘 비트레이트 상한 전달(D-081), 방송 정보의 크기 맞추기,
/// 송출·캡처가 끝나면 방송 종료.
struct Monitor {
    app: AppHandle,
    publisher: Arc<Publisher>,
    video: Arc<VideoPipeline>,
    conn: Arc<ServerConn>,
    info: Arc<Mutex<PublishInfo>>,
    stop: Arc<AtomicBool>,
    /// 게임 창 따라가기(롤 창을 방송할 때만, 화면 전환이 바꾼다).
    follower: FollowerSlot,
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
            follower,
        } = self;
        let mut tick = 0u32;
        let mut conn_wait_ticks = 0u32;
        // 직전 확인에서 본 출력 크기. 두 번 연속 같아야 고친다(설정을 바꾼 직후의 옛 크기 무시).
        let mut seen: Option<(u32, u32)> = None;
        // 인코더에 건 비트레이트 상한(송출의 목표). 바뀔 때만 보낸다.
        let mut cap: Option<u32> = None;
        while !stop.load(Ordering::Relaxed) {
            if publisher.take_keyframe_request() {
                video.request_keyframe();
            }
            let target = publisher.video_target_kbps();
            if cap != Some(target) {
                video.set_bitrate_cap(Some(target));
                cap = Some(target);
            }
            if let Some(mut f) = try_lock_follower(&follower)
                && let Some(f) = f.as_mut()
            {
                f.tick(&video);
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

/// 인코더의 실제 출력 크기·fps가 서버에 알린 값과 다르면 고쳐 알린다.
/// 창 크기가 바뀌거나, 원본 크기를 미리 알 수 없는 소스(테스트 패턴 등)이거나, 주사율 원본으로 방송하다
/// 대상이 주사율이 다른 모니터로 바뀌면(게임 창 따라가기 포함) 생긴다.
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
    let fps = video.fps();
    let stable = *seen == Some(size);
    *seen = Some(size);
    let mut info = info.lock().unwrap_or_else(|e| e.into_inner());
    if !stable
        || stats.codec != Some(info.codec)
        || ((info.width, info.height) == size && info.fps == fps)
    {
        return;
    }
    info.width = size.0;
    info.height = size.1;
    info.fps = fps;
    let updated = *info;
    drop(info);
    info!(
        width = size.0,
        height = size.1,
        fps,
        "방송 정보의 출력 크기·fps를 고침"
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

/// 방송 목록 썸네일 한 장(D-076): 송출 화면 미리 보기(D-066)를 요청한 뒤에 나온 새 프레임을 JPEG으로 만든다.
///
/// 파이프라인은 약한 참조로만 잡는다(방송을 끝낼 때 이 스레드가 캡처·송출 정리를 늦추지 않게).
/// 새 프레임이 1.5초 안에 안 오면(방송 시작 직후, 화면이 멈춤) `None`.
fn thumbnail_grabber(
    video: std::sync::Weak<VideoPipeline>,
    stop: Arc<AtomicBool>,
) -> impl FnMut() -> Option<Vec<u8>> + Send + 'static {
    const FRESH_WAIT: Duration = Duration::from_millis(1500);
    const POLL: Duration = Duration::from_millis(50);
    move || {
        let (w, h) = crate::session::THUMBNAIL_MAX;
        // 칸에 남은 그림은 오래됐을 수 있다(송출 화면 카드가 예전에 부른 것). 그 번호 뒤에 나온 그림을 쓴다.
        let base = video
            .upgrade()?
            .preview(u64::MAX, w, h)
            .map_or(0, |f| f.seq);
        let deadline = std::time::Instant::now() + FRESH_WAIT;
        let frame = loop {
            std::thread::sleep(POLL);
            if stop.load(Ordering::Relaxed) || std::time::Instant::now() > deadline {
                return None;
            }
            if let Some(f) = video.upgrade()?.preview(base, w, h) {
                break f;
            }
        };
        match dongsi_media::preview_jpeg(&frame, w, h, crate::session::THUMBNAIL_TARGET_BYTES) {
            Ok(dongsi_media::Thumbnail::Jpeg { data, .. }) => Some(data),
            Ok(_) => None,
            Err(e) => {
                warn!("썸네일을 만들지 못했습니다: {e}");
                None
            }
        }
    }
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
        source_id: lock_source(&a.source).id.to_string(),
        source_name: lock_source(&a.source).name.clone(),
        source_waiting: a.video.is_waiting_source(),
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
        fps: a.video.fps(),
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

/// 송출 화면 미리 보기(방송 중 화면의 "다른 사람에게 보이는 모습" 카드, D-066). 소리는 없다.
///
/// 응답은 바이너리다([`preview_bytes`]). 화면은 받은 번호를 `after`로 다시 보낸다. 부를 때마다 1초 동안
/// 미리 보기를 만들게 하므로 화면이 부르지 않으면 비용이 없다. `max_width`×`max_height`는 기기 픽셀이다.
#[tauri::command]
pub async fn broadcast_preview(
    broadcaster: State<'_, Broadcaster>,
    after: u32,
    max_width: u32,
    max_height: u32,
) -> CmdResult<tauri::ipc::Response> {
    let video = broadcaster
        .active
        .lock()
        .await
        .as_ref()
        .map(|a| a.video.clone());
    let frame = video.and_then(|v| v.preview(u64::from(after), max_width, max_height));
    Ok(tauri::ipc::Response::new(preview_bytes(frame.as_deref())))
}

/// [`broadcast_preview`]의 응답: 방송 중이 아니거나 새 그림이 없으면 빈 배열, 있으면 머리 12바이트
/// (번호·너비·높이, 각각 u32 리틀 엔디언) 뒤에 RGBA(행 간격 = 너비 × 4).
fn preview_bytes(frame: Option<&PreviewFrame>) -> Vec<u8> {
    let Some(f) = frame else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(12 + f.rgba.len());
    out.extend_from_slice(&(f.seq as u32).to_le_bytes());
    out.extend_from_slice(&f.width.to_le_bytes());
    out.extend_from_slice(&f.height.to_le_bytes());
    out.extend_from_slice(&f.rgba);
    out
}

/// 방송 중에 코덱·해상도·fps·비트레이트를 바꾼다.
///
/// 비트레이트는 송출의 상한(D-081)을 먼저 바꾸고 인코더 설정을 바꾼다. 인코더 비트레이트는 `min(설정, 상한)`이고
/// 상한은 감시 스레드가 송출의 목표를 따라 건다. 어느 쪽이 먼저 적용돼도 인코더는 한 번만 바뀐다.
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
    let old_max = video.settings().bitrate_kbps;
    active.publisher.set_max_video_kbps(settings.bitrate_kbps);
    let updated = match tauri::async_runtime::spawn_blocking(move || video.update(settings)).await {
        Ok(r) => r.map_err(media_err),
        Err(e) => Err(CmdError::new(e.to_string())),
    };
    if let Err(e) = updated {
        active.publisher.set_max_video_kbps(old_max);
        return Err(e);
    }
    let (sw, sh) = {
        let s = lock_source(&active.source);
        (s.width, s.height)
    };
    let (width, height) = settings.output_size(sw, sh);
    let info = PublishInfo {
        codec: settings.codec,
        width,
        height,
        fps: active.video.fps(),
        has_audio: active.info().has_audio,
    };
    apply_publish_info(&active.conn, &active.info, info)?;
    info!(?settings, "방송 설정 변경");
    Ok(status_of(active))
}

/// 방송을 멈추지 않고 캡처 대상을 바꾼다(D-086). 새 대상을 먼저 열고 옛 캡처를 닫으므로, 열지 못하면 지금 대상이
/// 그대로다. 소리는 [`audio_after_switch`] 규칙을 따르고, 롤 창이면 게임 창 따라가기(D-069)를 켜고 아니면 끈다.
/// 출력 크기·fps가 바뀌면 감시 스레드가 서버에 다시 알린다(`sync_output_size`).
#[tauri::command]
pub async fn switch_broadcast_source(
    broadcaster: State<'_, Broadcaster>,
    source: String,
) -> CmdResult<BroadcastStatus> {
    let mut slot = broadcaster.active.lock().await;
    let active = slot
        .as_mut()
        .ok_or_else(|| CmdError::new("방송 중이 아닙니다"))?;
    let id: SourceId = source
        .parse()
        .map_err(|_| CmdError::new("캡처 대상을 찾을 수 없습니다"))?;
    let target = find_source(&id).await?;
    if target.kind == SourceKind::Test || target.process_id == Some(std::process::id()) {
        return Err(CmdError::new("이 대상으로는 바꿀 수 없습니다"));
    }
    let from = lock_source(&active.source).clone();
    if from.id == target.id && !active.video.is_waiting_source() {
        return Ok(status_of(active));
    }
    let before = if active.audio_mode == AudioMode::App {
        active.audio_before_app
    } else {
        active.audio_mode
    };
    let audio_mode = match &active.audio {
        Some(_) => audio_after_switch(&target, active.audio_mode, before),
        None => active.audio_mode,
    };
    let follow = follow_kind(&target);
    let audio = active.audio.as_ref().map(AudioPipeline::control);
    let video = active.video.clone();
    let current = active.source.clone();
    let follower = active.follower.clone();
    let next = target.clone();
    let switched = tauri::async_runtime::spawn_blocking(move || {
        // 따라가기 칸을 쥔 채 바꾼다(그사이 감시 스레드의 따라가기가 옛 대상으로 되돌리지 않게).
        let mut follower = lock_follower(&follower);
        video.switch_source(next.id.clone())?;
        // 바꾼 뒤에 건다(대상이 닫혀 기다리는 중에 먼저 끄면 그 자리에서 방송이 끝난다).
        video.set_hold_on_close(follow.map(|_| HOLD));
        if let (Some(a), Some(src)) = (&audio, audio_source_for(audio_mode, &next)) {
            a.set_source(src);
        }
        *lock_source(&current) = next;
        *follower = follow.map(|kind| {
            let app_audio = (audio_mode == AudioMode::App)
                .then(|| audio.clone())
                .flatten();
            Follower::new(kind, current.clone(), app_audio)
        });
        Ok::<_, dongsi_media::MediaError>(())
    })
    .await;
    match switched {
        Ok(r) => r.map_err(media_err)?,
        Err(e) => return Err(CmdError::new(e.to_string())),
    }
    active.audio_before_app = before;
    active.audio_mode = audio_mode;
    info!(from = %from.name, to = %target.name, audio = ?audio_mode, ?follow, "방송 캡처 대상 변경");
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

    #[test]
    fn preview_bytes_header_then_rgba() {
        assert!(preview_bytes(None).is_empty());
        let f = PreviewFrame {
            seq: 3,
            width: 2,
            height: 1,
            rgba: vec![1, 2, 3, 255, 4, 5, 6, 255],
        };
        assert_eq!(
            preview_bytes(Some(&f)),
            vec![
                3, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 1, 2, 3, 255, 4, 5, 6, 255
            ]
        );
    }

    #[test]
    fn thumbnail_view_is_a_jpeg_data_url_or_empty() {
        let t = ThumbnailView::new(
            "monitor:1".into(),
            Thumbnail::Jpeg {
                width: 2,
                height: 1,
                data: vec![0xFF, 0xD8, 0xFF],
            },
        );
        assert_eq!(t.image.as_deref(), Some("data:image/jpeg;base64,/9j/"));
        assert!(!t.minimized);
        let t = ThumbnailView::new("window:2".into(), Thumbnail::Minimized);
        assert_eq!((t.image, t.minimized), (None, true));
        let t = ThumbnailView::new("test".into(), Thumbnail::Unavailable);
        assert_eq!((t.image, t.minimized), (None, false));
    }

    #[test]
    fn source_view_marks_discord_windows() {
        let info = |process: Option<&str>| SourceInfo {
            id: SourceId::Window(1),
            kind: SourceKind::Window,
            name: "창".into(),
            width: 100,
            height: 100,
            primary: false,
            process_id: Some(1),
            process_name: process.map(str::to_owned),
        };
        assert!(SourceView::new(&info(Some("discord.exe")), None).discord);
        assert!(SourceView::new(&info(Some("DiscordPTB.exe")), None).discord);
        assert!(!SourceView::new(&info(Some("chrome.exe")), None).discord);
        assert!(!SourceView::new(&info(None), None).discord);
    }

    #[test]
    fn source_view_sends_the_refresh_rate() {
        let info = SourceInfo {
            id: SourceId::Monitor(1),
            kind: SourceKind::Monitor,
            name: "화면".into(),
            width: 2560,
            height: 1440,
            primary: true,
            process_id: None,
            process_name: None,
        };
        let v = serde_json::to_value(SourceView::new(&info, Some(144))).unwrap();
        assert_eq!(v["refreshRate"], 144);
        let v = serde_json::to_value(SourceView::new(&info, None)).unwrap();
        assert!(v["refreshRate"].is_null());
    }

    fn source(kind: SourceKind, process: Option<&str>, pid: Option<u32>) -> SourceInfo {
        SourceInfo {
            id: SourceId::Window(1),
            kind,
            name: "대상".into(),
            width: 1920,
            height: 1080,
            primary: false,
            process_id: pid,
            process_name: process.map(str::to_owned),
        }
    }

    #[test]
    fn switching_to_a_window_uses_the_app_audio_and_back_restores_the_mode_before() {
        use AudioMode::{App, System, SystemExceptDiscord, TestTone};
        let window = source(SourceKind::Window, Some("chrome.exe"), Some(7));
        let monitor = source(SourceKind::Monitor, None, None);
        let discord = source(SourceKind::Window, Some("Discord.exe"), Some(8));
        // 창으로: 앱 소리를 쓸 수 있으면 앱 소리만(시스템 전체였어도).
        assert_eq!(
            audio_after_switch(&window, SystemExceptDiscord, SystemExceptDiscord),
            App
        );
        assert_eq!(audio_after_switch(&window, System, System), App);
        assert_eq!(audio_after_switch(&window, App, System), App);
        // 화면·Discord 창으로: 앱 소리였으면 창을 고르기 전 모드, 그 전도 앱 소리면 시스템 − Discord.
        assert_eq!(audio_after_switch(&monitor, App, System), System);
        assert_eq!(audio_after_switch(&monitor, App, App), SystemExceptDiscord);
        assert_eq!(
            audio_after_switch(&discord, App, SystemExceptDiscord),
            SystemExceptDiscord
        );
        // 앱 소리가 아니면 그대로(Discord 창이어도 앱 소리로 바꾸지 않는다).
        assert_eq!(audio_after_switch(&monitor, System, System), System);
        assert_eq!(audio_after_switch(&discord, System, System), System);
        assert_eq!(
            audio_after_switch(&discord, SystemExceptDiscord, SystemExceptDiscord),
            SystemExceptDiscord
        );
        // 소리 없음·테스트 톤은 바꾸지 않는다(방송 도중 오디오 트랙을 더하지 않는다).
        assert_eq!(
            audio_after_switch(&window, AudioMode::None, AudioMode::None),
            AudioMode::None
        );
        assert_eq!(audio_after_switch(&window, TestTone, TestTone), TestTone);
    }

    #[test]
    fn audio_source_for_follows_the_mode_and_the_window_process() {
        let window = source(SourceKind::Window, Some("chrome.exe"), Some(7));
        let no_pid = source(SourceKind::Window, Some("chrome.exe"), None);
        assert_eq!(
            audio_source_for(AudioMode::App, &window),
            Some(AudioSource::ProcessTree(7))
        );
        assert_eq!(
            audio_source_for(AudioMode::App, &no_pid),
            Some(AudioSource::SystemExceptDiscord)
        );
        assert_eq!(
            audio_source_for(AudioMode::SystemExceptDiscord, &window),
            Some(AudioSource::SystemExceptDiscord)
        );
        assert_eq!(
            audio_source_for(AudioMode::System, &window),
            Some(AudioSource::System)
        );
        assert_eq!(audio_source_for(AudioMode::None, &window), None);
    }

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
    fn dev_test_options_default_to_1080p60_h264() {
        let options: DevTestOptions =
            serde_json::from_value(serde_json::json!({ "hosting": "lan" })).unwrap();
        let start = options.into_start_options();
        assert_eq!(start.hosting, HostingKind::Lan);
        assert_eq!(start.source, "test");
        assert_eq!(start.audio, AudioMode::TestTone);
        assert_eq!(start.settings, VideoSettings::DEFAULT);
        assert_eq!(
            (
                start.settings.codec,
                start.settings.width,
                start.settings.height
            ),
            (VideoCodec::H264, 1920, 1080)
        );
        assert_eq!(
            (start.settings.fps, start.settings.bitrate_kbps),
            (60, 7000)
        );
        assert_eq!(start.password, None);

        let options: DevTestOptions = serde_json::from_value(serde_json::json!({
            "hosting": "server", "serverId": "x", "password": "pw",
            "settings": { "codec": "h264", "width": 2560, "height": 1440, "fps": 0, "bitrateKbps": 9000 },
            "codec": "av1",
        }))
        .unwrap();
        let start = options.into_start_options();
        assert_eq!(start.server_id.as_deref(), Some("x"));
        assert_eq!(
            start.settings,
            VideoSettings {
                codec: VideoCodec::Av1,
                width: 2560,
                height: 1440,
                fps: 0,
                bitrate_kbps: 9000,
            }
        );
        assert_eq!(start.password.as_deref(), Some("pw"));
    }
}
