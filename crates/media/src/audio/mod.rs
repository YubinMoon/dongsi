//! 오디오 파이프라인: WASAPI 루프백(이벤트 방식) → 48kHz 스테레오 시간축 → Opus(10ms, lowdelay) 패킷(D-080).
//!
//! 스트림은 끊기지 않는다. 캡처가 아무것도 주지 않으면(재생 중인 소리가 없거나 캡처를
//! 다시 만드는 중) 벽시계에 맞춰 무음 패킷을 낸다. `sample_index`는 패킷마다 정확히
//! [`AUDIO_PACKET_SAMPLES`](480)씩 늘어난다.

// probe(측정용)가 루프백 캡처를 같이 쓴다.
pub(crate) mod capture;
mod opus;
pub(crate) mod timeline;
pub(crate) mod tone;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;

use self::capture::{LoopbackCapture, Target, default_render_device_id};
use self::opus::OpusEncoder;
use self::timeline::{FRAME_SAMPLES, Timeline, align_base_qpc, qpc_of_sample};
use crate::MediaError;
use crate::clock::{instant_from_qpc, qpc_now_100ns};
use crate::process::find_discord_root_pids;

/// 오디오 캡처 대상.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioSource {
    /// 시스템 소리에서 Discord(Discord.exe / DiscordPTB.exe / DiscordCanary.exe)만 뺀 것.
    ///
    /// Discord 루트 프로세스([`find_discord_root_pids`]의 첫 번째) 트리를 프로세스 루프백
    /// EXCLUDE로 뺀다. Discord가 없으면 기본 출력 장치 전체 루프백을 쓴다.
    /// 약 3초마다 다시 확인해서 Discord가 켜지거나 꺼지면, 또는 기본 출력 장치가 바뀌면
    /// 캡처를 다시 만든다(그동안은 무음으로 이어진다).
    SystemExceptDiscord,
    /// 이 프로세스 트리의 소리만(프로세스 루프백 INCLUDE).
    ProcessTree(u32),
    /// 시스템 소리 전체(기본 출력 장치 루프백). 기본 장치가 바뀌면 따라간다.
    System,
    /// A/V 동기 측정용 테스트 톤.
    ///
    /// 무음이다가 매 정초(QPC가 10^7의 배수)에 1000Hz 사인파를 100ms 동안 낸다(진폭 0.5).
    /// 정초 시각의 샘플이 버스트의 첫 샘플(값 0, 위상 0)이다. 테스트 패턴 비디오의
    /// 흰 프레임과 같은 정초에 맞춰져 있다. 자세한 규칙과 타임스탬프 정의는
    /// [`EncodedAudio::capture_qpc_100ns`]를 본다.
    TestTone,
}

/// Opus 패킷 하나의 샘플 수(채널당, 48kHz). 10ms(D-080, 예전 20ms = 960). RTP 타임스탬프도 패킷마다 이만큼 는다.
pub const AUDIO_PACKET_SAMPLES: u64 = FRAME_SAMPLES as u64;

/// Opus 패킷 하나(10ms, 48kHz 스테레오).
#[derive(Debug, Clone)]
pub struct EncodedAudio {
    pub data: Vec<u8>,
    /// 이 패킷에 넣은 첫 입력 샘플의 번호(48kHz, 파이프라인 시작 기준).
    /// 패킷마다 정확히 [`AUDIO_PACKET_SAMPLES`]씩 늘고, 캡처를 다시 만들거나 무음이어도 끊기지 않는다.
    /// RTP 타임스탬프로 그대로 쓸 수 있다.
    pub sample_index: u64,
    /// 이 패킷을 디코딩했을 때 나오는 첫 샘플의 캡처 시각(QPC 100ns).
    ///
    /// Opus 인코더는 앞을 내다보는 지연(pre-skip, libopus `application=lowdelay`에서 120샘플
    /// = 2.5ms, D-080. 예전 `audio`는 312샘플 = 6.5ms)이 있어서, 패킷 k를 디코딩한 출력은 입력 샘플
    /// `sample_index - delay`부터 시작한다. 이 값은 그 지연을 반영한 시각이다:
    /// `base + (sample_index - delay) × 10^7 / 48000`. 따라서 디코딩한 오디오를 이 시각에
    /// 놓으면 비디오 `capture_qpc_100ns`와 같은 축에서 정확히 맞는다.
    /// 지연 값은 [`AudioStats::encoder_delay_samples`]에 있다.
    pub capture_qpc_100ns: i64,
    /// `capture_qpc_100ns`를 [`instant_from_qpc`]로 바꾼 값.
    pub capture: Instant,
}

