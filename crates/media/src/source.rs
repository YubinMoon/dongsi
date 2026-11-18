//! 캡처 대상(모니터, 창, 테스트 패턴) 목록과 ID.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW,
    HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_EXSTYLE, GetClassNameW, GetShellWindow, GetWindow, GetWindowLongW,
    GetWindowPlacement, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, MONITORINFOF_PRIMARY, SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
    WDA_NONE, WINDOWPLACEMENT, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, PCWSTR};

use crate::MediaError;
use crate::process::{process_name, wide_to_string};

/// 테스트 패턴의 기본(논리) 크기.
pub const TEST_PATTERN_SIZE: (u32, u32) = (1280, 720);

/// 캡처 대상 ID.
///
/// 문자열 형태(`monitor:<HMONITOR>`, `window:<HWND>`, `test`)로 직렬화한다.
/// HMONITOR·HWND 값은 디스플레이 구성이 바뀌거나 창이 닫히면 무효가 되므로
/// 오래 저장하지 말고 [`list_sources`]로 다시 받는다.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum SourceId {
    Monitor(isize),
    Window(isize),
    TestPattern,
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceId::Monitor(h) => write!(f, "monitor:{h}"),
            SourceId::Window(h) => write!(f, "window:{h}"),
            SourceId::TestPattern => f.write_str("test"),
        }
    }
}

impl FromStr for SourceId {
    type Err = MediaError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("test") {
            return Ok(SourceId::TestPattern);
        }
        let bad = || MediaError::InvalidSourceId(s.to_string());
        let (kind, num) = s.split_once(':').ok_or_else(bad)?;
        let num: isize = num.trim().parse().map_err(|_| bad())?;
        match kind.trim().to_ascii_lowercase().as_str() {
            "monitor" => Ok(SourceId::Monitor(num)),
            "window" => Ok(SourceId::Window(num)),
            _ => Err(bad()),
        }
    }
}

impl From<SourceId> for String {
    fn from(id: SourceId) -> String {
        id.to_string()
    }
}

impl TryFrom<String> for SourceId {
    type Error = MediaError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    Monitor,
    Window,
    Test,
}

/// 캡처 대상 하나의 정보. `width`·`height`는 물리 픽셀이다.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub id: SourceId,
    pub kind: SourceKind,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
    pub process_id: Option<u32>,
    pub process_name: Option<String>,
}

/// 캡처할 수 있는 대상 목록: 모니터(주 모니터 먼저), 보이는 앱 창, 마지막에 테스트 패턴.
///
/// 창은 보이지 않거나 클로킹된 창, 도구 창, 크기가 0인 창, 이 프로세스의 창을 뺀다.
pub fn list_sources() -> Result<Vec<SourceInfo>, MediaError> {
    let mut out = list_monitors();
    out.extend(list_windows());
    out.push(SourceInfo {
        id: SourceId::TestPattern,
        kind: SourceKind::Test,
        name: "테스트 패턴".to_string(),
        width: TEST_PATTERN_SIZE.0,
        height: TEST_PATTERN_SIZE.1,
        primary: false,
        process_id: None,
        process_name: None,
    });
    Ok(out)
}

