//! 송신 쪽 프레임 기록(진단용, 기본 꺼짐).
//!
//! 환경 변수 `DONGSI_MEDIA_TRACE=<폴더>`가 있으면 그 폴더에 CSV를 쓴다. 변수는 프로세스에서 처음 쓸 때
//! 한 번만 읽는다. 없거나 비어 있으면 [`video`]·[`audio`]가 `None`을 돌려주고, 기록하는 쪽은
//! 그 `Option`을 한 번 확인하는 것 말고는 아무것도 하지 않는다(시각 읽기·문자열 만들기 없음).
//!
//! - `video.csv`: 인코더 출력 패킷마다(+ 출력 없이 버려진 입력마다) 한 줄. [`VIDEO_HEADER`] 참고.
//! - `audio.csv`: Opus 패킷마다 한 줄. [`AUDIO_HEADER`] 참고.
//!
//! 파일은 이어 쓴다. 비어 있을 때만 머리글을 쓰고, 파이프라인·인코더를 새로 시작하면 `#`로 시작하는 구분 줄
//! (`# pipeline … qpc=<QPC> epoch_us=<UNIX µs>`)을 넣는다. 구분 줄의 `qpc`와 `epoch_us`는 같은 순간의
//! 두 시계 값이라 QPC 시각을 벽시계로 바꾸는 데 쓴다.
//!
//! 쓰기는 채널로 기록 스레드에 넘기고(캡처·인코딩 스레드는 막히지 않는다), 기록 스레드가 버퍼에
//! 모아 250ms마다 내보낸다.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dongsi_core::VideoCodec;

use crate::clock::qpc_now_100ns;

/// 기록 폴더를 정하는 환경 변수.
pub const ENV: &str = "DONGSI_MEDIA_TRACE";

/// `video.csv`의 열. 시각은 모두 QPC 100ns 단위다.
///
/// - `capture_qpc`: 짝지은 입력 프레임의 캡처 시각. `arrival_qpc`: 프레임이 파이프라인에 도착한 시각(패턴은 틱 처리 시각).
/// - `enc_in_qpc`: 인코더에 넣기 직전. `pkt_out_qpc`: 패킷을 받은 직후.
/// - `in_pts`: 인코더에 넣은 pts. `pkt_pts`·`pkt_dts`: 출력 패킷의 pts·dts(없으면 빈 칸).
/// - `matched_qpc`: `take_qpc` 결과(비디오 파이프라인이 캡처 시각으로 쓰는 값).
/// - `match`: `exact`(맨 앞이 같은 pts), `skip`(앞쪽 입력을 버리고 맞춤), `fallback`(pts 일치 없이 맨 앞을 씀,
///   대기열이 비면 pkt pts를 그대로 씀), `lost`(출력 없이 버려진 입력, 출력 열은 빈 칸).
/// - `key_req`: 키프레임을 요청했나. `key_pkt`: ffmpeg 패킷의 키 플래그. `key_out`: 내보낸 결과.
/// - `bytes`: 내보낸 바이트 수. `pending`: 짝 대기열 길이(이 줄을 처리한 뒤).
pub const VIDEO_HEADER: &str = "codec,encoder,w,h,capture_qpc,arrival_qpc,enc_in_qpc,pkt_out_qpc,in_pts,pkt_pts,pkt_dts,matched_qpc,match,key_req,key_pkt,key_out,bytes,pending";

/// `audio.csv`의 열.
///
/// - `first_in_qpc`: 패킷에 넣은 첫 입력 샘플의 시각(지연 반영 전).
/// - `capture_qpc`: 인코더 지연을 반영한 캡처 시각(`EncodedAudio::capture_qpc_100ns`).
/// - `emit_qpc`: 콜백으로 내보내기 직전 시각.
pub const AUDIO_HEADER: &str = "sample_index,first_in_qpc,capture_qpc,emit_qpc,delay_samples,bytes";

const FLUSH_EVERY: Duration = Duration::from_millis(250);

enum Msg {
    Line(String),
    Flush(SyncSender<()>),
}

/// CSV 한 파일로 가는 기록 통로.
pub struct Sink {
    tx: Sender<Msg>,
}