/// 오디오 파이프라인 통계.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioStats {
    /// 예: `system-except-discord(pid 1234)`, `system`, `process-tree(pid 42)`, `test-tone`
    pub mode: String,
    pub discord_excluded: bool,
    /// 최근 소리 크기(dBFS, RMS, 빠르게 오르고 천천히 내림). 무음이면 -100.
    pub level_dbfs: f32,
    pub packets: u64,
    /// (추가) Opus 인코더 지연(샘플). lowdelay면 120(D-080, 예전 `audio`는 312).
    pub encoder_delay_samples: u32,
    /// (추가) 캡처가 없어 채운 무음(누적, ms)
    pub silence_filled_ms: u64,
    /// (추가) 시간축이 겹쳐 버린 캡처 샘플(누적, ms)
    pub dropped_ms: u64,
    /// (추가) 마지막 캡처 오류
    pub last_error: Option<String>,
}

struct Shared {
    stop: AtomicBool,
    stats: Mutex<AudioStats>,
    /// 워커가 다음 돌 때 바꿀 캡처 대상([`AudioControl::set_source`]).
    pending: Mutex<Option<AudioSource>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 오디오 캡처·인코딩 파이프라인.
pub struct AudioPipeline {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

/// 캡처가 조용하면 지금보다 이만큼 이전까지 무음을 채운다(예비 경로: 루프백은 보통 늘 흐른다, capture.rs). 루프백 버퍼는 보통 10~20ms 늦게
/// 도착하므로 그보다 넉넉하게 잡는다.
const SILENCE_LAG_QPC: i64 = 600_000; // 60ms
/// 고리를 도는 최대 간격. 이벤트 방식 캡처면 엔진이 버퍼를 채우는 즉시 깨어나고, 아니면(폴링·테스트 톤·캡처 없음) 이만큼 잔다.
const POLL: Duration = Duration::from_millis(5);
const RECHECK: Duration = Duration::from_secs(3);

impl AudioPipeline {
    /// 캡처와 Opus 인코딩을 시작한다. `on_packet`은 내부 스레드에서 10ms마다 불린다.
    /// `bitrate_kbps`는 Opus 비트레이트(보통 128, 16~510으로 맞춤).
    pub fn start(
        source: AudioSource,
        bitrate_kbps: u32,
        on_packet: Box<dyn FnMut(EncodedAudio) + Send>,
    ) -> Result<Self, MediaError> {
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            stats: Mutex::new(AudioStats {
                level_dbfs: -100.0,
                ..Default::default()
            }),
            pending: Mutex::new(None),
        });
        // 워커가 끝날 때 MTA가 내려가지 않게(비디오 파이프라인과 같은 이유, crate::com).
        crate::com::keep_mta();
        let (init_tx, init_rx) = mpsc::sync_channel(1);
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("dongsi-audio".into())
            .spawn(move || {
                let com = wasapi::initialize_mta().is_ok();
                let _mmcss = MmcssGuard::pro_audio();
                let worker = AudioWorker::new(source, bitrate_kbps, s, on_packet);
                match worker {
                    Ok(mut w) => {
                        let _ = init_tx.send(Ok(()));
                        w.run();
                    }
                    Err(e) => {
                        let _ = init_tx.send(Err(e));
                    }
                }
                if com {
                    wasapi::deinitialize();
                }
            })
            .map_err(|e| MediaError::Thread(e.to_string()))?;
        match init_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(AudioPipeline {
                shared,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                shared.stop.store(true, Ordering::SeqCst);
                Err(MediaError::Thread(
                    "오디오 워커가 시작되지 않았습니다".into(),
                ))
            }
        }
    }

