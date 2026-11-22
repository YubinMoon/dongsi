//! 스레드·프로세스 우선순위(D-081).
//!
//! 게임 다운로드처럼 CPU·디스크를 많이 쓰는 앱이 함께 돌 때 캡처·인코딩·송출이 밀리지 않게 한다.
//! Windows에서만 동작하고 다른 OS(리눅스 전용 서버)에서는 아무것도 하지 않는다.
//!
//! - [`boost_current_thread`]: 지금 스레드를 MMCSS(Multimedia Class Scheduler Service) 작업에 등록한다.
//!   실패하면 `SetThreadPriority(THREAD_PRIORITY_HIGHEST)`로 대신한다. 돌려받은 [`ThreadBoost`]를 놓으면 되돌린다.
//! - [`ProcessBoost`]: 프로세스 우선순위 등급을 보통 → 보통 초과(ABOVE_NORMAL)로 올린다. 놓으면 보통으로 되돌린다.
//! - [`TimerResolution`]: 타이머 해상도 1ms를 요청한다(`timeBeginPeriod(1)`). Windows 기본 해상도(15.6ms)에서는
//!   `recv_timeout(0.5ms)`도 약 15.6ms 뒤에 깨어나서(2026-11-22 이 PC에서 잰 값, `.docs/research.md`) str0m pacer가
//!   미디어·탐색 패킷을 16ms 단위로 몰아 보내고, 탐색 결과가 낮게 나와 대역폭 추정이 실제보다 낮아진다. 창이 가려지거나
//!   최소화된 프로세스의 요청을 Windows 11이 무시하지 않도록 전원 조절(EcoQoS, 타이머 해상도 무시)에서도 뺀다.

/// 스레드가 하는 일. MMCSS 작업 이름을 고른다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadRole {
    /// 오디오 캡처·인코딩: MMCSS "Pro Audio".
    Audio,
    /// 화면 캡처 변환·인코딩: MMCSS "Capture".
    Video,
    /// 송출·SFU 패킷 처리: MMCSS "Capture".
    Network,
}

impl ThreadRole {
    /// MMCSS 작업 이름(`HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile\Tasks`).
    pub fn mmcss_task(self) -> &'static str {
        match self {
            ThreadRole::Audio => "Pro Audio",
            ThreadRole::Video | ThreadRole::Network => "Capture",
        }
    }
}

/// 실제로 건 방식.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoostKind {
    /// MMCSS 작업에 등록했다.
    Mmcss,
    /// MMCSS가 안 돼서 스레드 우선순위만 올렸다.
    ThreadPriority,
    /// 아무것도 못 했다(Windows가 아니거나 둘 다 실패).
    None,
}

/// [`boost_current_thread`]가 돌려주는 값. 놓으면(스레드를 마칠 때) 되돌린다. 건 스레드에서 놓아야 한다.
#[must_use = "놓으면 바로 되돌린다. 스레드가 끝날 때까지 들고 있어야 한다"]
pub struct ThreadBoost {
    kind: BoostKind,
    #[cfg(windows)]
    inner: imp::ThreadInner,
    /// 다른 스레드로 넘기지 못하게 한다(MMCSS 등록은 건 스레드에 묶인다).
    _not_send: std::marker::PhantomData<*const ()>,
}

impl ThreadBoost {
    pub fn kind(&self) -> BoostKind {
        self.kind
    }
}

impl std::fmt::Debug for ThreadBoost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThreadBoost")
            .field("kind", &self.kind)
            .finish()
    }
}

