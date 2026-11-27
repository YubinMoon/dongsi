//! Discord Rich Presence: 방송하는 동안 Discord 프로필에 "dongsi 하는 중"과 시청자 수, 방송 시간을 띄운다.
//!
//! 이 PC에서 돌고 있는 Discord 앱에 로컬 IPC(`\\.\pipe\discord-ipc-N`)로 붙는 공식 방식이다. 로그인이나 토큰은 쓰지 않고
//! Developer Portal에 등록한 앱 ID([`CLIENT_ID`])만 보낸다. Discord가 꺼져 있으면 조용히 기다렸다가 다시 붙는다.
//! 연결을 닫으면 Discord가 그 상태를 지운다. 사용자는 Discord 설정의 "활동 상태 공유"로 끌 수 있다.
//!
//! 프레임은 `op`(u32 LE)·길이(u32 LE)·JSON이다. 핸드셰이크(op 0) 뒤 `READY`를 받고, `SET_ACTIVITY`(op 1)를 보낸다.

use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tracing::{debug, info, warn};

/// Developer Portal의 앱 "dongsi" ID. 비밀값이 아니다.
pub const CLIENT_ID: &str = "1575794656111890554";

/// Discord가 여는 파이프 번호 범위(Discord를 여러 개 띄우면 번호가 늘어난다).
const PIPES: u32 = 10;
/// 받는 프레임 크기 상한(정상 응답은 몇 KB).
const MAX_FRAME: u32 = 1 << 20;

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;

/// Discord에 띄울 방송 상태.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub viewers: u32,
    /// 방송 시작 시각(유닉스 초). Discord가 경과 시간을 센다.
    pub started_at: u64,
}

impl Activity {
    fn to_json(&self) -> Value {
        json!({
            "details": "방송 중",
            "state": format!("시청자 {}명", self.viewers),
            "timestamps": { "start": self.started_at * 1000 },
            "instance": false,
        })
    }
}

/// 다시 붙기와 상태 바꾸기 간격.
#[derive(Debug, Clone, Copy)]
struct Pace {
    /// Discord에 붙지 못했거나 연결이 끊겼을 때 다시 해 보는 간격.
    retry: Duration,
    /// 상태를 바꾸는 최소 간격. Discord는 20초에 5번까지 받는다.
    min_interval: Duration,
}

const PACE: Pace = Pace {
    retry: Duration::from_secs(15),
    min_interval: Duration::from_secs(4),
};

/// Discord 상태를 맡는 스레드의 손잡이. 처음 [`Presence::show`]를 부를 때 스레드를 띄운다.
/// 같은 값을 여러 번 보내도 스레드가 한 번만 바꾼다.
#[derive(Default)]
pub struct Presence {
    tx: Mutex<Option<Sender<Option<Activity>>>>,
}

impl Presence {
    pub fn show(&self, activity: Activity) {
        self.send(Some(activity));
    }

    /// 상태를 지운다(연결을 닫는다).
    pub fn clear(&self) {
        self.send(None);
    }

    fn send(&self, value: Option<Activity>) {
        let mut tx = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        if tx.is_none() {
            if value.is_none() {
                return;
            }
            let (s, r) = mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("dongsi-discord".into())
                .spawn(move || run(r, connect_discord, PACE));
            if let Err(e) = spawned {
                warn!("Discord 상태 스레드를 띄우지 못했습니다: {e}");
                return;
            }
            *tx = Some(s);
        }
        if let Some(s) = tx.as_ref() {
            let _ = s.send(value);
        }
    }
}

