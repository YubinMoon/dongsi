//! 개발용 측정 명령(디버그 빌드에서만 동작): 시계 맞추기와 프로세스 트리의 삐 시작 시각.
//!
//! e2e 하네스의 A/V 측정(`apps/desktop/e2e/lib/sync-probe.mjs`)이 쓴다. 페이지의
//! `performance.now()`와 QPC(100ns)를 맞추는 `dev_clock`, 이 앱 프로세스 트리의 소리(WebView2 포함)를
//! WASAPI 프로세스 루프백으로 받아 테스트 톤의 삐 시작 시각(QPC)을 모으는 `dev_tone_probe_*`.
//! 배포 빌드에서는 모두 오류를 돌려준다.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use dongsi_media::clock::qpc_now_100ns;
use dongsi_media::probe::{DEFAULT_THRESHOLD, ProbeTarget, ToneProbe};
use serde::Serialize;
use tauri::State;

use crate::error::{CmdError, CmdResult};

/// 측정 스레드(하나만). `app.manage(ToneProbeState::default())`로 등록한다.
#[derive(Default)]
pub struct ToneProbeState(Mutex<Option<ToneProbe>>);

impl ToneProbeState {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<ToneProbe>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn dev_only() -> CmdResult<()> {
    if cfg!(debug_assertions) {
        Ok(())
    } else {
        Err(CmdError::new("개발 빌드에서만 쓸 수 있습니다"))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevClock {
    /// QPC(100ns 단위). 미디어 캡처 시각과 같은 축.
    #[serde(rename = "qpc100ns")]
    qpc_100ns: i64,
    unix_ms: i64,
}

/// 지금의 QPC(100ns)와 유닉스 시각(ms).
#[tauri::command]
pub fn dev_clock() -> CmdResult<DevClock> {
    dev_only()?;
    let qpc_100ns = qpc_now_100ns();
    let unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok(DevClock { qpc_100ns, unix_ms })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToneProbeInfo {
    pid: u32,
    threshold: f32,
}

/// 이 앱 프로세스 트리(`pid`로 다른 프로세스, `device`로 기본 출력 장치 전체)의 소리를 루프백으로 받아 삐 시작 시각을 모으기 시작한다.
/// 이미 켜져 있으면 다시 시작한다(모은 것은 버린다).
#[tauri::command]
pub async fn dev_tone_probe_start(
    state: State<'_, ToneProbeState>,
    pid: Option<u32>,
    device: Option<bool>,
) -> CmdResult<ToneProbeInfo> {
    dev_only()?;
    let pid = pid.unwrap_or_else(std::process::id);
    let target = if device == Some(true) {
        ProbeTarget::Device
    } else {
        ProbeTarget::Process(pid)
    };
    // 이전 측정은 먼저 멈춘다(스레드 join이 있으므로 블로킹 스레드에서).
    let old = state.lock().take();
    let probe = tauri::async_runtime::spawn_blocking(move || {
        drop(old);
        ToneProbe::start(target, DEFAULT_THRESHOLD)
    })
    .await
    .map_err(|e| CmdError::new(e.to_string()))?
    .map_err(|e| CmdError::new(e.to_string()))?;
    *state.lock() = Some(probe);
    Ok(ToneProbeInfo {
        pid,
        threshold: DEFAULT_THRESHOLD,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToneProbeTake {
    /// 지금까지 찾은 삐 시작(QPC 100ns). 가져가면 비운다.
    beeps: Vec<i64>,
    /// 지금까지 받은 샘플 수(채널당). 0이면 루프백에서 아무것도 안 온 것이다.
    samples: u64,
    /// 지금까지 본 가장 큰 절댓값
    peak: f32,
    last_error: Option<String>,
}

/// 지금까지의 삐 시작(QPC 100ns)을 넘기고 비운다.
#[tauri::command]
pub fn dev_tone_probe_take(state: State<'_, ToneProbeState>) -> CmdResult<ToneProbeTake> {
    dev_only()?;
    let guard = state.lock();
    let probe = guard
        .as_ref()
        .ok_or_else(|| CmdError::new("삐 측정이 켜져 있지 않습니다"))?;
    let t = probe.take();
    Ok(ToneProbeTake {
        beeps: t.beeps,
        samples: t.samples,
        peak: t.peak,
        last_error: t.last_error,
    })
}

/// 삐 측정을 멈춘다(켜져 있지 않아도 오류가 아니다).
#[tauri::command]
pub async fn dev_tone_probe_stop(state: State<'_, ToneProbeState>) -> CmdResult<()> {
    dev_only()?;
    let old = state.lock().take();
    tauri::async_runtime::spawn_blocking(move || drop(old))
        .await
        .map_err(|e| CmdError::new(e.to_string()))?;
    Ok(())
}