impl Sink {
    /// `path`를 이어 쓰기로 연다. 비어 있으면 머리글을 쓴다.
    pub fn open(path: &Path, header: &str) -> std::io::Result<Sink> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let empty = file.metadata()?.len() == 0;
        let mut w = BufWriter::new(file);
        if empty {
            writeln!(w, "{header}")?;
            w.flush()?;
        }
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("dongsi-trace".into())
            .spawn(move || writer_loop(w, rx))?;
        Ok(Sink { tx })
    }

    /// 한 줄을 기록 대기열에 넣는다(줄바꿈은 여기서 붙인다).
    pub fn line(&self, s: String) {
        let _ = self.tx.send(Msg::Line(s));
    }

    /// 구분 줄: `# <label> <fields> qpc=<QPC> epoch_us=<µs>`.
    pub fn mark(&self, label: &str, fields: &str) {
        let (qpc, epoch_us) = clock_anchor();
        self.line(format!("# {label} {fields} qpc={qpc} epoch_us={epoch_us}"));
    }

    /// 지금까지 넣은 줄을 파일에 내보낼 때까지 기다린다(최대 2초).
    pub fn flush(&self) {
        let (ack_tx, ack_rx) = mpsc::sync_channel(1);
        if self.tx.send(Msg::Flush(ack_tx)).is_ok() {
            let _ = ack_rx.recv_timeout(Duration::from_secs(2));
        }
    }
}

fn writer_loop(mut w: BufWriter<File>, rx: Receiver<Msg>) {
    let mut last = Instant::now();
    loop {
        match rx.recv_timeout(FLUSH_EVERY) {
            Ok(Msg::Line(s)) => {
                let _ = w.write_all(s.as_bytes());
                let _ = w.write_all(b"\n");
            }
            Ok(Msg::Flush(ack)) => {
                let _ = w.flush();
                last = Instant::now();
                let _ = ack.send(());
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                let _ = w.flush();
                return;
            }
        }
        if last.elapsed() >= FLUSH_EVERY {
            let _ = w.flush();
            last = Instant::now();
        }
    }
}

/// 같은 순간의 (QPC 100ns, UNIX epoch µs).
pub fn clock_anchor() -> (i64, i64) {
    let qpc = qpc_now_100ns();
    let epoch_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as i64);
    (qpc, epoch_us)
}

fn dir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        std::env::var_os(ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    })
    .as_deref()
}

fn open_in_env(file: &str, header: &str) -> Option<Sink> {
    let path = dir()?.join(file);
    match Sink::open(&path, header) {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::warn!(path = %path.display(), "미디어 기록 파일을 열지 못했습니다: {e}");
            None
        }
    }
}

static VIDEO: OnceLock<Option<Sink>> = OnceLock::new();
static AUDIO: OnceLock<Option<Sink>> = OnceLock::new();

/// `video.csv`로 가는 통로. 기록이 꺼져 있으면 `None`.
pub(crate) fn video() -> Option<&'static Sink> {
    VIDEO
        .get_or_init(|| open_in_env("video.csv", VIDEO_HEADER))
        .as_ref()
}

/// `audio.csv`로 가는 통로. 기록이 꺼져 있으면 `None`.
pub(crate) fn audio() -> Option<&'static Sink> {
    AUDIO
        .get_or_init(|| open_in_env("audio.csv", AUDIO_HEADER))
        .as_ref()
}

/// 열려 있는 기록 파일을 모두 내보낸다. 기록이 꺼져 있으면 아무것도 안 한다.
pub fn flush() {
    for s in [&VIDEO, &AUDIO] {
        if let Some(Some(sink)) = s.get() {
            sink.flush();
        }
    }
}

fn opt(s: &mut String, v: Option<i64>) {
    use std::fmt::Write as _;
    if let Some(v) = v {
        let _ = write!(s, "{v}");
    }
}

/// `video.csv`의 한 줄.
pub(crate) struct VideoRow<'a> {
    pub codec: VideoCodec,
    pub encoder: &'a str,
    pub w: u32,
    pub h: u32,
    pub capture_qpc: Option<i64>,
    pub arrival_qpc: Option<i64>,
    pub enc_in_qpc: Option<i64>,
    pub pkt_out_qpc: Option<i64>,
    pub in_pts: Option<i64>,
    pub pkt_pts: Option<i64>,
    pub pkt_dts: Option<i64>,
    pub matched_qpc: Option<i64>,
    pub matching: &'a str,
    pub key_req: bool,
    pub key_pkt: bool,
    pub key_out: bool,
    pub bytes: usize,
    pub pending: usize,
}

