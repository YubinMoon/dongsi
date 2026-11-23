//! 통합 시험 도움 코드: 시험용 앱(Hub + 상태 저장소), 이벤트 기다리기, 합성 방송, str0m 시청자.
//!
//! 앱이 쓰는 코드(`hub`, `embedded`, `session`, `viewing`, `store`)를 그대로 부르고, Tauri만 없다.
#![allow(dead_code)]

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dongsi_core::proto::PublishInfo;
use dongsi_core::{Fingerprint, InstallId, ServerId, VideoCodec};
use dongsi_desktop_lib::commands::AppState;
use dongsi_desktop_lib::embedded::{
    BoxFuture, Embedded, EmbeddedParams, HostingKind, PortMapper, PortMapping, start_embedded,
};
use dongsi_desktop_lib::error::CmdError;
use dongsi_desktop_lib::hub::{HOST_KEY, Hub, HubEvent, HubHost, ServerConn, Target};
use dongsi_desktop_lib::session::{apply_publish_info, connect_publisher, stop_publishing};
use dongsi_desktop_lib::store::Store;
use dongsi_desktop_lib::viewing::{self, LocalServerView, WatchResult};
use dongsi_net::discovery::{self, Via};
use dongsi_net::ifaces::primary_lan_ip;
use dongsi_net::publish::{Publisher, SR_INTERVAL};
use str0m::change::SdpAnswer;
use str0m::format::Codec;
use str0m::media::{Direction, MediaKind};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};

/// 기다리는 제한 시간.
pub const WAIT: Duration = Duration::from_secs(10);
const MAGIC: &[u8; 3] = b"DSK";
/// 표시 길이: `DSK` + tick(16진 8자).
const MARKER_LEN: usize = 3 + 8;

// ---------- 임시 폴더 ----------

/// 시험용 임시 폴더(지울 때 링크를 따라가지 않도록 `remove_dir_all`만 쓴다. 안에 링크를 만들지 않는다).
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        Self(std::env::temp_dir().join(format!("dongsi-lan-test-{tag}-{}", InstallId::generate())))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------- 조건 기다리기 ----------

/// `cond`가 참이 될 때까지 제한 시간 안에서 기다린다. 못 기다리면 `what`을 담아 패닉한다.
pub async fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
    eventually_within(what, WAIT, &mut cond).await
}

