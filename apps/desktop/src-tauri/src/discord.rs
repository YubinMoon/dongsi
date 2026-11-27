//! Discord 연동(공식 Rich Presence, 로컬 IPC).
//!
//! - 방송하는 동안 Discord 프로필과 음성 채널에 "dongsi 하는 중", 시청자 수, 방송 시간을 띄운다.
//! - 그 상태에 공개 파티와 참가 비밀값(방송 초대를 줄인 것, [`Invite::join_secret`])을 넣는다. 친구와 같은 음성 채널
//!   사람에게 "참가" 단추가 뜨고, 누르면 그 사람의 dongsi가 `ACTIVITY_JOIN`으로 비밀값을 받아 서버를 저장하고
//!   시청 화면을 연다([`accept_join`]). 비밀번호는 싣지 않는다(비밀번호 방송이면 시청 화면이 묻는다).
//! - dongsi가 꺼져 있으면 Discord가 등록된 실행 명령([`register_launch_command`])으로 띄우고 같은 이벤트를 보낸다.
//!
//! 이 PC에서 돌고 있는 Discord 앱에 로컬 IPC(`\\.\pipe\discord-ipc-N`)로 붙는다. 로그인이나 토큰은 쓰지 않고 Developer
//! Portal에 등록한 앱 ID([`CLIENT_ID`])만 보낸다. 참가 이벤트를 받으려고 앱이 켜져 있는 동안 연결을 유지하고, Discord가
//! 꺼져 있으면 조용히 기다렸다가 다시 붙는다. 사용자는 Discord 설정의 "활동 상태 공유"로 끌 수 있다.
//!
//! 프레임은 `op`(u32 LE)·길이(u32 LE)·JSON이다. 핸드셰이크(op 0) 뒤 `READY`를 받고, `SUBSCRIBE`·`SET_ACTIVITY`(op 1)를
//! 보낸다. 상태를 지울 때는 `activity` 없이 `SET_ACTIVITY`를 보낸다.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use dongsi_core::Invite;
use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager, State};
use tracing::{debug, info, warn};

use crate::commands::AppState;

/// Developer Portal의 앱 "dongsi" ID. 비밀값이 아니다.
pub const CLIENT_ID: &str = "1575794656111890554";

/// Discord가 여는 파이프 번호 범위(Discord를 여러 개 띄우면 번호가 늘어난다).
const PIPES: u32 = 10;
/// 이 번호의 파이프에만 붙는다. 한 PC에서 Discord를 둘(예: 정식판과 Canary) 띄우고 참가 단추를 시험할 때 쓴다.
const PIPE_ENV: &str = "DONGSI_DISCORD_PIPE";
/// 받는 프레임 크기 상한(정상 응답은 몇 KB).
const MAX_FRAME: u32 = 1 << 20;

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;

/// 파티 공개 범위: 친구와 같은 음성 채널 사람은 요청 없이 바로 참가한다(Discord가 flags 64·128을 붙인다).
const PARTY_PUBLIC: u32 = 1;

/// Discord "참가" 단추에 싣는 값.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Join {
    /// 방송마다 다른 파티 ID.
    pub party_id: String,
    /// [`Invite::join_secret`].
    pub secret: String,
}

/// Discord에 띄울 방송 상태.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub viewers: u32,
    /// 방송 하나에 들어올 수 있는 시청자 수. 다 차면 Discord가 참가 단추를 끈다.
    pub max_viewers: u32,
    /// 방송 시작 시각(유닉스 초). Discord가 경과 시간을 센다.
    pub started_at: u64,
    pub join: Option<Join>,
}

impl Activity {
    fn to_json(&self) -> Value {
        let mut v = json!({
            "details": "방송 중",
            "state": format!("시청자 {}명", self.viewers),
            "timestamps": { "start": self.started_at * 1000 },
            "instance": false,
        });
        if let Some(join) = &self.join {
            v["party"] = json!({
                "id": join.party_id,
                "size": [self.viewers, self.max_viewers.max(self.viewers)],
                "privacy": PARTY_PUBLIC,
            });
            v["secrets"] = json!({ "join": join.secret });
        }
        v
    }
}