impl VideoRow<'_> {
    pub fn to_line(&self) -> String {
        use std::fmt::Write as _;
        let mut s = String::with_capacity(200);
        let _ = write!(s, "{},{},{},{},", self.codec, self.encoder, self.w, self.h);
        for v in [
            self.capture_qpc,
            self.arrival_qpc,
            self.enc_in_qpc,
            self.pkt_out_qpc,
            self.in_pts,
            self.pkt_pts,
            self.pkt_dts,
            self.matched_qpc,
        ] {
            opt(&mut s, v);
            s.push(',');
        }
        let _ = write!(
            s,
            "{},{},{},{},{},{}",
            self.matching,
            self.key_req as u8,
            self.key_pkt as u8,
            self.key_out as u8,
            self.bytes,
            self.pending
        );
        s
    }
}

/// `audio.csv`의 한 줄.
pub(crate) fn audio_line(
    sample_index: u64,
    first_in_qpc: i64,
    capture_qpc: i64,
    emit_qpc: i64,
    delay_samples: u32,
    bytes: usize,
) -> String {
    format!("{sample_index},{first_in_qpc},{capture_qpc},{emit_qpc},{delay_samples},{bytes}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "dongsi-trace-test-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn row_has_as_many_fields_as_header() {
        let row = VideoRow {
            codec: VideoCodec::H264,
            encoder: "h264_nvenc",
            w: 1280,
            h: 720,
            capture_qpc: Some(10),
            arrival_qpc: Some(11),
            enc_in_qpc: Some(12),
            pkt_out_qpc: Some(13),
            in_pts: Some(10),
            pkt_pts: Some(10),
            pkt_dts: None,
            matched_qpc: Some(10),
            matching: "exact",
            key_req: true,
            key_pkt: true,
            key_out: true,
            bytes: 1234,
            pending: 0,
        };
        let line = row.to_line();
        assert_eq!(
            line.split(',').count(),
            VIDEO_HEADER.split(',').count(),
            "{line}"
        );
        assert_eq!(
            line,
            "H.264,h264_nvenc,1280,720,10,11,12,13,10,10,,10,exact,1,1,1,1234,0"
        );
        // 출력 없는 줄(lost)도 열 수가 같다.
        let lost = VideoRow {
            arrival_qpc: None,
            enc_in_qpc: None,
            pkt_out_qpc: None,
            pkt_pts: None,
            matched_qpc: None,
            matching: "lost",
            ..row
        };
        assert_eq!(
            lost.to_line().split(',').count(),
            VIDEO_HEADER.split(',').count()
        );
        let a = audio_line(960, 1, 2, 3, 312, 80);
        assert_eq!(a, "960,1,2,3,312,80");
        assert_eq!(a.split(',').count(), AUDIO_HEADER.split(',').count());
    }

    #[test]
    fn header_once_and_append_with_marks() {
        let dir = temp_dir("append");
        let path = dir.join("video.csv");
        let s = Sink::open(&path, "a,b").unwrap();
        s.mark("pipeline", "source=test");
        s.line("1,2".into());
        s.flush();
        drop(s);
        // 다시 열어도 머리글은 한 번이다.
        let s = Sink::open(&path, "a,b").unwrap();
        s.mark("pipeline", "source=test");
        s.line("3,4".into());
        s.flush();
        drop(s);
        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5, "{text}");
        assert_eq!(lines[0], "a,b");
        assert!(
            lines[1].starts_with("# pipeline source=test qpc="),
            "{text}"
        );
        assert!(lines[1].contains(" epoch_us="));
        assert_eq!(lines[2], "1,2");
        assert!(lines[3].starts_with("# pipeline"));
        assert_eq!(lines[4], "3,4");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabled_without_env_is_none() {
        // 이 시험 프로세스에는 환경 변수를 주지 않으므로 꺼져 있다.
        if std::env::var_os(ENV).is_none() {
            assert!(video().is_none());
            assert!(audio().is_none());
            flush(); // 아무 일도 없다
        }
    }
}