pub async fn eventually_within(what: &str, limit: Duration, cond: &mut impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !cond() {
        assert!(
            Instant::now() < deadline,
            "시간 안에 되지 않았습니다: {what}"
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

// ---------- 이벤트 ----------

/// Hub가 알린 이벤트를 모두 기록한다.
#[derive(Clone, Default)]
pub struct Events(Arc<Mutex<Vec<HubEvent>>>);

impl Events {
    /// 지금까지 쌓인 이벤트 수. `wait`·`since`의 시작점으로 쓴다.
    pub fn mark(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    pub fn since(&self, from: usize) -> Vec<HubEvent> {
        self.0.lock().unwrap()[from..].to_vec()
    }

    pub fn all(&self) -> Vec<HubEvent> {
        self.since(0)
    }

    /// `from` 이후에 `pred`를 만족하는 이벤트가 올 때까지 기다리고, 그 이벤트의 위치(전체 기준)를 돌려준다.
    pub async fn wait(&self, from: usize, what: &str, pred: impl Fn(&HubEvent) -> bool) -> usize {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(i) = self.0.lock().unwrap()[from..].iter().position(&pred) {
                return from + i;
            }
            assert!(
                Instant::now() < deadline,
                "이벤트가 오지 않았습니다: {what} (받은 것: {:?})",
                self.since(from)
            );
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }

    /// `dur` 동안 기다린 뒤 `from` 이후 `pred`를 만족한 이벤트 수.
    pub async fn count_after(
        &self,
        from: usize,
        dur: Duration,
        pred: impl Fn(&HubEvent) -> bool,
    ) -> usize {
        tokio::time::sleep(dur).await;
        self.since(from).iter().filter(|e| pred(e)).count()
    }
}

// ---------- 시험용 앱 ----------

/// 앱의 `AppState`(상태 저장소)를 그대로 쓰고 이벤트를 기록하는 `HubHost`.
struct StoreHost {
    state: Arc<AppState>,
    events: Events,
}

impl HubHost for StoreHost {
    fn identity(&self) -> (InstallId, String) {
        self.state.identity()
    }

    fn saved_target(&self, id: ServerId) -> Option<Target> {
        self.state.saved_target(id)
    }

    fn confirm_fingerprint(&self, id: ServerId, fingerprint: Fingerprint) {
        self.state.confirm_fingerprint(id, fingerprint);
    }

    fn emit(&self, event: HubEvent) {
        self.events.0.lock().unwrap().push(event);
    }
}

/// 앱 하나: 설치 ID·이름·서버 목록이 든 임시 `Store`와 `Hub`.
pub struct TestApp {
    pub name: String,
    pub hub: Hub,
    pub state: Arc<AppState>,
    pub events: Events,
    pub dir: TempDir,
}

impl TestApp {
    pub fn new(name: &str) -> Self {
        let dir = TempDir::new("app");
        let mut store = Store::open(&dir.0).unwrap();
        store.update(|s| s.set_display_name(name)).unwrap();
        let state = Arc::new(AppState::new(store));
        let events = Events::default();
        let hub = Hub::new(Arc::new(StoreHost {
            state: state.clone(),
            events: events.clone(),
        }));
        Self {
            name: name.to_owned(),
            hub,
            state,
            events,
            dir,
        }
    }

    pub fn install_id(&self) -> InstallId {
        self.state.identity().0
    }

    /// 내장 서버 인증서 폴더(앱 데이터의 `host/`와 같은 자리).
    pub fn host_dir(&self) -> PathBuf {
        self.dir.0.join("host")
    }

    /// 로컬 감지로 서버를 찾는다(이 앱의 설치 ID·저장한 서버를 기준으로 화면 목록을 만든다).
    pub async fn discover(
        &self,
        targets: &[(SocketAddr, Via)],
        wait: Duration,
    ) -> Vec<LocalServerView> {
        let found = discovery::scan_targets(targets, wait).await;
        let me = self.install_id();
        let saved = viewing::saved_fingerprints(&self.state);
        viewing::local_server_views(&self.hub, found, me, &saved)
    }

    /// 방송이 보일 때까지 감지를 되풀이한다(출발지별 초당 10회 질의 제한을 넘지 않게 한 번에 300ms씩 기다린다. 훑기 하나는 질의를 최대 3번 보낸다).
    pub async fn discover_until(
        &self,
        targets: &[(SocketAddr, Via)],
        what: &str,
        pred: impl Fn(&[LocalServerView]) -> bool,
    ) -> Vec<LocalServerView> {
        let deadline = Instant::now() + WAIT;
        loop {
            let views = self.discover(targets, Duration::from_millis(300)).await;
            if pred(&views) {
                return views;
            }
            assert!(Instant::now() < deadline, "감지로 찾지 못했습니다: {what}");
        }
    }

    /// 시청을 시작하고 str0m 수신자를 이어 돌려준다.
    pub async fn watch(
        &self,
        key: &str,
        broadcaster: InstallId,
        password: Option<&str>,
        remember: bool,
    ) -> Result<Watching, CmdError> {
        let pending = PendingViewer::new();
        let result = viewing::watch(
            &self.hub,
            &self.state,
            key,
            &broadcaster.to_string(),
            password.map(str::to_owned),
            remember,
            pending.offer.clone(),
        )
        .await?;
        let viewer = pending.start(&result.answer.answer);
        Ok(Watching {
            session: result.answer.session,
            viewer,
            result,
        })
    }
}

/// 시작한 시청: 서버의 응답과 미디어 수신자.
pub struct Watching {
    pub session: u64,
    pub viewer: TestViewer,
    pub result: WatchResult,
}

/// 감지 대상: 루프백만.
pub fn loopback_targets(port: u16) -> Vec<(SocketAddr, Via)> {
    vec![(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        Via::Local,
    )]
}

/// 감지 대상: 앱의 기본 대상에서 Tailscale만 뺀 것(인터페이스별 서브넷 브로드캐스트 + 루프백).
pub async fn default_targets(port: u16) -> Vec<(SocketAddr, Via)> {
    discovery::default_targets(port, false).await
}

// ---------- 가짜 UPnP ----------

/// 가짜 공유기가 하는 일.
#[derive(Clone)]
pub enum Router {
    /// 공유기를 찾지 못함.
    Missing(String),
    /// 포트를 열었다. `port`가 `None`이면 내부 포트와 같은 외부 포트.
    Maps { ip: IpAddr, port: Option<u16> },
}

pub struct FakeMapper {
    router: Router,
    pub calls: Mutex<Vec<(Ipv4Addr, u16)>>,
    pub removed: Arc<AtomicUsize>,
}

impl FakeMapper {
    pub fn new(router: Router) -> Arc<Self> {
        Arc::new(Self {
            router,
            calls: Mutex::default(),
            removed: Arc::default(),
        })
    }
}

struct FakeMapping {
    external: SocketAddr,
    removed: Arc<AtomicUsize>,
}

impl PortMapping for FakeMapping {
    fn external(&self) -> SocketAddr {
        self.external
    }

    fn renew(&self) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn remove(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.removed.fetch_add(1, Ordering::SeqCst);
        })
    }
}

impl PortMapper for FakeMapper {
    fn map_ports(
        &self,
        lan_ip: Ipv4Addr,
        port: u16,
    ) -> BoxFuture<'_, Result<Arc<dyn PortMapping>, String>> {
        self.calls.lock().unwrap().push((lan_ip, port));
        let result = match &self.router {
            Router::Missing(msg) => Err(msg.clone()),
            Router::Maps { ip, port: ext } => Ok(Arc::new(FakeMapping {
                external: SocketAddr::new(*ip, ext.unwrap_or(port)),
                removed: self.removed.clone(),
            }) as Arc<dyn PortMapping>),
        };
        Box::pin(async move { result })
    }
}

/// 어떤 가짜 공유기에도 쓰이지 않는 매퍼(`lan`은 포트를 열지 않으므로 호출되면 안 된다).
pub fn unused_mapper() -> Arc<FakeMapper> {
    FakeMapper::new(Router::Missing("호출되면 안 됩니다".into()))
}

// ---------- 합성 방송 ----------

/// 20ms마다 오디오, 40ms마다 비디오(같은 캡처 시각)를 보내는 스레드.
pub struct Feeder {
    stop: Arc<AtomicBool>,
    codec: Arc<AtomicU8>,
    thread: Option<thread::JoinHandle<()>>,
}

fn codec_id(codec: VideoCodec) -> u8 {
    match codec {
        VideoCodec::H264 => 0,
        _ => 1,
    }
}

fn codec_of(id: u8) -> VideoCodec {
    if id == 0 {
        VideoCodec::H264
    } else {
        VideoCodec::Av1
    }
}

impl Feeder {
    pub fn start(publisher: Arc<Publisher>, codec: VideoCodec) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let codec = Arc::new(AtomicU8::new(codec_id(codec)));
        let (t_stop, t_codec) = (stop.clone(), codec.clone());
        let thread = thread::spawn(move || {
            let start = Instant::now();
            let mut sample_index = 0u64;
            let mut tick = 0u32;
            while !t_stop.load(Ordering::Relaxed) {
                let at = start + Duration::from_millis(20 * u64::from(tick));
                let now = Instant::now();
                if at > now {
                    thread::sleep(at - now);
                }
                publisher.send_audio(payload(tick, 80), at, sample_index);
                sample_index += 960;
                if tick.is_multiple_of(2) {
                    let codec = codec_of(t_codec.load(Ordering::Relaxed));
                    publisher.send_video(codec, video_frame(codec, tick), at);
                }
                tick += 1;
            }
        });
        Self {
            stop,
            codec,
            thread: Some(thread),
        }
    }

    /// 이후 프레임의 코덱(방송 정보 갱신과 함께 부른다).
    pub fn set_codec(&self, codec: VideoCodec) {
        self.codec.store(codec_id(codec), Ordering::Relaxed);
    }

    /// 멈추고 기다린다. 송출 연결의 참조를 놓는다.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Feeder {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 페이로드: `DSK` + tick(16진 8자) + 채움(0xAA). 0 바이트가 없어서 H.264 시작 코드(`00 00 01`)로 잘못
/// 나뉘지 않는다.
pub fn payload(index: u32, len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len.max(MARKER_LEN));
    v.extend_from_slice(MAGIC);
    v.extend_from_slice(format!("{index:08x}").as_bytes());
    v.resize(len.max(MARKER_LEN), 0xAA);
    v
}

