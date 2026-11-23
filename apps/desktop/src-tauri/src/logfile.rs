//! 배포 빌드의 로그 파일(D-088).
//!
//! 배포 빌드는 `windows_subsystem = "windows"`라 콘솔이 없어서 stdout 로그가 어디에도 남지 않는다.
//! tracing을 `<앱 로컬 데이터>/logs/dongsi.log`에도 쓴다(프로필이면 `profiles/<이름>/logs/`, `paths::log_dir`).
//!
//! - tracing은 `run()` 첫머리, 폴더를 알기 전에 시작한다. 그 전 줄은 메모리에 [`PENDING_CAP`]까지 모아 두었다가
//!   `setup`에서 [`LogSink::attach`]로 파일을 열 때 앞에 쓴다(버리지 않는 이유: 시작 직후의 줄이 원인일 때가 많다).
//! - 시작할 때 있던 `dongsi.log`는 `dongsi.old.log`로 바꿔(덮어씀) 직전 실행 하나를 남긴다.
//! - 한 실행에 [`FILE_CAP`]을 넘으면 안내 한 줄을 쓰고 더 쓰지 않는다.
//! - 새 크레이트 없이 std `File` + `Mutex` + tracing-subscriber `MakeWriter`로 한다.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

pub const LOG_FILE: &str = "dongsi.log";
pub const OLD_LOG_FILE: &str = "dongsi.old.log";
/// 파일을 열기 전에 모아 두는 최대 바이트.
pub const PENDING_CAP: usize = 256 * 1024;
/// 한 실행의 로그 파일 최대 바이트.
pub const FILE_CAP: u64 = 20 * 1024 * 1024;

enum State {
    /// 아직 파일이 없다. 줄을 모은다.
    Pending { buf: Vec<u8>, dropped: bool },
    Open {
        file: File,
        written: u64,
        capped: bool,
    },
    /// 파일을 못 열었다. 더 쓰지 않는다.
    Off,
}

struct Inner {
    state: State,
    pending_cap: usize,
    file_cap: u64,
}

/// 로그 파일 싱크. 복제해 `MakeWriter`로 쓴다.
#[derive(Clone)]
pub struct LogSink(Arc<Mutex<Inner>>);

impl Default for LogSink {
    fn default() -> Self {
        Self::with_caps(PENDING_CAP, FILE_CAP)
    }
}