/// 다시 붙기와 상태 바꾸기 간격.
#[derive(Debug, Clone, Copy)]
struct Pace {
    /// Discord에 붙지 못했거나 연결이 끊겼을 때 다시 해 보는 간격.
    retry: Duration,
    /// 상태를 바꾸는 최소 간격. Discord는 20초에 5번까지 받는다.
    min_interval: Duration,
    /// Discord가 보낸 이벤트를 확인하는 간격.
    poll: Duration,
}

const PACE: Pace = Pace {
    retry: Duration::from_secs(15),
    min_interval: Duration::from_secs(4),
    poll: Duration::from_millis(250),
};

/// Discord 연결 스레드의 손잡이. 앱이 시작할 때 [`Presence::start`]로 스레드를 띄운다(시험에서는 띄우지 않는다).
/// 같은 값을 여러 번 보내도 스레드가 한 번만 바꾼다.
pub struct Presence {
    tx: Sender<Option<Activity>>,
    rx: Mutex<Option<Receiver<Option<Activity>>>>,
}

impl Default for Presence {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx: Mutex::new(Some(rx)),
        }
    }
}

impl Presence {
    /// 연결 스레드를 띄운다. `on_join`은 "참가" 단추로 받은 비밀값을 받는다. 두 번째 부르기부터는 아무것도 하지 않는다.
    pub fn start(&self, on_join: impl FnMut(String) + Send + 'static) {
        let Some(rx) = self.rx.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return;
        };
        let spawned = std::thread::Builder::new()
            .name("dongsi-discord".into())
            .spawn(move || run(rx, connect_discord, PACE, on_join));
        if let Err(e) = spawned {
            warn!("Discord 연결 스레드를 띄우지 못했습니다: {e}");
        }
    }

    pub fn show(&self, activity: Activity) {
        let _ = self.tx.send(Some(activity));
    }

    /// 상태를 지운다(연결은 남겨 참가 이벤트를 계속 받는다).
    pub fn clear(&self) {
        let _ = self.tx.send(None);
    }
}