/// 시험용 비디오 키프레임. H.264는 Annex B(SPS, PPS, IDR), AV1은 sequence header OBU + frame OBU
/// (str0m은 첫 OBU가 sequence header면 키프레임으로 보낸다). 디코딩하지 않으므로 형식만 맞춘다.
pub fn video_frame(codec: VideoCodec, index: u32) -> Vec<u8> {
    let body = payload(index, 3000);
    match codec {
        VideoCodec::H264 => {
            let mut v = vec![0, 0, 0, 1, 0x67, 0x64, 0x00, 0x1f, 0xac, 0xd9, 0x40];
            v.extend_from_slice(&[0, 0, 0, 1, 0x68, 0xeb, 0xe3, 0xcb]);
            v.extend_from_slice(&[0, 0, 0, 1, 0x65]);
            v.extend_from_slice(&body);
            v
        }
        VideoCodec::Av1 => {
            // sequence header OBU(유형 1, 크기 필드 있음) + frame OBU(유형 6, 크기는 leb128 2바이트)
            let mut v = vec![0x0a, 0x07, 0x00, 0x00, 0x00, 0x02, 0xaf, 0xff, 0x9b];
            let n = body.len();
            assert!(n < 1 << 14);
            v.extend_from_slice(&[0x32, (n & 0x7f) as u8 | 0x80, (n >> 7) as u8]);
            v.extend_from_slice(&body);
            v
        }
    }
}