    pub fn stats(&self) -> AudioStats {
        lock(&self.shared.stats).clone()
    }

    /// 다른 스레드에서 캡처 대상을 바꾸는 손잡이. 파이프라인이 멈춘 뒤에는 아무 일도 하지 않는다.
    pub fn control(&self) -> AudioControl {
        AudioControl {
            shared: self.shared.clone(),
        }
    }

    /// 멈추고 내부 스레드가 끝날 때까지 기다린다.
    pub fn stop(self) {
        drop(self);
    }
}

/// [`AudioPipeline`]의 캡처 대상을 바꾸는 손잡이([`AudioPipeline::control`]).
#[derive(Clone)]
pub struct AudioControl {
    shared: Arc<Shared>,
}

impl AudioControl {
    /// 캡처 대상을 바꾼다(게임 창 따라가기에서 앱 소리를 게임 프로세스로). 스트림은 끊기지 않고
    /// (`sample_index`가 이어지고) 새 캡처가 열리기 전까지는 무음이다. 테스트 톤과는 서로 바꾸지 않는다.
    pub fn set_source(&self, source: AudioSource) {
        *lock(&self.shared.pending) = Some(source);
    }
}

impl Drop for AudioPipeline {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// 오디오 스레드를 MMCSS "Pro Audio" 작업으로 올린다(D-080, Sunshine·MS ApplicationLoopback 샘플과 같음). 다른 일이 많아도
/// 캡처 이벤트에 제때 깨어나게 한다. 실패하면 보통 우선순위로 그냥 간다. 놓으면 되돌린다.
struct MmcssGuard(windows::Win32::Foundation::HANDLE);

impl MmcssGuard {
    fn pro_audio() -> Option<Self> {
        let mut index = 0u32;
        // SAFETY: 이 스레드에서 부르고, 돌려받은 핸들은 같은 스레드에서 Drop할 때 되돌린다.
        match unsafe {
            windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW(
                windows::core::w!("Pro Audio"),
                &mut index,
            )
        } {
            Ok(h) => Some(MmcssGuard(h)),
            Err(e) => {
                tracing::warn!("MMCSS 등록 실패(보통 우선순위로 계속): {e}");
                None
            }
        }
    }
}

impl Drop for MmcssGuard {
    fn drop(&mut self) {
        // SAFETY: `pro_audio`가 이 스레드에서 받은 핸들이다.
        let _ =
            unsafe { windows::Win32::System::Threading::AvRevertMmThreadCharacteristics(self.0) };
    }
}

struct AudioWorker {
    source: AudioSource,
    shared: Arc<Shared>,
    on_packet: Box<dyn FnMut(EncodedAudio) + Send>,
    encoder: OpusEncoder,
    timeline: Timeline,
    capture: Option<LoopbackCapture>,
    next_check: Instant,
    level: f32,
    out: Vec<Vec<u8>>,
    /// 패킷 기록(`DONGSI_MEDIA_TRACE`). 꺼져 있으면 `None`.
    trace: Option<&'static crate::trace::Sink>,
}

fn mode_name(t: &Target) -> String {
    match t {
        Target::Device(_) => "system".into(),
        Target::Exclude(pid) => format!("system-except-discord(pid {pid})"),
        Target::Include(pid) => format!("process-tree(pid {pid})"),
    }
}

impl AudioWorker {
    fn new(
        source: AudioSource,
        bitrate_kbps: u32,
        shared: Arc<Shared>,
        on_packet: Box<dyn FnMut(EncodedAudio) + Send>,
    ) -> Result<Self, MediaError> {
        let encoder = OpusEncoder::open(bitrate_kbps)?;
        let base = align_base_qpc(qpc_now_100ns());
        {
            let mut st = lock(&shared.stats);
            st.encoder_delay_samples = encoder.delay;
            if source == AudioSource::TestTone {
                st.mode = "test-tone".into();
            }
        }
        let mut w = AudioWorker {
            source,
            shared,
            on_packet,
            encoder,
            timeline: Timeline::new(base),
            capture: None,
            next_check: Instant::now(),
            level: -100.0,
            out: Vec::new(),
            trace: crate::trace::audio(),
        };
        if let Some(sink) = w.trace {
            sink.mark(
                "pipeline",
                &format!(
                    "source={:?} base_qpc={} delay_samples={}",
                    w.source, base, w.encoder.delay
                ),
            );
        }
        // 처음 캡처는 여기서 열어 본다. 실패해도 시작은 하고(무음), 3초마다 다시 시도한다.
        w.check_target();
        Ok(w)
    }