/// 연결 스레드. Discord에 붙어 있으면서 마지막으로 받은 값(`desired`)을 띄우고, 참가 이벤트를 `on_join`에 넘긴다.
/// 값이 빨리 바뀌면 `min_interval`마다 마지막 값만 보내고, Discord가 없으면 `retry`마다 다시 붙어 본다.
/// 손잡이가 사라지면 연결을 닫고 끝난다.
fn run<S, C>(
    rx: Receiver<Option<Activity>>,
    mut connect: C,
    pace: Pace,
    mut on_join: impl FnMut(String),
) where
    S: Pipe,
    C: FnMut() -> io::Result<Ipc<S>>,
{
    let mut desired: Option<Activity> = None;
    // Discord에 지금 떠 있는 값. 새로 붙으면 아무것도 없다.
    let mut shown: Option<Activity> = None;
    let mut ipc: Option<Ipc<S>> = None;
    let mut connect_at = Instant::now();
    let mut send_at = Instant::now();
    loop {
        match rx.recv_timeout(pace.poll) {
            Ok(v) => desired = rx.try_iter().last().unwrap_or(v),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if ipc.is_none() && Instant::now() >= connect_at {
            match connect() {
                Ok(mut c) => {
                    if let Err(e) = c.subscribe("ACTIVITY_JOIN") {
                        warn!("Discord 참가 이벤트를 구독하지 못했습니다: {e}");
                    }
                    ipc = Some(c);
                    shown = None;
                }
                Err(e) => {
                    debug!("Discord에 붙지 못했습니다: {e}");
                    connect_at = Instant::now() + pace.retry;
                }
            }
        }
        let Some(conn) = ipc.as_mut() else {
            continue;
        };
        let synced = sync(conn, &desired, &mut shown, &mut send_at, pace);
        for secret in conn.joins.drain(..) {
            on_join(secret);
        }
        if let Err(e) = synced {
            warn!("Discord 연결이 끊겼습니다: {e}");
            ipc = None;
            shown = None;
            connect_at = Instant::now() + pace.retry;
        }
    }
}

/// 쌓인 이벤트를 읽고, 띄울 값이 바뀌었으면(간격이 지났으면) 보낸다.
fn sync<S: Pipe>(
    conn: &mut Ipc<S>,
    desired: &Option<Activity>,
    shown: &mut Option<Activity>,
    send_at: &mut Instant,
    pace: Pace,
) -> io::Result<()> {
    conn.poll()?;
    if desired != shown && Instant::now() >= *send_at {
        conn.set_activity(desired.as_ref())?;
        if desired.is_none() {
            debug!("Discord 상태를 지웠습니다");
        }
        *shown = desired.clone();
        *send_at = Instant::now() + pace.min_interval;
    }
    Ok(())
}

/// 이 PC의 Discord 앱에 붙는다. 파이프를 차례로 열어 핸드셰이크가 되는 첫 곳을 쓴다.
fn connect_discord() -> io::Result<Ipc<File>> {
    let pipes = match std::env::var(PIPE_ENV)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
    {
        Some(n) if n < PIPES => n..n + 1,
        _ => 0..PIPES,
    };
    let mut last = io::Error::new(io::ErrorKind::NotFound, "Discord가 실행 중이 아닙니다");
    for n in pipes {
        let pipe = format!(r"\\.\pipe\discord-ipc-{n}");
        match OpenOptions::new().read(true).write(true).open(&pipe) {
            Ok(f) => match Ipc::handshake(f, CLIENT_ID) {
                Ok(ipc) => {
                    info!(pipe, "Discord에 붙었습니다");
                    return Ok(ipc);
                }
                Err(e) => last = e,
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// 막히지 않고 읽을 수 있는 바이트 수를 아는 스트림(이벤트를 기다리지 않고 확인한다).
trait Pipe: Read + Write {
    fn available(&mut self) -> io::Result<usize>;
}

impl Pipe for File {
    #[cfg(windows)]
    fn available(&mut self) -> io::Result<usize> {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Pipes::PeekNamedPipe;

        let mut avail = 0u32;
        // SAFETY: 이 File이 가진 열린 파이프 핸들이고, 받는 포인터는 지역 변수다.
        unsafe {
            PeekNamedPipe(
                HANDLE(self.as_raw_handle()),
                None,
                0,
                None,
                Some(&mut avail),
                None,
            )
        }
        .map_err(io::Error::other)?;
        Ok(avail as usize)
    }

    #[cfg(not(windows))]
    fn available(&mut self) -> io::Result<usize> {
        Ok(0)
    }
}

/// Discord IPC 연결 하나. 명령을 보내고 그 응답을 기다리는 동안 온 참가 이벤트는 `joins`에 모은다.
struct Ipc<S> {
    stream: S,
    nonce: u64,
    joins: Vec<String>,
}

impl<S: Pipe> Ipc<S> {
    /// 핸드셰이크를 보내고 `READY`를 기다린다. 앱 ID가 틀리면 Discord가 CLOSE를 보낸다.
    fn handshake(stream: S, client_id: &str) -> io::Result<Self> {
        let mut ipc = Self {
            stream,
            nonce: 0,
            joins: Vec::new(),
        };
        ipc.write_json(OP_HANDSHAKE, &json!({ "v": 1, "client_id": client_id }))?;
        let ready = ipc.read_frame()?;
        if ready["evt"] != "READY" {
            return Err(io::Error::other(format!("READY 대신 {ready}")));
        }
        Ok(ipc)
    }

    fn subscribe(&mut self, evt: &str) -> io::Result<()> {
        self.command(json!({ "cmd": "SUBSCRIBE", "evt": evt, "args": {} }))
    }

    /// 상태를 띄운다. `None`이면 지운다.
    fn set_activity(&mut self, activity: Option<&Activity>) -> io::Result<()> {
        let mut args = json!({ "pid": std::process::id() });
        if let Some(a) = activity {
            args["activity"] = a.to_json();
        }
        self.command(json!({ "cmd": "SET_ACTIVITY", "args": args }))
    }

    /// 쌓인 프레임(이벤트, PING)을 모두 처리한다. 쌓인 것이 없으면 바로 돌아온다.
    fn poll(&mut self) -> io::Result<()> {
        while self.stream.available()? > 0 {
            let frame = self.read_frame()?;
            self.dispatch(&frame);
        }
        Ok(())
    }

    /// 명령을 보내고 그 nonce의 응답을 기다린다. Discord가 ERROR로 답하면 오류다.
    fn command(&mut self, mut body: Value) -> io::Result<()> {
        self.nonce += 1;
        let nonce = self.nonce.to_string();
        body["nonce"] = json!(nonce);
        self.write_json(OP_FRAME, &body)?;
        loop {
            let reply = self.read_frame()?;
            if reply["nonce"] != nonce.as_str() {
                self.dispatch(&reply);
                continue;
            }
            if reply["evt"] == "ERROR" {
                return Err(io::Error::other(format!("Discord 오류: {}", reply["data"])));
            }
            return Ok(());
        }
    }

    /// 응답이 아닌 프레임: 참가 이벤트면 비밀값을 모은다.
    fn dispatch(&mut self, frame: &Value) {
        if frame["cmd"] == "DISPATCH"
            && frame["evt"] == "ACTIVITY_JOIN"
            && let Some(secret) = frame["data"]["secret"].as_str()
        {
            self.joins.push(secret.to_owned());
        }
    }

    fn write_json(&mut self, op: u32, body: &Value) -> io::Result<()> {
        self.write_frame(op, body.to_string().as_bytes())
    }

    /// 머리와 본문을 한 번에 쓴다.
    fn write_frame(&mut self, op: u32, body: &[u8]) -> io::Result<()> {
        let len = u32::try_from(body.len()).map_err(io::Error::other)?;
        let mut buf = Vec::with_capacity(8 + body.len());
        buf.extend_from_slice(&op.to_le_bytes());
        buf.extend_from_slice(&len.to_le_bytes());
        buf.extend_from_slice(body);
        self.stream.write_all(&buf)?;
        self.stream.flush()
    }

    /// 다음 프레임(op 1)을 읽는다. PING에는 PONG으로 답하고, CLOSE면 오류다.
    fn read_frame(&mut self) -> io::Result<Value> {
        loop {
            let mut head = [0u8; 8];
            self.stream.read_exact(&mut head)?;
            let op = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
            let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
            if len > MAX_FRAME {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "프레임이 너무 큽니다",
                ));
            }
            let mut body = vec![0u8; len as usize];
            self.stream.read_exact(&mut body)?;
            match op {
                OP_FRAME => {
                    return serde_json::from_slice(&body)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e));
                }
                OP_PING => self.write_frame(OP_PONG, &body)?,
                OP_CLOSE => {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        format!(
                            "Discord가 연결을 닫았습니다: {}",
                            String::from_utf8_lossy(&body)
                        ),
                    ));
                }
                _ => {}
            }
        }
    }
}