pub fn publish_info(codec: VideoCodec, width: u32, height: u32, fps: u32) -> PublishInfo {
    PublishInfo {
        codec,
        width,
        height,
        fps,
        has_audio: true,
    }
}

/// 방송을 시작할 때의 선택.
pub struct LanOptions {
    pub hosting: HostingKind,
    pub port: u16,
    pub password: Option<String>,
    pub lan_ip: Option<Ipv4Addr>,
    pub info: PublishInfo,
    /// `false`면 송출 연결까지만 만들고 프레임은 보내지 않는다.
    pub feed: bool,
}

impl LanOptions {
    pub fn new(port: u16) -> Self {
        Self {
            hosting: HostingKind::Lan,
            port,
            password: None,
            lan_ip: primary_lan_ip(),
            info: publish_info(VideoCodec::H264, 1280, 720, 25),
            feed: true,
        }
    }

    pub fn password(mut self, password: &str) -> Self {
        self.password = Some(password.to_owned());
        self
    }

    pub fn hosting(mut self, hosting: HostingKind) -> Self {
        self.hosting = hosting;
        self
    }
}

/// 방송자 앱이 내장 서버를 켜고 `host` 키로 접속해 합성 방송을 송출하는 중인 상태.
pub struct Lan {
    pub app: TestApp,
    pub embedded: Embedded,
    pub conn: Arc<ServerConn>,
    pub publisher: Arc<Publisher>,
    pub info: Arc<Mutex<PublishInfo>>,
    pub feeder: Option<Feeder>,
    pub mapper: Arc<FakeMapper>,
}

impl Lan {
    pub async fn start(name: &str, options: LanOptions, mapper: Arc<FakeMapper>) -> Self {
        Self::start_on(TestApp::new(name), options, mapper).await
    }

    /// 이미 있는 방송자 앱(같은 설치 ID·같은 인증서 폴더)에서 방송을 (다시) 시작한다.
    pub async fn start_on(app: TestApp, options: LanOptions, mapper: Arc<FakeMapper>) -> Self {
        let embedded = start_embedded(
            &app.hub,
            EmbeddedParams {
                dir: &app.host_dir(),
                name: &app.name,
                hosting: options.hosting,
                port: options.port,
                lan_ip: options.lan_ip,
            },
            &*mapper,
        )
        .await
        .expect("내장 서버를 열지 못했습니다");
        let conn = app.hub.connect(HOST_KEY).await.expect("host 접속 실패");
        assert!(conn.can_publish, "호스트는 방송할 수 있어야 합니다");
        assert!(conn.host_only);
        let publisher =
            connect_publisher(&conn, options.info, options.password.clone(), 7_000, false)
                .await
                .expect("송출 연결 실패");
        eventually("송출 연결", || publisher.stats().connected).await;
        let feeder = options
            .feed
            .then(|| Feeder::start(publisher.clone(), options.info.codec));
        Self {
            app,
            embedded,
            conn,
            publisher,
            info: Arc::new(Mutex::new(options.info)),
            feeder,
            mapper,
        }
    }

    pub fn port(&self) -> u16 {
        self.embedded.port()
    }