/// 이 프로세스의 창을 화면 캡처에서 빼거나(`exclude = true`) 되돌린다.
///
/// `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`를 쓴다. 다른 프로세스의 창에는 쓸 수 없다.
pub fn exclude_window_from_capture(hwnd: isize, exclude: bool) -> Result<(), MediaError> {
    let affinity = if exclude {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    // SAFETY: 잘못된 HWND면 API가 오류를 돌려준다.
    unsafe { SetWindowDisplayAffinity(HWND(hwnd as *mut _), affinity) }
        .map_err(|e| MediaError::DisplayAffinity(e.message()))
}

struct MonitorEntry {
    handle: isize,
    device: String,
    width: u32,
    height: u32,
    left: i32,
    top: i32,
    primary: bool,
}

fn list_monitors() -> Vec<SourceInfo> {
    unsafe extern "system" fn callback(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: data는 아래 list_monitors의 Vec을 가리킨다.
        let list = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
        list.push(m);
        TRUE
    }
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: 콜백은 동기적으로 불리고, handles는 호출 동안 살아 있다.
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(callback),
            LPARAM(&mut handles as *mut _ as isize),
        );
    }

    let names = monitor_friendly_names();
    let mut entries: Vec<MonitorEntry> = handles.into_iter().filter_map(monitor_entry).collect();
    entries.sort_by_key(|e| (!e.primary, e.left, e.top));
    entries
        .into_iter()
        .enumerate()
        .map(|(i, e)| {
            let friendly = names
                .iter()
                .find(|(dev, _)| dev.eq_ignore_ascii_case(&e.device))
                .map(|(_, n)| n.clone())
                .filter(|n| !n.is_empty());
            let name = match friendly {
                Some(n) => format!("화면 {} ({n})", i + 1),
                None => format!("화면 {}", i + 1),
            };
            SourceInfo {
                id: SourceId::Monitor(e.handle),
                kind: SourceKind::Monitor,
                name,
                width: e.width,
                height: e.height,
                primary: e.primary,
                process_id: None,
                process_name: None,
            }
        })
        .collect()
}

fn monitor_entry(m: HMONITOR) -> Option<MonitorEntry> {
    // SAFETY: 크기를 채운 구조체 포인터를 넘긴다.
    unsafe {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(m, &mut info as *mut _ as *mut MONITORINFO).as_bool() {
            return None;
        }
        let device = wide_to_string(&info.szDevice);
        let rc = info.monitorInfo.rcMonitor;
        let (mut width, mut height) = ((rc.right - rc.left) as u32, (rc.bottom - rc.top) as u32);
        // GetMonitorInfo는 DPI 가상화될 수 있으므로 실제 디스플레이 모드 크기를 쓴다.
        let mut mode = DEVMODEW {
            dmSize: size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        let dev_w: Vec<u16> = device.encode_utf16().chain(Some(0)).collect();
        if EnumDisplaySettingsW(PCWSTR(dev_w.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode).as_bool()
            && mode.dmPelsWidth > 0
            && mode.dmPelsHeight > 0
        {
            width = mode.dmPelsWidth;
            height = mode.dmPelsHeight;
        }
        Some(MonitorEntry {
            handle: m.0 as isize,
            device,
            width,
            height,
            left: rc.left,
            top: rc.top,
            primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        })
    }
}

/// (GDI 장치 이름 `\\.\DISPLAY1`, 모니터 이름) 목록. 실패하면 빈 목록.
fn monitor_friendly_names() -> Vec<(String, String)> {
    let mut out = Vec::new();
    // SAFETY: 버퍼 크기를 먼저 받아 그만큼 할당한다. 헤더의 size·type을 정확히 채운다.
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != ERROR_SUCCESS {
            return out;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut np,
            paths.as_mut_ptr(),
            &mut nm,
            modes.as_mut_ptr(),
            None,
        ) != ERROR_SUCCESS
        {
            return out;
        }
        paths.truncate(np as usize);
        for path in &paths {
            let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                    size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                    adapterId: path.sourceInfo.adapterId,
                    id: path.sourceInfo.id,
                },
                ..Default::default()
            };
            if DisplayConfigGetDeviceInfo(&mut src.header) != 0 {
                continue;
            }
            let mut tgt = DISPLAYCONFIG_TARGET_DEVICE_NAME {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                    size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                    adapterId: path.targetInfo.adapterId,
                    id: path.targetInfo.id,
                },
                ..Default::default()
            };
            if DisplayConfigGetDeviceInfo(&mut tgt.header) != 0 {
                continue;
            }
            out.push((
                wide_to_string(&src.viewGdiDeviceName),
                wide_to_string(&tgt.monitorFriendlyDeviceName),
            ));
        }
    }
    out
}

fn list_windows() -> Vec<SourceInfo> {
    unsafe extern "system" fn callback(hwnd: HWND, data: LPARAM) -> BOOL {
        // SAFETY: data는 아래 list_windows의 Vec을 가리킨다.
        let list = unsafe { &mut *(data.0 as *mut Vec<HWND>) };
        list.push(hwnd);
        TRUE
    }
    let mut handles: Vec<HWND> = Vec::new();
    // SAFETY: 콜백은 동기적으로 불리고, handles는 호출 동안 살아 있다.
    unsafe {
        let _ = EnumWindows(Some(callback), LPARAM(&mut handles as *mut _ as isize));
    }
    let own_pid = std::process::id();
    handles
        .into_iter()
        .filter_map(|h| window_info(h, own_pid))
        .collect()
}