impl LogSink {
    pub fn with_caps(pending_cap: usize, file_cap: u64) -> Self {
        Self(Arc::new(Mutex::new(Inner {
            state: State::Pending {
                buf: Vec::new(),
                dropped: false,
            },
            pending_cap,
            file_cap,
        })))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 로그 폴더를 만들고, 있던 `dongsi.log`를 `dongsi.old.log`로 바꾼 뒤 새 파일을 열어
    /// 모아 둔 줄을 쓴다. 실패하면 이후 줄은 버리고(`Off`) 오류를 돌려준다. 파일 경로를 돌려준다.
    pub fn attach(&self, dir: &Path) -> io::Result<PathBuf> {
        let mut inner = self.lock();
        if !matches!(inner.state, State::Pending { .. }) {
            return Err(io::Error::other(
                "이미 로그 파일에 연결했거나 열지 못했습니다",
            ));
        }
        let opened = open_log(dir);
        let State::Pending { buf, dropped } = std::mem::replace(&mut inner.state, State::Off)
        else {
            unreachable!("위에서 확인했다");
        };
        let (mut file, path) = opened?;
        let mut written = 0u64;
        file.write_all(&buf)?;
        written += buf.len() as u64;
        if dropped {
            let note = "(파일을 열기 전 로그가 너무 많아 일부를 버렸습니다)\n";
            file.write_all(note.as_bytes())?;
            written += note.len() as u64;
        }
        inner.state = State::Open {
            file,
            written,
            capped: false,
        };
        Ok(path)
    }

    fn write_bytes(&self, data: &[u8]) {
        let mut inner = self.lock();
        let (pending_cap, file_cap) = (inner.pending_cap, inner.file_cap);
        match &mut inner.state {
            State::Pending { buf, dropped } => {
                if buf.len() + data.len() <= pending_cap {
                    buf.extend_from_slice(data);
                } else {
                    *dropped = true;
                }
            }
            State::Open {
                file,
                written,
                capped,
            } => {
                if *capped {
                    return;
                }
                if *written + data.len() as u64 > file_cap {
                    *capped = true;
                    let note = format!(
                        "(로그가 {}MB를 넘어 더 쓰지 않습니다. 앱을 다시 시작하면 새 파일에 씁니다)\n",
                        file_cap / (1024 * 1024)
                    );
                    let _ = file.write_all(note.as_bytes());
                    return;
                }
                if file.write_all(data).is_ok() {
                    *written += data.len() as u64;
                }
            }
            State::Off => {}
        }
    }
}

/// `dir`을 만들고 이전 로그를 `.old`로 돌린 뒤 새 로그 파일을 연다.
fn open_log(dir: &Path) -> io::Result<(File, PathBuf)> {
    fs::create_dir_all(dir)?;
    let path = dir.join(LOG_FILE);
    if path.exists() {
        let old = dir.join(OLD_LOG_FILE);
        // Windows의 rename은 대상이 있으면 실패한다.
        let _ = fs::remove_file(&old);
        fs::rename(&path, &old)?;
    }
    let file = File::create(&path)?;
    Ok((file, path))
}

/// tracing이 한 줄씩 받아 쓰는 writer.
pub struct SinkWriter(LogSink);

impl Write for SinkWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write_bytes(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for LogSink {
    type Writer = SinkWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SinkWriter(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 시험마다 따로 쓰는 임시 폴더(끝나면 지운다).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("dongsi-logfile-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_line(sink: &LogSink, line: &str) {
        sink.make_writer().write_all(line.as_bytes()).unwrap();
    }

    #[test]
    fn lines_before_attach_are_written_first() {
        let tmp = TempDir::new("pending");
        let sink = LogSink::default();
        write_line(&sink, "첫 줄\n");
        write_line(&sink, "둘째 줄\n");
        let path = sink.attach(&tmp.0.join("logs")).unwrap();
        write_line(&sink, "셋째 줄\n");
        assert_eq!(path, tmp.0.join("logs").join(LOG_FILE));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "첫 줄\n둘째 줄\n셋째 줄\n"
        );
    }

    #[test]
    fn previous_run_is_kept_as_old_and_older_ones_are_overwritten() {
        let tmp = TempDir::new("rotate");
        let dir = tmp.0.join("logs");
        for run in ["첫째 실행\n", "둘째 실행\n", "셋째 실행\n"] {
            let sink = LogSink::default();
            sink.attach(&dir).unwrap();
            write_line(&sink, run);
        }
        assert_eq!(
            fs::read_to_string(dir.join(LOG_FILE)).unwrap(),
            "셋째 실행\n"
        );
        assert_eq!(
            fs::read_to_string(dir.join(OLD_LOG_FILE)).unwrap(),
            "둘째 실행\n"
        );
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
    }

    #[test]
    fn stops_writing_after_the_size_cap_with_one_note() {
        let tmp = TempDir::new("cap");
        let sink = LogSink::with_caps(1024, 100);
        let path = sink.attach(&tmp.0).unwrap();
        for _ in 0..10 {
            write_line(&sink, "0123456789\n"); // 11바이트
        }
        write_line(&sink, "한도 뒤의 줄\n");
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("0123456789\n").count(), 9, "{text}"); // 99바이트까지
        assert_eq!(text.matches("더 쓰지 않습니다").count(), 1, "{text}");
        assert!(!text.contains("한도 뒤의 줄"), "{text}");
    }

    #[test]
    fn too_many_lines_before_attach_are_dropped_with_a_note() {
        let tmp = TempDir::new("pending-cap");
        let sink = LogSink::with_caps(30, FILE_CAP);
        write_line(&sink, "0123456789\n");
        write_line(&sink, "0123456789\n");
        write_line(&sink, "넘치는 줄\n");
        let path = sink.attach(&tmp.0).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("0123456789\n0123456789\n"), "{text}");
        assert!(
            !text.contains("넘치는 줄") && text.contains("일부를 버렸습니다"),
            "{text}"
        );
    }

    #[test]
    fn attach_failure_drops_later_lines_without_panicking() {
        let tmp = TempDir::new("fail");
        fs::create_dir_all(&tmp.0).unwrap();
        // 폴더 자리에 파일이 있어 폴더를 만들 수 없다.
        let blocker = tmp.0.join("logs");
        fs::write(&blocker, "x").unwrap();
        let sink = LogSink::default();
        write_line(&sink, "앞 줄\n");
        assert!(sink.attach(&blocker).is_err());
        write_line(&sink, "뒷 줄\n");
        assert!(sink.attach(&tmp.0.join("again")).is_err());
    }
}