    pub fn id(&self) -> InstallId {
        self.app.install_id()
    }

    /// 방송 중 방송 정보를 바꾸고(`UpdatePublish`) 이후 프레임의 코덱도 바꾼다.
    pub fn change(&self, info: PublishInfo) -> bool {
        let changed = apply_publish_info(&self.conn, &self.info, info).unwrap();
        if let Some(f) = &self.feeder {
            f.set_codec(info.codec);
        }
        changed
    }

    /// 앱이 방송을 끝낼 때의 순서(`stop_publishing`)로 끝낸다. 방송자 앱(Hub·상태)을 돌려준다.
    pub async fn stop(self) -> TestApp {
        let Lan {
            app,
            embedded,
            conn,
            publisher,
            feeder,
            ..
        } = self;
        let mut feeder = feeder;
        stop_publishing(&app.hub, &conn, publisher, Some(embedded), move || {
            if let Some(f) = feeder.as_mut() {
                f.stop();
            }
        })
        .await;
        app
    }
}

// ---------- str0m 시청자(수신) ----------

/// 시청 쪽 str0m 클라이언트(시험용). 받은 미디어를 채널로 넘긴다.
pub struct TestViewer {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    media: std::sync::mpsc::Receiver<Received>,
}

impl TestViewer {
    /// 지금까지 받은 것을 모두 꺼낸다.
    pub fn drain(&self) -> Vec<Received> {
        self.media.try_iter().collect()
    }
}

#[derive(Debug, Clone)]
pub struct Received {
    pub kind: MediaKind,
    pub codec: Codec,
    pub index: Option<u32>,
    /// 시청 쪽에서 SR로 복원한 캡처 시각
    pub capture: Option<std::time::SystemTime>,
}

pub struct PendingViewer {
    rtc: Rtc,
    pending: str0m::change::SdpPendingOffer,
    socket: UdpSocket,
    local: SocketAddr,
    pub offer: String,
}

impl PendingViewer {
    pub fn new() -> Self {
        let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
        let ip = primary_lan_ip()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let local = SocketAddr::new(ip, socket.local_addr().unwrap().port());
        let mut rtc = Rtc::builder()
            .clear_codecs()
            .enable_opus(true, false)
            .enable_h264(true)
            .enable_av1(true)
            .set_rtcp_report_interval_audio(SR_INTERVAL)
            .set_rtcp_report_interval_video(SR_INTERVAL)
            .build(Instant::now());
        rtc.add_local_candidate(Candidate::host(local, "udp").unwrap());
        let mut api = rtc.sdp_api();
        api.add_media(MediaKind::Video, Direction::RecvOnly, None, None, None);
        api.add_media(MediaKind::Audio, Direction::RecvOnly, None, None, None);
        let (offer, pending) = api.apply().unwrap();
        Self {
            rtc,
            pending,
            socket,
            local,
            offer: offer.to_sdp_string(),
        }
    }

    pub fn start(self, answer: &str) -> TestViewer {
        let PendingViewer {
            mut rtc,
            pending,
            socket,
            local,
            ..
        } = self;
        rtc.sdp_api()
            .accept_answer(pending, SdpAnswer::from_sdp_string(answer).unwrap())
            .unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(5)))
            .unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let thread_stop = stop.clone();
        let thread = thread::spawn(move || {
            let mut buf = vec![0u8; 2048];
            while !thread_stop.load(Ordering::Relaxed) && rtc.is_alive() {
                loop {
                    match rtc.poll_output().unwrap() {
                        Output::Timeout(_) => break,
                        Output::Transmit(t) => {
                            let _ = socket.send_to(&t.contents, t.destination);
                        }
                        Output::Event(Event::MediaData(d)) => {
                            let kind = if d.params.spec().codec == Codec::Opus {
                                MediaKind::Audio
                            } else {
                                MediaKind::Video
                            };
                            let capture = d.last_sender_info.map(|sr| {
                                let delta = d.time.as_seconds()
                                    - sr.rtp_time.rebase(d.time.frequency()).as_seconds();
                                if delta >= 0.0 {
                                    sr.ntp_time + Duration::from_secs_f64(delta)
                                } else {
                                    sr.ntp_time - Duration::from_secs_f64(-delta)
                                }
                            });
                            let _ = tx.send(Received {
                                kind,
                                codec: d.params.spec().codec,
                                index: find_index(&d.data),
                                capture,
                            });
                        }
                        Output::Event(_) => {}
                    }
                }
                match socket.recv_from(&mut buf) {
                    Ok((n, source)) => {
                        if let Ok(contents) = buf[..n].try_into() {
                            let _ = rtc.handle_input(Input::Receive(
                                Instant::now(),
                                Receive {
                                    proto: Protocol::Udp,
                                    source,
                                    destination: local,
                                    contents,
                                },
                            ));
                        }
                    }
                    Err(_) => {
                        let _ = rtc.handle_input(Input::Timeout(Instant::now()));
                    }
                }
            }
        });
        TestViewer {
            stop,
            thread: Some(thread),
            media: rx,
        }
    }
}