static BOOST_DISABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 이 프로세스에서 [`boost_current_thread`]가 더는 우선순위를 올리지 않게 한다(이미 올린 스레드는 그대로 둔다).
/// 되돌릴 수 없다.
///
/// 시험용이다. 서버 e2e는 한 프로세스에서 방송 수십 개(송출·SFU 스레드 100개 남짓)를 동시에 돌린다. 그러면 우선순위를
/// 올린 스레드들이 BWE 탐색 등으로 CPU를 다 차지할 때 보통 우선순위인 시험 시청자 스레드가 수백 ms씩 밀린다. 그 사이
/// 소켓 버퍼가 넘쳐 오디오가 20~30% 빠졌다(비디오는 NACK로 복구된다). 2026-11-22에 확인했고, MMCSS 대신
/// `THREAD_PRIORITY_HIGHEST`로 올려도 같았다. 앱과 전용 서버는 그 프로세스의 방송·SFU 스레드가 몇 개뿐이라 이 함수를
/// 부르지 않는다.
pub fn disable_thread_boost() {
    BOOST_DISABLED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// 지금 스레드의 우선순위를 올린다. 실패해도 오류를 내지 않는다(`kind()`가 [`BoostKind::None`]).
/// [`disable_thread_boost`]를 불렀으면 아무것도 하지 않는다.
pub fn boost_current_thread(role: ThreadRole) -> ThreadBoost {
    #[cfg(windows)]
    {
        let (kind, inner) = if BOOST_DISABLED.load(std::sync::atomic::Ordering::Relaxed) {
            (BoostKind::None, imp::ThreadInner::None)
        } else {
            imp::boost(role)
        };
        ThreadBoost {
            kind,
            inner,
            _not_send: std::marker::PhantomData,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = role;
        ThreadBoost {
            kind: BoostKind::None,
            _not_send: std::marker::PhantomData,
        }
    }
}

#[cfg(windows)]
impl Drop for ThreadBoost {
    fn drop(&mut self) {
        imp::revert(&mut self.inner);
    }
}

/// 프로세스 우선순위 등급을 ABOVE_NORMAL로 올려 둔 상태. 놓으면 보통으로 되돌린다.
///
/// 지금 등급이 보통일 때만 올린다(사용자가 이미 바꿔 둔 등급은 건드리지 않는다). 방송하는 동안만 들고 있는다.
#[derive(Debug)]
#[must_use = "놓으면 바로 되돌린다"]
pub struct ProcessBoost {
    _private: (),
}

impl ProcessBoost {
    /// 올렸으면 `Some`. 지금 등급이 보통이 아니거나 실패했거나 Windows가 아니면 `None`.
    pub fn above_normal() -> Option<ProcessBoost> {
        #[cfg(windows)]
        {
            imp::raise_process().then_some(ProcessBoost { _private: () })
        }
        #[cfg(not(windows))]
        {
            None
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessBoost {
    fn drop(&mut self) {
        imp::restore_process();
    }
}

/// 타이머 해상도 1ms를 요청해 둔 상태. 놓으면 요청을 거둔다. 여러 개를 동시에 들고 있어도 된다.
///
/// 방송하는 동안(송출 스레드, 같은 프로세스의 내장 서버 SFU)과 Windows 전용 서버가 들고 있는다. 전력을 조금 더 쓴다.
#[derive(Debug)]
#[must_use = "놓으면 바로 요청을 거둔다"]
pub struct TimerResolution {
    _private: (),
}

impl TimerResolution {
    /// 1ms를 요청한다. 실패하거나 Windows가 아니면 아무것도 하지 않는 값을 돌려준다.
    pub fn request_1ms() -> TimerResolution {
        #[cfg(windows)]
        imp::timer_acquire();
        TimerResolution { _private: () }
    }
}

#[cfg(windows)]
impl Drop for TimerResolution {
    fn drop(&mut self) {
        imp::timer_release();
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::Mutex;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
    use windows::Win32::System::Threading::{
        ABOVE_NORMAL_PRIORITY_CLASS, AvRevertMmThreadCharacteristics,
        AvSetMmThreadCharacteristicsW, GetCurrentProcess, GetCurrentThread, GetPriorityClass,
        GetThreadPriority, NORMAL_PRIORITY_CLASS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
        PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling, SetPriorityClass,
        SetProcessInformation, SetThreadPriority, THREAD_PRIORITY, THREAD_PRIORITY_HIGHEST,
    };
    use windows::core::HSTRING;

    use super::{BoostKind, ThreadRole};

    pub(super) enum ThreadInner {
        Mmcss(HANDLE),
        Priority(i32),
        None,
    }

    pub(super) fn boost(role: ThreadRole) -> (BoostKind, ThreadInner) {
        let task = HSTRING::from(role.mmcss_task());
        let mut index = 0u32;
        // SAFETY: 작업 이름은 널 종료 UTF-16 문자열이고 index는 이 함수 동안 살아 있는 지역 변수다.
        match unsafe { AvSetMmThreadCharacteristicsW(&task, &mut index) } {
            Ok(h) if !h.is_invalid() => {
                tracing::debug!(
                    task = role.mmcss_task(),
                    "스레드를 MMCSS 작업에 등록했습니다"
                );
                return (BoostKind::Mmcss, ThreadInner::Mmcss(h));
            }
            Ok(_) => {}
            Err(e) => tracing::debug!(
                task = role.mmcss_task(),
                "MMCSS 등록 실패, 스레드 우선순위로 대신합니다: {e}"
            ),
        }
        // SAFETY: GetCurrentThread는 지금 스레드를 가리키는 의사 핸들을 돌려준다(닫지 않는다).
        unsafe {
            let me = GetCurrentThread();
            let prev = GetThreadPriority(me);
            match SetThreadPriority(me, THREAD_PRIORITY_HIGHEST) {
                Ok(()) => (BoostKind::ThreadPriority, ThreadInner::Priority(prev)),
                Err(e) => {
                    tracing::debug!("스레드 우선순위를 올리지 못했습니다: {e}");
                    (BoostKind::None, ThreadInner::None)
                }
            }
        }
    }

    pub(super) fn revert(inner: &mut ThreadInner) {
        match std::mem::replace(inner, ThreadInner::None) {
            ThreadInner::Mmcss(h) => {
                // SAFETY: AvSetMmThreadCharacteristicsW가 이 스레드에서 돌려준 핸들이다.
                let _ = unsafe { AvRevertMmThreadCharacteristics(h) };
            }
            ThreadInner::Priority(prev) => {
                // SAFETY: 지금 스레드의 의사 핸들.
                let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY(prev)) };
            }
            ThreadInner::None => {}
        }
    }

    pub(super) fn raise_process() -> bool {
        // SAFETY: GetCurrentProcess는 지금 프로세스의 의사 핸들을 돌려준다(닫지 않는다).
        unsafe {
            let me = GetCurrentProcess();
            if GetPriorityClass(me) != NORMAL_PRIORITY_CLASS.0 {
                return false;
            }
            match SetPriorityClass(me, ABOVE_NORMAL_PRIORITY_CLASS) {
                Ok(()) => true,
                Err(e) => {
                    tracing::debug!("프로세스 우선순위를 올리지 못했습니다: {e}");
                    false
                }
            }
        }
    }

    pub(super) fn restore_process() {
        // SAFETY: 지금 프로세스의 의사 핸들. 우리가 올린 등급 그대로일 때만 되돌린다.
        unsafe {
            let me = GetCurrentProcess();
            if GetPriorityClass(me) == ABOVE_NORMAL_PRIORITY_CLASS.0 {
                let _ = SetPriorityClass(me, NORMAL_PRIORITY_CLASS);
            }
        }
    }

    /// 들고 있는 [`super::TimerResolution`] 수. 처음 것이 전원 조절을 끄고 마지막 것이 되돌린다.
    static TIMERS: Mutex<usize> = Mutex::new(0);

    pub(super) fn timer_acquire() {
        let mut n = TIMERS.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: 인자 없는 Win32 호출. 같은 값으로 timeEndPeriod를 한 번 부른다(timer_release).
        let r = unsafe { timeBeginPeriod(1) };
        if r != 0 {
            tracing::debug!(code = r, "타이머 해상도 1ms를 요청하지 못했습니다");
        }
        if *n == 0 {
            // EcoQoS를 끄고, 창이 가려져도 타이머 해상도 요청을 지키게 한다(ControlMask에 넣고 StateMask는 0).
            set_power_throttling(
                PROCESS_POWER_THROTTLING_EXECUTION_SPEED
                    | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
            );
        }
        *n += 1;
    }

    pub(super) fn timer_release() {
        let mut n = TIMERS.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: timer_acquire에서 건 요청을 거둔다.
        let _ = unsafe { timeEndPeriod(1) };
        *n = n.saturating_sub(1);
        if *n == 0 {
            // 시스템이 정하는 기본 동작으로 되돌린다.
            set_power_throttling(0);
        }
    }

    fn set_power_throttling(control_mask: u32) {
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: control_mask,
            StateMask: 0,
        };
        // SAFETY: 지금 프로세스의 의사 핸들과 이 함수 동안 살아 있는 구조체, 그 크기.
        let r = unsafe {
            SetProcessInformation(
                GetCurrentProcess(),
                ProcessPowerThrottling,
                &state as *const _ as *const core::ffi::c_void,
                size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            )
        };
        if let Err(e) = r {
            tracing::debug!("전원 조절 설정을 바꾸지 못했습니다: {e}");
        }
    }

    #[cfg(test)]
    pub(super) fn process_class() -> u32 {
        // SAFETY: 지금 프로세스의 의사 핸들.
        unsafe { GetPriorityClass(GetCurrentProcess()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_names() {
        assert_eq!(ThreadRole::Audio.mmcss_task(), "Pro Audio");
        assert_eq!(ThreadRole::Video.mmcss_task(), "Capture");
        assert_eq!(ThreadRole::Network.mmcss_task(), "Capture");
    }

    #[test]
    fn boost_and_revert_on_a_thread() {
        let kind = std::thread::spawn(|| {
            let b = boost_current_thread(ThreadRole::Video);
            let kind = b.kind();
            drop(b);
            kind
        })
        .join()
        .unwrap();
        if cfg!(windows) {
            assert_ne!(
                kind,
                BoostKind::None,
                "Windows에서는 MMCSS나 스레드 우선순위 중 하나는 되어야 합니다"
            );
        } else {
            assert_eq!(kind, BoostKind::None);
        }
    }

    /// 타이머 해상도를 요청하면 짧은 `recv_timeout`이 15.6ms까지 늘어지지 않는다.
    #[cfg(windows)]
    #[test]
    fn timer_resolution_shortens_waits() {
        let wait = || {
            let (_tx, rx) = std::sync::mpsc::sync_channel::<()>(1);
            let mut v: Vec<f64> = (0..20)
                .map(|_| {
                    let t = std::time::Instant::now();
                    let _ = rx.recv_timeout(std::time::Duration::from_micros(500));
                    t.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            v.sort_by(|a, b| a.total_cmp(b));
            v[10]
        };
        let a = TimerResolution::request_1ms();
        let b = TimerResolution::request_1ms();
        drop(a);
        let p50 = wait();
        drop(b);
        assert!(
            p50 < 8.0,
            "타이머 해상도 1ms를 요청했는데 0.5ms 대기의 중앙값이 {p50:.2}ms"
        );
    }

    #[cfg(windows)]
    #[test]
    fn process_boost_restores_normal() {
        use windows::Win32::System::Threading::{
            ABOVE_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
        };
        if imp::process_class() != NORMAL_PRIORITY_CLASS.0 {
            return; // 시험을 돌린 셸이 등급을 바꿔 두었다.
        }
        let boost = ProcessBoost::above_normal().expect("보통 등급이면 올릴 수 있어야 합니다");
        assert_eq!(imp::process_class(), ABOVE_NORMAL_PRIORITY_CLASS.0);
        assert!(
            ProcessBoost::above_normal().is_none(),
            "이미 올렸으면 다시 올리지 않는다"
        );
        drop(boost);
        assert_eq!(imp::process_class(), NORMAL_PRIORITY_CLASS.0);
    }
}
