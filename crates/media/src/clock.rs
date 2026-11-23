//! 시각 기준: QueryPerformanceCounter(QPC)를 100ns 단위로 쓴다.
//!
//! WGC 프레임의 `SystemRelativeTime`, WASAPI `GetBuffer`의 QPC 위치가 모두 이 단위라서
//! 비디오와 오디오의 캡처 시각을 같은 축에서 비교할 수 있다.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

/// 1초에 해당하는 100ns 단위 수.
pub const QPC_PER_SEC: i64 = 10_000_000;

fn frequency() -> i64 {
    static FREQ: OnceLock<i64> = OnceLock::new();
    *FREQ.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: 유효한 출력 포인터를 넘긴다. Windows XP 이후로 실패하지 않는다.
        let _ = unsafe { QueryPerformanceFrequency(&mut f) };
        if f <= 0 { QPC_PER_SEC } else { f }
    })
}

fn ticks_to_100ns(ticks: i64, freq: i64) -> i64 {
    if freq == QPC_PER_SEC {
        ticks
    } else {
        (ticks as i128 * QPC_PER_SEC as i128 / freq as i128) as i64
    }
}

/// 지금의 QPC 값(100ns 단위). WGC `SystemRelativeTime`, WASAPI QPC 위치와 같은 축이다.
pub fn qpc_now_100ns() -> i64 {
    let mut c = 0i64;
    // SAFETY: 유효한 출력 포인터를 넘긴다.
    let _ = unsafe { QueryPerformanceCounter(&mut c) };
    ticks_to_100ns(c, frequency())
}

/// QPC 100ns 시각을 [`Instant`]로 바꾼다.
///
/// Windows의 `Instant`도 QPC 기반이지만 내부 값을 꺼낼 수 없으므로, 처음 부를 때 잡은 기준점(`Instant::now`와 QPC
/// 한 쌍)에서 차이로 환산한다. 기준점의 오차(1µs 안쪽)는 모든 값에 똑같이 들어가고, 둘 다 QPC라 시간이 지나도
/// 벌어지지 않는다. 같은 QPC는 늘 같은 `Instant`가 된다: simulcast 계층(D-098)이 같은 캡처를 따로 인코딩해도 송출
/// RTP 시각이 같다(부를 때마다 지금 시각으로 환산하면 1µs 안쪽에서 흔들려 90kHz 1클록씩 어긋날 수 있었다).
pub fn instant_from_qpc(qpc_100ns: i64) -> Instant {
    static ANCHOR: OnceLock<(Instant, i64)> = OnceLock::new();
    let &(base, base_qpc) = ANCHOR.get_or_init(|| (Instant::now(), qpc_now_100ns()));
    let diff = qpc_100ns - base_qpc;
    if diff >= 0 {
        base + Duration::from_nanos(diff as u64 * 100)
    } else {
        base.checked_sub(Duration::from_nanos(diff.unsigned_abs() * 100))
            .unwrap_or(base)
    }
}

/// 100ns 단위 길이를 밀리초(f32)로.
pub(crate) fn qpc_ms(d_100ns: i64) -> f32 {
    d_100ns as f32 / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instant_from_qpc_is_stable_and_tracks_now() {
        let q = qpc_now_100ns();
        let a = instant_from_qpc(q);
        std::thread::sleep(Duration::from_millis(5));
        // 같은 QPC는 늘 같은 Instant
        assert_eq!(instant_from_qpc(q), a);
        // 차이는 그대로
        assert_eq!(instant_from_qpc(q + 10_000) - a, Duration::from_millis(1));
        assert_eq!(a - instant_from_qpc(q - 10_000), Duration::from_millis(1));
        // 지금 시각과 맞는다(기준점 오차는 1µs 안쪽, 스레드 지연 여유 2ms).
        let now = Instant::now();
        let mapped = instant_from_qpc(qpc_now_100ns());
        let gap = if mapped > now {
            mapped - now
        } else {
            now - mapped
        };
        assert!(gap < Duration::from_millis(2), "{gap:?}");
    }

    #[test]
    fn ticks_conversion() {
        assert_eq!(ticks_to_100ns(12_345, QPC_PER_SEC), 12_345);
        assert_eq!(ticks_to_100ns(3_000_000, 3_000_000), QPC_PER_SEC);
        assert_eq!(ticks_to_100ns(1_500, 3_000), 5_000_000);
    }

    #[test]
    fn qpc_is_monotonic_and_instant_roundtrip() {
        let a = qpc_now_100ns();
        let b = qpc_now_100ns();
        assert!(b >= a);
        let past = a - 5_000_000; // 0.5초 전
        let i = instant_from_qpc(past);
        let elapsed = i.elapsed();
        assert!(elapsed >= Duration::from_millis(499), "{elapsed:?}");
        assert!(elapsed < Duration::from_millis(600), "{elapsed:?}");
    }
}