    fn desired_target(&self) -> Result<Option<Target>, MediaError> {
        Ok(match self.source {
            AudioSource::TestTone => None,
            AudioSource::ProcessTree(pid) => Some(Target::Include(pid)),
            AudioSource::System => Some(Target::Device(default_render_device_id()?)),
            AudioSource::SystemExceptDiscord => {
                let roots = find_discord_root_pids();
                if roots.len() > 1 {
                    tracing::warn!(
                        ?roots,
                        "Discord가 여러 개 실행 중입니다. 첫 번째 트리만 뺍니다"
                    );
                }
                match roots.first() {
                    Some(&pid) => Some(Target::Exclude(pid)),
                    None => Some(Target::Device(default_render_device_id()?)),
                }
            }
        })
    }

    fn check_target(&mut self) {
        let now = Instant::now();
        self.next_check = now + RECHECK;
        let want = match self.desired_target() {
            Ok(Some(t)) => t,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!("오디오 캡처 대상 확인 실패: {e}");
                lock(&self.shared.stats).last_error = Some(e.to_string());
                return;
            }
        };
        if self.capture.as_ref().is_some_and(|c| c.target == want) {
            return;
        }
        if let Some(old) = &self.capture {
            tracing::info!(from = ?old.target, to = ?want, "오디오 캡처 대상 변경");
        }
        self.capture = None;
        match LoopbackCapture::open(want.clone()) {
            Ok(c) => {
                let mut st = lock(&self.shared.stats);
                st.mode = mode_name(&c.target);
                st.discord_excluded = matches!(c.target, Target::Exclude(_));
                st.last_error = None;
                tracing::info!(mode = %st.mode, "오디오 캡처 시작");
                drop(st);
                self.capture = Some(c);
            }
            Err(e) => {
                tracing::warn!("오디오 캡처 열기 실패(무음으로 계속): {e}");
                let mut st = lock(&self.shared.stats);
                st.last_error = Some(e.to_string());
                st.mode = format!("{} (실패)", mode_name(&want));
                st.discord_excluded = false;
                self.next_check = now + Duration::from_secs(1);
            }
        }
    }