/// 상태 스레드. 마지막으로 받은 값(`desired`)을 Discord에 띄운다. 값이 빨리 바뀌면 `min_interval`마다 마지막 값만
/// 보내고, Discord가 없으면 `retry`마다 다시 붙어 본다. 손잡이가 사라지면 연결을 닫고 끝난다.
fn run<S, C>(rx: Receiver<Option<Activity>>, mut connect: C, pace: Pace)
where
    S: Read + Write,
    C: FnMut() -> io::Result<Ipc<S>>,
{
    let mut desired: Option<Activity> = None;
    let mut shown: Option<Activity> = None;
    let mut ipc: Option<Ipc<S>> = None;
    let mut next_at = Instant::now();
    loop {
        let got = if desired == shown {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            rx.recv_timeout(next_at.saturating_duration_since(Instant::now()))
        };
        match got {
            Ok(v) => desired = rx.try_iter().last().unwrap_or(v),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let Some(activity) = &desired else {
            if ipc.take().is_some() {
                debug!("Discord 상태를 지웠습니다");
            }
            shown = None;
            continue;
        };
        if shown.as_ref() == Some(activity) || Instant::now() < next_at {
            continue;
        }
        let conn = match ipc.as_mut() {
            Some(c) => c,
            None => match connect() {
                Ok(c) => ipc.insert(c),
                Err(e) => {
                    debug!("Discord에 붙지 못했습니다: {e}");
                    next_at = Instant::now() + pace.retry;
                    continue;
                }
            },
        };
        match conn.set_activity(activity) {
            Ok(()) => {
                shown = desired.clone();
                next_at = Instant::now() + pace.min_interval;
            }
            Err(e) => {
                warn!("Discord 상태를 바꾸지 못했습니다: {e}");
                ipc = None;
                shown = None;
                next_at = Instant::now() + pace.retry;
            }
        }
    }
}

/// 이 PC의 Discord 앱에 붙는다. 파이프를 차례로 열어 핸드셰이크가 되는 첫 곳을 쓴다.
fn connect_discord() -> io::Result<Ipc<std::fs::File>> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "Discord가 실행 중이 아닙니다");
    for n in 0..PIPES {
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

/// Discord IPC 연결 하나. 보내고 나서 그 응답을 바로 읽는다(응답을 쌓아 두지 않는다).
struct Ipc<S> {
    stream: S,
    nonce: u64,
}

impl<S: Read + Write> Ipc<S> {
    /// 핸드셰이크를 보내고 `READY`를 기다린다. 앱 ID가 틀리면 Discord가 CLOSE를 보낸다.
    fn handshake(stream: S, client_id: &str) -> io::Result<Self> {
        let mut ipc = Self { stream, nonce: 0 };
        ipc.write_json(OP_HANDSHAKE, &json!({ "v": 1, "client_id": client_id }))?;
        let ready = ipc.read_frame()?;
        if ready["evt"] != "READY" {
            return Err(io::Error::other(format!("READY 대신 {ready}")));
        }
        Ok(ipc)
    }

    fn set_activity(&mut self, activity: &Activity) -> io::Result<()> {
        self.nonce += 1;
        let nonce = self.nonce.to_string();
        self.write_json(
            OP_FRAME,
            &json!({
                "cmd": "SET_ACTIVITY",
                "args": { "pid": std::process::id(), "activity": activity.to_json() },
                "nonce": nonce,
            }),
        )?;
        loop {
            let reply = self.read_frame()?;
            if reply["nonce"] != nonce.as_str() {
                continue;
            }
            if reply["evt"] == "ERROR" {
                return Err(io::Error::other(format!("Discord 오류: {}", reply["data"])));
            }
            return Ok(());
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

    /// 다음 응답(op 1)을 읽는다. PING에는 PONG으로 답하고, CLOSE면 오류다.
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

    fn ready() -> Vec<u8> {
        frame(
            OP_FRAME,
            &json!({ "cmd": "DISPATCH", "evt": "READY", "data": {} }),
        )
    }

    #[test]
    fn activity_shows_broadcast_viewers_and_elapsed_time() {
        let a = Activity {
            viewers: 3,
            started_at: 1_700_000_000,
        };
        let v = a.to_json();
        assert_eq!(v["details"], "방송 중");
        assert_eq!(v["state"], "시청자 3명");
        assert_eq!(v["timestamps"]["start"], 1_700_000_000_000u64);
    }

    #[test]
    fn handshake_then_set_activity_answers_ping_and_waits_for_its_nonce() {
        let stream = Script::new(&[
            ready(),
            frame(OP_PING, &json!({ "p": 1 })),
            frame(
                OP_FRAME,
                &json!({ "cmd": "DISPATCH", "evt": "OTHER", "nonce": null }),
            ),
            frame(
                OP_FRAME,
                &json!({ "cmd": "SET_ACTIVITY", "evt": null, "nonce": "1" }),
            ),
        ]);
        let mut ipc = Ipc::handshake(stream, CLIENT_ID).unwrap();
        ipc.set_activity(&Activity {
            viewers: 2,
            started_at: 10,
        })
        .unwrap();
        let sent = frames(&ipc.stream.output);
        assert_eq!(sent[0].0, OP_HANDSHAKE);
        assert_eq!(sent[0].1, json!({ "v": 1, "client_id": CLIENT_ID }));
        assert_eq!(sent[1].0, OP_FRAME);
        assert_eq!(sent[1].1["cmd"], "SET_ACTIVITY");
        assert_eq!(sent[1].1["nonce"], "1");
        assert_eq!(sent[1].1["args"]["pid"], std::process::id());
        assert_eq!(sent[1].1["args"]["activity"]["state"], "시청자 2명");
        assert_eq!(sent[2], (OP_PONG, json!({ "p": 1 })));
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
        let a = Activity {
            viewers: 0,
            started_at: 0,
        };
        assert!(ipc.set_activity(&a).is_err());
    }

    /// 보낸 프레임마다 그 nonce로 바로 답하는 가짜 Discord. 받은 시청자 수를 `log`에 남기고, 닫히면 `closed`를 센다.
    struct FakeDiscord {
        pending: VecDeque<u8>,
        log: Arc<Mutex<Vec<u32>>>,
        closed: Arc<AtomicU32>,
    }

    impl Read for FakeDiscord {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
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
                    let viewers = v["args"]["activity"]["state"].as_str().unwrap();
                    let n = viewers.trim_start_matches("시청자 ").trim_end_matches('명');
                    self.log.lock().unwrap().push(n.parse().unwrap());
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
    fn worker_retries_coalesces_and_closes_on_clear() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let closed = Arc::new(AtomicU32::new(0));
        let attempts = Arc::new(AtomicU32::new(0));
        let connect = {
            let (log, closed, attempts) = (log.clone(), closed.clone(), attempts.clone());
            move || {
                // 처음 한 번은 Discord가 꺼져 있다.
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err(io::Error::new(io::ErrorKind::NotFound, "없음"));
                }
                let fake = FakeDiscord {
                    pending: VecDeque::new(),
                    log: log.clone(),
                    closed: closed.clone(),
                };
                Ipc::handshake(fake, CLIENT_ID)
            }
        };
        let pace = Pace {
            retry: Duration::from_millis(30),
            min_interval: Duration::from_millis(200),
        };
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || run(rx, connect, pace));
        let a = |viewers| {
            Some(Activity {
                viewers,
                started_at: 1,
            })
        };

        tx.send(a(0)).unwrap();
        wait_until("다시 붙어 첫 상태", || *log.lock().unwrap() == [0]);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);

        // 같은 값은 다시 보내지 않고, 빨리 바뀐 값은 마지막 것만 보낸다.
        tx.send(a(0)).unwrap();
        tx.send(a(1)).unwrap();
        tx.send(a(2)).unwrap();
        wait_until("마지막 값", || log.lock().unwrap().last() == Some(&2));
        assert_eq!(*log.lock().unwrap(), [0, 2]);

        // 지우면 연결을 닫는다(Discord가 상태를 지운다).
        tx.send(None).unwrap();
        wait_until("연결 닫기", || closed.load(Ordering::SeqCst) == 1);

        // 다시 보이면 새로 붙는다. 손잡이가 사라지면 연결을 닫고 끝난다.
        tx.send(a(5)).unwrap();
        wait_until("다시 보이기", || {
            log.lock().unwrap().last() == Some(&5)
        });
        drop(tx);
        worker.join().unwrap();
        assert_eq!(closed.load(Ordering::SeqCst), 2);
    }

    /// 실제 Discord 앱에 상태를 띄워 본다. Discord가 켜져 있어야 한다.
    /// `DONGSI_DISCORD_HOLD_SECS`만큼 띄워 둔다(기본 0초).
    #[test]
    #[ignore = "실제 Discord 앱에 상태를 띄운다. 직접 볼 때만 --ignored로 돌린다"]
    fn shows_on_real_discord() {
        let mut ipc = connect_discord().expect("Discord에 붙지 못했습니다");
        let started_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        ipc.set_activity(&Activity {
            viewers: 3,
            started_at,
        })
        .unwrap();
        let hold = std::env::var("DONGSI_DISCORD_HOLD_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        std::thread::sleep(Duration::from_secs(hold));
    }
}
