//! COM 아파트먼트(MTA) 유지.

use std::sync::Once;

use windows::Win32::System::Com::CoIncrementMTAUsage;

/// 프로세스가 끝날 때까지 MTA를 살려 둔다(처음 한 번만 한다).
///
/// - 캡처 스레드는 `RoInitialize`/`RoUninitialize`(또는 `CoInitializeEx`/`CoUninitialize`)를 짝지어 부른다.
///   다른 곳이 MTA를 잡고 있지 않으면 마지막 해제가 MTA를 내린다. windows-rs는 WinRT 활성화 팩터리를
///   정적 캐시에 두므로, MTA가 내려간 뒤 다음 캡처가 그 팩터리를 쓰면 접근 위반으로 죽었다
///   (2026-11-21 시험: 첫 비디오 파이프라인이 끝난 뒤 두 번째 파이프라인 시작, 썸네일 뒤 `CoUninitialize`).
/// - COM을 켜지 않은 스레드(tokio 블로킹 스레드 등)도 암시적 MTA로 WIC 같은 COM 객체를 만들 수 있다.
pub(crate) fn keep_mta() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: 돌려받은 쿠키를 버려 사용 수를 프로세스 끝까지 유지한다.
        if let Err(e) = unsafe { CoIncrementMTAUsage() } {
            tracing::warn!("CoIncrementMTAUsage 실패: {e}");
        }
    });
}