    fn run(&mut self) {
        let base = self.timeline.base_qpc();
        while !self.shared.stop.load(Ordering::SeqCst) {
            let pending = lock(&self.shared.pending).take();
            if let Some(source) = pending {
                self.retarget(source);
            }
            if self.source != AudioSource::TestTone && Instant::now() >= self.next_check {
                self.check_target();
            }
            let mut silence = 0u64;
            let mut dropped = 0u64;
            if let Some(c) = &mut self.capture {
                let tl = &mut self.timeline;
                let r = c.read(|qpc, s| {
                    let o = tl.push(qpc, s);
                    silence += o.silence;
                    dropped += o.dropped;
                });
                if let Err(e) = r {
                    tracing::warn!("오디오 캡처 오류, 다시 엽니다: {e}");
                    lock(&self.shared.stats).last_error = Some(e.to_string());
                    self.capture = None;
                    self.next_check = Instant::now() + Duration::from_millis(500);
                }
            }
            let now_qpc = qpc_now_100ns();
            if self.source == AudioSource::TestTone {
                self.timeline
                    .generate_until(now_qpc, |i| tone::tone_sample(base, i));
            } else {
                silence += self.timeline.fill_silence_until(now_qpc - SILENCE_LAG_QPC);
            }
            if silence > 0 || dropped > 0 {
                let mut st = lock(&self.shared.stats);
                st.silence_filled_ms += silence / 48;
                st.dropped_ms += dropped / 48;
            }
            if let Err(e) = self.emit_packets() {
                tracing::error!("Opus 인코딩 오류: {e}");
                lock(&self.shared.stats).last_error = Some(e.to_string());
            }
            self.wait();
        }
        crate::trace::flush();
        tracing::info!("오디오 파이프라인 종료");
    }

    /// 다음 고리까지 기다린다. 이벤트 방식 캡처면 엔진이 버퍼를 채우는 즉시(늦어도 `POLL`) 깨어난다.
    fn wait(&self) {
        match &self.capture {
            Some(c) if c.has_event() => c.wait(POLL),
            _ => std::thread::sleep(POLL),
        }
    }

    /// 캡처 대상을 바꾸고 바로 새 캡처를 연다. 테스트 톤과는 서로 바꾸지 않는다(시간축 생성 방식이 다르다).
    fn retarget(&mut self, source: AudioSource) {
        if source == self.source
            || source == AudioSource::TestTone
            || self.source == AudioSource::TestTone
        {
            return;
        }
        tracing::info!(from = ?self.source, to = ?source, "오디오 캡처 대상 바꾸기");
        self.source = source;
        self.check_target();
    }

    fn emit_packets(&mut self) -> Result<(), MediaError> {
        let base = self.timeline.base_qpc();
        let delay = self.encoder.delay as i64;
        while let Some((idx, pcm)) = self.timeline.pop_packet() {
            self.update_level(&pcm);
            self.out.clear();
            self.encoder.encode(idx, &pcm, &mut self.out)?;
            let n = self.out.len() as u64;
            for data in self.out.drain(..) {
                let qpc = qpc_of_sample(base, idx as i64 - delay);
                if let Some(sink) = self.trace {
                    sink.line(crate::trace::audio_line(
                        idx,
                        qpc_of_sample(base, idx as i64),
                        qpc,
                        qpc_now_100ns(),
                        self.encoder.delay,
                        data.len(),
                    ));
                }
                (self.on_packet)(EncodedAudio {
                    data,
                    sample_index: idx,
                    capture_qpc_100ns: qpc,
                    capture: instant_from_qpc(qpc),
                });
            }
            let mut st = lock(&self.shared.stats);
            st.packets += n;
            st.level_dbfs = self.level;
        }
        Ok(())
    }

    fn update_level(&mut self, pcm: &[f32]) {
        let sum: f32 = pcm.iter().map(|v| v * v).sum();
        let rms = (sum / pcm.len().max(1) as f32).sqrt();
        let db = if rms > 1e-5 {
            (20.0 * rms.log10()).max(-100.0)
        } else {
            -100.0
        };
        // 빠르게 오르고 20ms마다 1.5dB씩 내린다(패킷 길이와 상관없이).
        let fall = 1.5 * AUDIO_PACKET_SAMPLES as f32 / 960.0;
        self.level = if db > self.level {
            db
        } else {
            (self.level - fall).max(db)
        };
    }
}