// ---------- 참가 단추로 방송 열기 ----------

/// "참가" 단추로 연 방송. 프런트엔드가 [`take_discord_join`]으로 가져가 시청 화면을 연다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinTarget {
    pub server_id: String,
    pub broadcaster_id: String,
}

/// 아직 프런트엔드가 가져가지 않은 참가(Discord가 dongsi를 띄웠다면 첫 화면이 준비되기 전에 온다).
#[derive(Default)]
pub struct PendingJoin(Mutex<Option<JoinTarget>>);

/// 참가 비밀값을 받았을 때: 서버를 저장하고(지문이 같은 서버가 있으면 그대로 쓴다) 메인 창에 시청 화면을 열게 한다.
pub fn accept_join(app: &AppHandle, secret: &str) {
    let invite = match Invite::from_join_secret(secret) {
        Ok(invite) => invite,
        Err(e) => {
            warn!("Discord 참가 정보를 읽지 못했습니다: {e}");
            return;
        }
    };
    let Some(broadcaster) = invite.broadcaster().copied() else {
        return;
    };
    let server = {
        let state = app.state::<AppState>();
        let mut store = state.lock();
        let existing = store
            .state()
            .servers
            .iter()
            .find(|s| s.fingerprint == Some(*invite.fingerprint()))
            .map(|s| s.id);
        match existing {
            Some(id) => id,
            None => match store.update(|s| Ok(s.servers.add_invite(&invite))) {
                Ok(outcome) => outcome.id(),
                Err(e) => {
                    warn!("Discord 참가로 서버를 저장하지 못했습니다: {e}");
                    return;
                }
            },
        }
    };
    info!(%server, %broadcaster, "Discord 참가 단추로 방송을 엽니다");
    *app.state::<PendingJoin>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(JoinTarget {
        server_id: server.to_string(),
        broadcaster_id: broadcaster.to_string(),
    });
    let _ = app.emit("servers-changed", json!({}));
    if let Some(main) = app.get_webview_window("main") {
        // 숨겨져 있으면 시작하는 중이다. 첫 화면이 준비되면 보이고(splash.rs) 그때 가져간다.
        if main.is_visible().unwrap_or(false) {
            let _ = main.unminimize();
            let _ = main.set_focus();
        }
    }
    let _ = app.emit_to("main", "discord-join", ());
}