impl Drop for TestViewer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn find_index(data: &[u8]) -> Option<u32> {
    let pos = data.windows(3).position(|w| w == MAGIC)?;
    let hex = std::str::from_utf8(data.get(pos + 3..pos + MARKER_LEN)?).ok()?;
    u32::from_str_radix(hex, 16).ok()
}

// ---------- 받은 프레임 분석 ----------

pub fn video(received: &[Received]) -> Vec<&Received> {
    received
        .iter()
        .filter(|r| r.kind == MediaKind::Video)
        .collect()
}

pub fn audio(received: &[Received]) -> Vec<&Received> {
    received
        .iter()
        .filter(|r| r.kind == MediaKind::Audio)
        .collect()
}

/// 같은 tick의 비디오와 오디오 캡처 시각(시청 쪽 SR 기준) 차이(ms).
pub fn av_diffs(received: &[Received]) -> Vec<f64> {
    let audio = audio(received);
    let mut diffs = Vec::new();
    for v in video(received) {
        let (Some(vi), Some(vc)) = (v.index, v.capture) else {
            continue;
        };
        if let Some(a) = audio.iter().find(|a| a.index == Some(vi))
            && let Some(ac) = a.capture
        {
            let d = match vc.duration_since(ac) {
                Ok(d) => d.as_secs_f64(),
                Err(e) => -e.duration().as_secs_f64(),
            };
            diffs.push(d * 1000.0);
        }
    }
    diffs
}

/// A/V 차이의 (평균, 최대 절댓값, 쌍 수).
pub fn av_summary(diffs: &[f64]) -> (f64, f64, usize) {
    let max = diffs.iter().fold(0f64, |m, d| m.max(d.abs()));
    let mean = diffs.iter().sum::<f64>() / diffs.len().max(1) as f64;
    (mean, max, diffs.len())
}

/// 받은 프레임이 충분한지, 내용이 보존됐는지, A/V 동기가 맞는지 확인하고 요약을 출력한다.
pub fn assert_stream_ok(label: &str, received: &[Received], min_video: usize, min_audio: usize) {
    let v = video(received);
    let a = audio(received);
    let diffs = av_diffs(received);
    let (mean, max, pairs) = av_summary(&diffs);
    eprintln!(
        "[{label}] 비디오 {}개, 오디오 {}개, A/V 차이 평균 {mean:.2}ms 최대 {max:.2}ms ({pairs}쌍)",
        v.len(),
        a.len()
    );
    assert!(
        v.len() >= min_video,
        "[{label}] 비디오 프레임이 적습니다: {}",
        v.len()
    );
    assert!(
        a.len() >= min_audio,
        "[{label}] 오디오 패킷이 적습니다: {}",
        a.len()
    );
    assert!(
        received.iter().all(|r| r.index.is_some()),
        "[{label}] 프레임 내용이 바뀌었습니다"
    );
    assert!(
        pairs >= 10,
        "[{label}] SR 이후 짝지은 프레임이 적습니다: {pairs}"
    );
    assert!(max < 5.0, "[{label}] A/V 차이가 큽니다: 최대 {max:.2}ms");
}

/// 앞으로 올 키프레임 요청 표시를 비운다.
pub fn clear_keyframe_request(publisher: &Publisher) {
    while publisher.take_keyframe_request() {}
}

pub fn path_exists(p: &Path) -> bool {
    p.exists()
}

/// `Future`를 제한 시간 안에 끝낸다.
pub async fn within<T>(what: &str, fut: impl Future<Output = T>) -> T {
    tokio::time::timeout(WAIT, fut)
        .await
        .unwrap_or_else(|_| panic!("시간 안에 끝나지 않았습니다: {what}"))
}