fn window_info(hwnd: HWND, own_pid: u32) -> Option<SourceInfo> {
    // SAFETY: 모두 HWND를 읽기만 하는 API다. 창이 사라졌으면 실패값을 돌려준다.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || hwnd == GetShellWindow() {
            return None;
        }
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TOOLWINDOW.0 != 0 {
            return None;
        }
        let owned = GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.is_invalid());
        if owned && ex & WS_EX_APPWINDOW.0 == 0 {
            return None;
        }
        if ex & WS_EX_NOACTIVATE.0 != 0 && ex & WS_EX_APPWINDOW.0 == 0 {
            return None;
        }
        let mut cloaked = 0u32;
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut _ as *mut _,
            size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0
        {
            return None;
        }
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return None;
        }
        let mut title = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd, &mut title);
        let title = String::from_utf16_lossy(&title[..n.max(0) as usize]);
        if title.trim().is_empty() {
            return None;
        }
        let mut class = [0u16; 256];
        let n = GetClassNameW(hwnd, &mut class);
        let class = String::from_utf16_lossy(&class[..n.max(0) as usize]);
        if matches!(
            class.as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        ) {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == own_pid {
            return None;
        }
        let (width, height) = window_size(hwnd)?;
        if width == 0 || height == 0 {
            return None;
        }
        Some(SourceInfo {
            id: SourceId::Window(hwnd.0 as isize),
            kind: SourceKind::Window,
            name: title,
            width,
            height,
            primary: false,
            process_id: Some(pid),
            process_name: process_name(pid),
        })
    }
}

/// 창의 물리 픽셀 크기. 최소화된 창은 복원했을 때의 크기.
fn window_size(hwnd: HWND) -> Option<(u32, u32)> {
    // SAFETY: 크기를 채운 출력 구조체를 넘긴다.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let mut wp = WINDOWPLACEMENT {
                length: size_of::<WINDOWPLACEMENT>() as u32,
                ..Default::default()
            };
            GetWindowPlacement(hwnd, &mut wp).ok()?;
            let r = wp.rcNormalPosition;
            return Some((
                (r.right - r.left).max(0) as u32,
                (r.bottom - r.top).max(0) as u32,
            ));
        }
        let mut r = RECT::default();
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut _ as *mut _,
            size_of::<RECT>() as u32,
        )
        .ok()?;
        Some((
            (r.right - r.left).max(0) as u32,
            (r.bottom - r.top).max(0) as u32,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_id_string_roundtrip() {
        for id in [
            SourceId::Monitor(65537),
            SourceId::Window(-123),
            SourceId::TestPattern,
        ] {
            let s = id.to_string();
            assert_eq!(s.parse::<SourceId>().unwrap(), id);
            let json = serde_json_like(&id);
            assert_eq!(json, s);
        }
        assert_eq!("TEST".parse::<SourceId>().unwrap(), SourceId::TestPattern);
        assert_eq!(
            " window: 42 ".parse::<SourceId>().unwrap(),
            SourceId::Window(42)
        );
        assert!("screen:1".parse::<SourceId>().is_err());
        assert!("monitor:x".parse::<SourceId>().is_err());
        assert!("monitor".parse::<SourceId>().is_err());
    }

    fn serde_json_like(id: &SourceId) -> String {
        String::from(id.clone())
    }

    #[test]
    fn lists_monitors_and_test_pattern() {
        let list = list_sources().unwrap();
        assert_eq!(list.last().unwrap().id, SourceId::TestPattern);
        // CI가 아닌 이 PC에서는 모니터가 하나 이상 있다.
        let monitors: Vec<_> = list
            .iter()
            .filter(|s| s.kind == SourceKind::Monitor)
            .collect();
        assert!(!monitors.is_empty());
        assert!(monitors[0].primary);
        assert!(monitors.iter().all(|m| m.width > 0 && m.height > 0));
    }
}