#[tauri::command]
pub fn take_discord_join(pending: State<'_, PendingJoin>) -> Option<JoinTarget> {
    pending.0.lock().unwrap_or_else(|e| e.into_inner()).take()
}

// ---------- Discord가 dongsi를 띄우는 명령 ----------

/// Discord가 꺼져 있는 dongsi를 "참가" 단추로 띄울 수 있게 실행 명령을 등록한다. Discord SDK의 RegisterCommand와 같은
/// 자리(`HKCU\Software\Classes\discord-<앱 ID>`)다. 설치 파일의 제거 단계가 지운다(`windows/installer-hooks.nsh`).
/// 배포 빌드만 부른다(개발 빌드가 설치한 앱의 등록을 덮지 않게).
pub fn register_launch_command() {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe.display().to_string(),
        Err(e) => {
            warn!("Discord 실행 명령을 등록하지 못했습니다: {e}");
            return;
        }
    };
    match write_launch_command(&format!(r"Software\Classes\discord-{CLIENT_ID}"), &exe) {
        Ok(()) => debug!(exe, "Discord 실행 명령을 등록했습니다"),
        Err(e) => warn!("Discord 실행 명령을 등록하지 못했습니다: {e}"),
    }
}

#[cfg(windows)]
fn write_launch_command(key: &str, exe: &str) -> io::Result<()> {
    let entries = [
        (
            key.to_owned(),
            None,
            format!("URL:Run game {CLIENT_ID} protocol"),
        ),
        (key.to_owned(), Some("URL Protocol"), String::new()),
        (format!(r"{key}\DefaultIcon"), None, format!("{exe},0")),
        (
            format!(r"{key}\shell\open\command"),
            None,
            format!("\"{exe}\""),
        ),
    ];
    for (sub, name, value) in entries {
        set_user_reg_string(&sub, name, &value)?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn write_launch_command(_key: &str, _exe: &str) -> io::Result<()> {
    Ok(())
}

/// `HKCU\<sub>`의 문자열 값을 쓴다(키가 없으면 만든다). `name`이 `None`이면 기본값이다.
#[cfg(windows)]
fn set_user_reg_string(sub: &str, name: Option<&str>, value: &str) -> io::Result<()> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};
    use windows::core::{HSTRING, PCWSTR};

    let sub = HSTRING::from(sub);
    let name = name.map(HSTRING::from);
    let name = name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
    let data: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
    let bytes = u32::try_from(data.len() * 2).map_err(io::Error::other)?;
    // SAFETY: 문자열들은 이 함수가 끝날 때까지 살아 있고, data는 NUL로 끝나는 UTF-16이다.
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &sub,
            name,
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            bytes,
        )
    }
    .ok()
    .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    fn frame(op: u32, body: &Value) -> Vec<u8> {
        let body = body.to_string();
        let mut out = op.to_le_bytes().to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body.as_bytes());
        out
    }

    /// 쓴 프레임을 (op, JSON)으로 나눈다.
    fn frames(mut bytes: &[u8]) -> Vec<(u32, Value)> {
        let mut out = Vec::new();
        while !bytes.is_empty() {
            let op = u32::from_le_bytes(bytes[..4].try_into().unwrap());
            let len = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
            out.push((op, serde_json::from_slice(&bytes[8..8 + len]).unwrap()));
            bytes = &bytes[8 + len..];
        }
        out
    }

    /// 미리 넣어 둔 바이트를 읽고, 쓴 바이트를 모은다.
    struct Script {
        input: io::Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Script {
        fn new(replies: &[Vec<u8>]) -> Self {
            Self {
                input: io::Cursor::new(replies.concat()),
                output: Vec::new(),
            }
        }
    }

    impl Read for Script {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Script {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Pipe for Script {
        fn available(&mut self) -> io::Result<usize> {
            Ok(self.input.get_ref().len() - self.input.position() as usize)
        }
    }

    fn ready() -> Vec<u8> {
        frame(
            OP_FRAME,
            &json!({ "cmd": "DISPATCH", "evt": "READY", "data": {} }),
        )
    }

    fn join_event(secret: &str) -> Vec<u8> {
        frame(
            OP_FRAME,
            &json!({ "cmd": "DISPATCH", "evt": "ACTIVITY_JOIN", "data": { "secret": secret }, "nonce": null }),
        )
    }

    fn activity(viewers: u32) -> Activity {
        Activity {
            viewers,
            max_viewers: 10,
            started_at: 1,
            join: None,
        }
    }

    #[test]
    fn activity_shows_viewers_elapsed_time_and_public_party() {
        let mut a = Activity {
            viewers: 3,
            max_viewers: 16,
            started_at: 1_700_000_000,
            join: None,
        };
        let v = a.to_json();
        assert_eq!(v["details"], "방송 중");
        assert_eq!(v["state"], "시청자 3명");
        assert_eq!(v["timestamps"]["start"], 1_700_000_000_000u64);
        assert!(v.get("party").is_none() && v.get("secrets").is_none());

        a.join = Some(Join {
            party_id: "p".into(),
            secret: "s".into(),
        });
        let v = a.to_json();
        assert_eq!(
            v["party"],
            json!({ "id": "p", "size": [3, 16], "privacy": 1 })
        );
        assert_eq!(v["secrets"], json!({ "join": "s" }));
        // 상한을 넘으면(전용 서버 설정이 더 크면) 꽉 찬 것으로 둔다.
        a.viewers = 20;
        assert_eq!(a.to_json()["party"]["size"], json!([20, 20]));
    }

    #[test]
    fn commands_answer_ping_collect_joins_and_wait_for_their_nonce() {
        let stream = Script::new(&[
            ready(),
            frame(OP_PING, &json!({ "p": 1 })),
            join_event("first"),
            frame(
                OP_FRAME,
                &json!({ "cmd": "SET_ACTIVITY", "evt": null, "nonce": "1" }),
            ),
            frame(
                OP_FRAME,
                &json!({ "cmd": "SET_ACTIVITY", "evt": null, "nonce": "2", "data": null }),
            ),
            join_event("second"),
        ]);
        let mut ipc = Ipc::handshake(stream, CLIENT_ID).unwrap();
        ipc.set_activity(Some(&activity(2))).unwrap();
        ipc.set_activity(None).unwrap();
        assert_eq!(ipc.joins, ["first"]);
        // 쌓인 이벤트는 poll이 읽는다.
        ipc.poll().unwrap();
        assert_eq!(ipc.joins, ["first", "second"]);

        let sent = frames(&ipc.stream.output);
        assert_eq!(sent[0].0, OP_HANDSHAKE);
        assert_eq!(sent[0].1, json!({ "v": 1, "client_id": CLIENT_ID }));
        assert_eq!(sent[1].0, OP_FRAME);
        assert_eq!(sent[1].1["cmd"], "SET_ACTIVITY");
        assert_eq!(sent[1].1["nonce"], "1");
        assert_eq!(sent[1].1["args"]["pid"], std::process::id());
        assert_eq!(sent[1].1["args"]["activity"]["state"], "시청자 2명");
        assert_eq!(sent[2], (OP_PONG, json!({ "p": 1 })));
        // 지우기는 activity 없이 보낸다.
        assert_eq!(sent[3].1["nonce"], "2");
        assert_eq!(sent[3].1["args"], json!({ "pid": std::process::id() }));
    }

    #[test]
    fn close_on_handshake_and_error_reply_are_errors() {
        let closed = Script::new(&[frame(
            OP_CLOSE,
            &json!({ "code": 4000, "message": "Invalid Client ID" }),
        )]);
        let e = Ipc::handshake(closed, "0").err().unwrap();
        assert!(e.to_string().contains("Invalid Client ID"), "{e}");

        let stream = Script::new(&[
            ready(),
            frame(
                OP_FRAME,
                &json!({ "evt": "ERROR", "nonce": "1", "data": { "message": "bad" } }),
            ),
        ]);
        let mut ipc = Ipc::handshake(stream, CLIENT_ID).unwrap();
        assert!(ipc.set_activity(Some(&activity(0))).is_err());
    }

    /// 가짜 Discord가 받은 명령. `Show(n)`은 시청자 n명 상태, `Clear`는 지우기.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Seen {
        Subscribe(String),
        Show(u32),
        Clear,
    }

    /// 보낸 프레임마다 그 nonce로 바로 답하는 가짜 Discord. 받은 명령을 `log`에 남기고, 닫히면 `closed`를 센다.
    /// `push`에 넣은 프레임은 다음 읽기 때 이벤트로 보낸다.
    struct FakeDiscord {
        pending: VecDeque<u8>,
        push: Arc<Mutex<Vec<u8>>>,
        log: Arc<Mutex<Vec<Seen>>>,
        closed: Arc<AtomicU32>,
    }

    impl FakeDiscord {
        fn take_pushed(&mut self) {
            let pushed = std::mem::take(&mut *self.push.lock().unwrap());
            self.pending.extend(pushed);
        }
    }

    impl Read for FakeDiscord {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.take_pushed();
            let n = buf.len().min(self.pending.len());
            for b in buf.iter_mut().take(n) {
                *b = self.pending.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    impl Write for FakeDiscord {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            for (op, v) in frames(buf) {
                let reply = if op == OP_HANDSHAKE {
                    ready()
                } else {
                    let seen = match v["cmd"].as_str().unwrap() {
                        "SUBSCRIBE" => Seen::Subscribe(v["evt"].as_str().unwrap().to_owned()),
                        _ => match v["args"]["activity"]["state"].as_str() {
                            Some(state) => {
                                let n = state.trim_start_matches("시청자 ").trim_end_matches('명');
                                Seen::Show(n.parse().unwrap())
                            }
                            None => Seen::Clear,
                        },
                    };
                    self.log.lock().unwrap().push(seen);
                    frame(OP_FRAME, &json!({ "evt": null, "nonce": v["nonce"] }))
                };
                self.pending.extend(reply);
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Pipe for FakeDiscord {
        fn available(&mut self) -> io::Result<usize> {
            self.take_pushed();
            Ok(self.pending.len())
        }
    }

    impl Drop for FakeDiscord {
        fn drop(&mut self) {
            self.closed.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(3);
        while !ok() {
            assert!(Instant::now() < end, "기다리다 시간 초과: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn worker_retries_coalesces_clears_and_hands_over_joins() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let push = Arc::new(Mutex::new(Vec::new()));
        let closed = Arc::new(AtomicU32::new(0));
        let attempts = Arc::new(AtomicU32::new(0));
        let connect = {
            let (log, push, closed, attempts) =
                (log.clone(), push.clone(), closed.clone(), attempts.clone());
            move || {
                // 처음 한 번은 Discord가 꺼져 있다.
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err(io::Error::new(io::ErrorKind::NotFound, "없음"));
                }
                let fake = FakeDiscord {
                    pending: VecDeque::new(),
                    push: push.clone(),
                    log: log.clone(),
                    closed: closed.clone(),
                };
                Ipc::handshake(fake, CLIENT_ID)
            }
        };
        let pace = Pace {
            retry: Duration::from_millis(30),
            min_interval: Duration::from_millis(200),
            poll: Duration::from_millis(5),
        };
        let joins = Arc::new(Mutex::new(Vec::<String>::new()));
        let on_join = {
            let joins = joins.clone();
            move |s: String| joins.lock().unwrap().push(s)
        };
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || run(rx, connect, pace, on_join));
        let seen = || log.lock().unwrap().clone();

        // 방송하지 않아도 붙어서 참가 이벤트를 구독한다.
        wait_until("다시 붙어 구독", || {
            seen() == [Seen::Subscribe("ACTIVITY_JOIN".into())]
        });
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        push.lock().unwrap().extend(join_event("secret-1"));
        wait_until("참가 이벤트", || {
            *joins.lock().unwrap() == ["secret-1"]
        });

        // 같은 값은 다시 보내지 않고, 빨리 바뀐 값은 마지막 것만 보낸다.
        tx.send(Some(activity(0))).unwrap();
        wait_until("첫 상태", || seen().last() == Some(&Seen::Show(0)));
        tx.send(Some(activity(0))).unwrap();
        tx.send(Some(activity(1))).unwrap();
        tx.send(Some(activity(2))).unwrap();
        wait_until("마지막 값", || seen().last() == Some(&Seen::Show(2)));
        assert_eq!(seen()[1..], [Seen::Show(0), Seen::Show(2)]);

        // 지우면 연결은 남기고 상태만 지운다.
        tx.send(None).unwrap();
        wait_until("지우기", || seen().last() == Some(&Seen::Clear));
        assert_eq!(closed.load(Ordering::SeqCst), 0);

        // 손잡이가 사라지면 연결을 닫고 끝난다.
        drop(tx);
        worker.join().unwrap();
        assert_eq!(closed.load(Ordering::SeqCst), 1);
    }

    /// 실행 명령을 시험용 키에 써 보고 지운다(HKCU를 건드린다).
    #[cfg(windows)]
    #[test]
    #[cfg_attr(
        not(feature = "full-tests"),
        ignore = "HKCU 레지스트리에 쓴다(전체 시험)"
    )]
    fn launch_command_is_written_to_the_registry() {
        let key = format!(r"Software\dongsi-test-{}", std::process::id());
        let exe = r"C:\dongsi 시험\dongsi.exe";
        write_launch_command(&key, exe).unwrap();
        let query = |sub: &str| {
            let out = std::process::Command::new("reg")
                .args(["query", &format!(r"HKCU\{sub}"), "/ve"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let command = query(&format!(r"{key}\shell\open\command"));
        let protocol = std::process::Command::new("reg")
            .args(["query", &format!(r"HKCU\{key}"), "/v", "URL Protocol"])
            .status()
            .unwrap();
        let _ = std::process::Command::new("reg")
            .args(["delete", &format!(r"HKCU\{key}"), "/f"])
            .status();
        // reg.exe는 콘솔 코드 페이지로 출력하므로 경로의 ASCII 부분과 따옴표만 본다.
        assert!(command.contains("dongsi.exe\""), "{command}");
        assert!(protocol.success());
    }

    /// 실제 Discord 앱에 방송 상태와 참가 단추를 띄워 본다. Discord가 켜져 있어야 한다.
    /// `DONGSI_DISCORD_HOLD_SECS`만큼 띄워 둔다(기본 0초).
    #[test]
    #[ignore = "실제 Discord 앱에 상태를 띄운다. 직접 볼 때만 --ignored로 돌린다"]
    fn shows_on_real_discord() {
        let mut ipc = connect_discord().expect("Discord에 붙지 못했습니다");
        ipc.subscribe("ACTIVITY_JOIN").unwrap();
        let started_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        ipc.set_activity(Some(&Activity {
            viewers: 3,
            max_viewers: 16,
            started_at,
            join: Some(Join {
                party_id: format!("dongsi-test-{started_at}"),
                secret: "dongsi-test".into(),
            }),
        }))
        .unwrap();
        let hold = std::env::var("DONGSI_DISCORD_HOLD_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        std::thread::sleep(Duration::from_secs(hold));
    }
}
