//! Wi-Fi 경로 감지와 WLAN media streaming mode(D-088).
//!
//! - [`route_path`]: 서버 주소로 가는 경로가 Wi-Fi인지 본다(읽기 전용, 패킷을 보내지 않는다).
//!   경로 어댑터가 VPN·Tailscale 같은 가상 어댑터면 기본 경로의 물리 어댑터로 판단한다(`via_tunnel`).
//! - media streaming mode: Wi-Fi 어댑터의 주기적 스캔이 만드는 정체를 줄이려고 시청·송출하는 동안 켠다
//!   (`WlanSetInterface`). 사용자(holder)별로 참조를 세어 마지막 사용자가 놓으면 끈다. 연결이 끊기면 Windows가
//!   저절로 끈다. 켜지 못해도(연결 안 됨·권한·드라이버) `warn!`만 남기고 시청·송출은 계속한다.
//!
//! 자동 시험이 사용자의 Wi-Fi 설정을 바꾸지 않게, 켜고 끄는 호출은 Tauri 명령 층(`live.rs`의 시청 명령,
//! `broadcast::start_broadcast`)에만 있다. `viewing`·`session`·`Broadcaster`와 `tests/`는 이 모듈을 부르지 않는다.
//! 이 모듈의 단위 시험은 가짜 backend로 참조 세기만 시험한다(`WlanSetInterface`는 부르지 않는다).

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{LazyLock, Mutex, MutexGuard};

use serde::Serialize;
use tracing::{info, warn};

/// 서버로 가는 경로의 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NetKind {
    Wifi,
    Wired,
    /// 루프백이거나 물리 경로를 알 수 없는 가상 어댑터(VPN 전체 터널 등).
    Other,
    /// 경로를 조회하지 못했다.
    Unknown,
}

/// 서버로 가는 지금 경로(`network_path`, `WatchAnswer.network`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPath {
    pub kind: NetKind,
    /// 경로 어댑터가 가상 어댑터(VPN·Tailscale)였고, 그 밑 물리 경로(기본 경로)로 판단했다.
    pub via_tunnel: bool,
    /// 0~100(`wlanSignalQuality`). Wi-Fi이고 읽었을 때만.
    pub signal_quality: Option<u8>,
    pub rssi_dbm: Option<i32>,
    /// 수신 속도(Mbps, `ulRxRate` kbps / 1000).
    pub rx_rate_mbps: Option<u32>,
    pub channel: Option<u32>,
    /// 이 앱이 그 인터페이스에 media streaming mode를 켜 두었는가.
    pub streaming_mode: bool,
}

impl NetworkPath {
    fn of(kind: NetKind, via_tunnel: bool) -> Self {
        Self {
            kind,
            via_tunnel,
            signal_quality: None,
            rssi_dbm: None,
            rx_rate_mbps: None,
            channel: None,
            streaming_mode: false,
        }
    }

    pub fn unknown() -> Self {
        Self::of(NetKind::Unknown, false)
    }

    /// 로그 한 줄용 요약.
    pub fn summary(&self) -> String {
        let mut s = format!("kind={:?} via_tunnel={}", self.kind, self.via_tunnel);
        if let Some(v) = self.signal_quality {
            s += &format!(" signal={v}%");
        }
        if let Some(v) = self.rssi_dbm {
            s += &format!(" rssi={v}dBm");
        }
        if let Some(v) = self.rx_rate_mbps {
            s += &format!(" rx_mbps={v}Mbps");
        }
        if let Some(v) = self.channel {
            s += &format!(" channel={v}");
        }
        s + &format!(" streaming_mode={}", self.streaming_mode)
    }
}

/// 어댑터 한 개의 정보(`MIB_IF_ROW2`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IfInfo {
    /// `IfType`(IANA).
    pub if_type: u32,
    /// `InterfaceGuid`(WLAN 인터페이스 GUID와 같다).
    pub guid: u128,
    pub description: String,
}

const IF_TYPE_ETHERNET: u32 = 6;
const IF_TYPE_LOOPBACK: u32 = 24;
const IF_TYPE_IEEE80211: u32 = 71;

/// `IfType`을 경로 종류로. 이더넷·Wi-Fi·루프백만 알고, 그 밖(터널·가상)은 `None`.
pub(crate) fn kind_of_if_type(if_type: u32) -> Option<NetKind> {
    match if_type {
        IF_TYPE_IEEE80211 => Some(NetKind::Wifi),
        IF_TYPE_ETHERNET => Some(NetKind::Wired),
        IF_TYPE_LOOPBACK => Some(NetKind::Other),
        _ => None,
    }
}

/// 경로 어댑터와(가상 어댑터일 때만 부르는) 기본 경로 어댑터로 경로를 판단한다.
/// 돌려주는 어댑터는 Wi-Fi일 때 WLAN을 조회·설정할 대상이다.
fn judge(
    first: Option<IfInfo>,
    default: impl FnOnce() -> Option<IfInfo>,
) -> (NetKind, bool, Option<IfInfo>) {
    let Some(first) = first else {
        return (NetKind::Unknown, false, None);
    };
    if let Some(kind) = kind_of_if_type(first.if_type) {
        return (kind, false, (kind == NetKind::Wifi).then_some(first));
    }
    match default() {
        Some(d)
            if matches!(
                kind_of_if_type(d.if_type),
                Some(NetKind::Wifi | NetKind::Wired)
            ) =>
        {
            let kind = kind_of_if_type(d.if_type).expect("방금 확인");
            (kind, true, (kind == NetKind::Wifi).then_some(d))
        }
        _ => (NetKind::Other, false, None),
    }
}

/// 서버 주소로 가는 지금 경로. 조회에 실패해도 패닉 없이 `unknown`.
pub fn route_path(target: IpAddr) -> NetworkPath {
    inspect(target).0
}

fn inspect(target: IpAddr) -> (NetworkPath, Option<IfInfo>) {
    let target = target.to_canonical();
    let (kind, via_tunnel, wifi) =
        judge(sys::best_if(target), || sys::default_if(target.is_ipv6()));
    let mut path = NetworkPath::of(kind, via_tunnel);
    if let Some(w) = &wifi {
        let d = sys::wlan_details(w.guid);
        path.signal_quality = d.signal_quality;
        path.rssi_dbm = d.rssi_dbm;
        path.rx_rate_mbps = d.rx_rate_mbps;
        path.channel = d.channel;
        path.streaming_mode = manager().is_on(w.guid);
    }
    (path, wifi)
}

/// WLAN 조회로 읽은 값.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct WlanDetails {
    pub signal_quality: Option<u8>,
    pub rssi_dbm: Option<i32>,
    pub rx_rate_mbps: Option<u32>,
    pub channel: Option<u32>,
}

/// media streaming mode를 쥐고 있는 쪽.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Holder {
    Watch { key: String, session: u64 },
    Broadcast,
}

/// 인터페이스에 media streaming mode를 켜고 끈다(`WlanSetInterface`). 시험은 가짜로 바꾼다.
pub(crate) trait ModeBackend {
    fn set(&mut self, iface: u128, on: bool) -> Result<(), String>;
}

struct Entry {
    description: String,
    holders: HashSet<Holder>,
}

/// holder별 참조 수로 인터페이스마다 첫 사용자가 켜고 마지막 사용자가 끈다.
pub(crate) struct Manager<B> {
    backend: B,
    held: HashMap<u128, Entry>,
}

impl<B: ModeBackend> Manager<B> {
    pub(crate) fn new(backend: B) -> Self {
        Self {
            backend,
            held: HashMap::new(),
        }
    }

    /// 그 인터페이스의 mode를 `holder` 몫으로 잡는다. 같은 holder를 또 잡아도 한 번으로 센다.
    /// 켜지 못하면 `warn!`만 남기고 false.
    pub(crate) fn acquire(
        &mut self,
        holder: &Holder,
        iface: u128,
        description: &str,
        reason: &str,
    ) -> bool {
        if let Some(entry) = self.held.get_mut(&iface) {
            entry.holders.insert(holder.clone());
            return true;
        }
        match self.backend.set(iface, true) {
            Ok(()) => {
                info!(
                    adapter = description,
                    reason = reason,
                    "WLAN media streaming mode 켬"
                );
                self.held.insert(
                    iface,
                    Entry {
                        description: description.to_owned(),
                        holders: HashSet::from([holder.clone()]),
                    },
                );
                true
            }
            Err(e) => {
                warn!(
                    adapter = description,
                    reason = reason,
                    "WLAN media streaming mode를 켜지 못했습니다: {e}"
                );
                false
            }
        }
    }

    /// `pred`에 맞는 holder를 모두 놓는다. 사용자가 없어진 인터페이스는 끈다.
    pub(crate) fn release_where(&mut self, pred: impl Fn(&Holder) -> bool, reason: &str) {
        let mut emptied = Vec::new();
        for (iface, entry) in &mut self.held {
            entry.holders.retain(|h| !pred(h));
            if entry.holders.is_empty() {
                emptied.push(*iface);
            }
        }
        for iface in emptied {
            let Some(entry) = self.held.remove(&iface) else {
                continue;
            };
            match self.backend.set(iface, false) {
                Ok(()) => info!(
                    adapter = entry.description,
                    reason = reason,
                    "WLAN media streaming mode 끔"
                ),
                Err(e) => warn!(
                    adapter = entry.description,
                    "WLAN media streaming mode를 끄지 못했습니다: {e}"
                ),
            }
        }
    }

    pub(crate) fn is_on(&self, iface: u128) -> bool {
        self.held.contains_key(&iface)
    }

    #[cfg(test)]
    pub(crate) fn holder_count(&self, iface: u128) -> usize {
        self.held.get(&iface).map_or(0, |e| e.holders.len())
    }
}

static MANAGER: LazyLock<Mutex<Manager<sys::SysBackend>>> =
    LazyLock::new(|| Mutex::new(Manager::new(sys::SysBackend::default())));

fn manager() -> MutexGuard<'static, Manager<sys::SysBackend>> {
    MANAGER.lock().unwrap_or_else(|e| e.into_inner())
}

/// `target`으로 가는 경로가 Wi-Fi면 mode를 `holder` 몫으로 켠다. 경로(`streaming_mode` 포함)를 돌려준다.
pub fn enable_for_route(holder: &Holder, target: IpAddr, reason: &str) -> NetworkPath {
    let (mut path, wifi) = inspect(target);
    if let Some(w) = wifi {
        path.streaming_mode = manager().acquire(holder, w.guid, &w.description, reason);
    }
    path
}

/// 연결된 WLAN 인터페이스 모두에 mode를 켠다(내장 서버로 방송할 때). 켠 개수를 돌려준다.
pub fn enable_for_all_wlan(holder: &Holder, reason: &str) -> usize {
    let mut on = 0;
    for (guid, description) in sys::connected_wlans() {
        if manager().acquire(holder, guid, &description, reason) {
            on += 1;
        }
    }
    on
}

pub fn release(holder: &Holder, reason: &str) {
    manager().release_where(|h| h == holder, reason);
}

/// 그 서버 연결로 한 시청을 모두 놓는다(연결을 끊을 때).
pub fn release_watch_key(key: &str, reason: &str) {
    manager().release_where(
        |h| matches!(h, Holder::Watch { key: k, .. } if k == key),
        reason,
    );
}

/// 남은 것을 모두 끈다(앱이 끝날 때).
pub fn shutdown() {
    manager().release_where(|_| true, "앱 종료");
}

// 아래는 Tauri 명령에서 쓰는 비동기 감싸개다. WLAN API는 수십 ms 걸릴 수 있어 blocking 풀에서 돈다.

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static, fallback: T) -> T {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .unwrap_or(fallback)
}

/// 시청을 시작했다: 경로를 보고 Wi-Fi면 mode를 켠다.
pub async fn watch_started(key: String, session: u64, target: IpAddr) -> NetworkPath {
    let path = blocking(
        move || enable_for_route(&Holder::Watch { key, session }, target, "시청"),
        NetworkPath::unknown(),
    )
    .await;
    info!("시청 시작 경로: {}", path.summary());
    path
}

/// 시청이 끝났다: 경로를 한 줄 남기고 놓는다. `target`이 없으면(연결이 닫힘) 놓기만 한다.
pub async fn watch_stopped(key: String, session: u64, target: Option<IpAddr>) {
    blocking(
        move || {
            if let Some(t) = target {
                info!("시청 끝 경로: {}", route_path(t).summary());
            }
            release(&Holder::Watch { key, session }, "시청 끝");
        },
        (),
    )
    .await;
}

/// 서버 연결의 지금 경로(`network_path` 명령).
pub async fn current(target: IpAddr) -> NetworkPath {
    blocking(move || route_path(target), NetworkPath::unknown()).await
}

/// 송출자가 쓰는 서버.
#[derive(Debug, Clone, Copy)]
pub enum BroadcastPath {
    /// 전용 서버(연결한 주소).
    Server(IpAddr),
    /// 앱 내장 서버(로컬 랜·PC 직접 호스팅).
    Embedded,
}

/// 방송을 시작했다. 전용 서버면 그 경로가 Wi-Fi일 때, 내장 서버면 연결된 WLAN 모두에 켠다.
pub async fn broadcast_started(server: BroadcastPath) {
    blocking(
        move || match server {
            BroadcastPath::Server(ip) => {
                let path = enable_for_route(&Holder::Broadcast, ip, "송출");
                info!("송출자 경로: {}", path.summary());
            }
            BroadcastPath::Embedded => {
                let on = enable_for_all_wlan(&Holder::Broadcast, "송출(내장 서버)");
                info!("송출자 경로: 내장 서버, 연결된 WLAN 중 {on}개에 streaming mode를 켬");
            }
        },
        (),
    )
    .await;
}

/// 방송이 끝났다.
pub async fn broadcast_stopped() {
    blocking(|| release(&Holder::Broadcast, "방송 끝"), ()).await;
}

/// 경로를 조회할 때 쓰는 공용 주소(기본 경로 판단용). 경로 조회만 하고 패킷은 보내지 않는다.
const PROBE_V4: IpAddr = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));
const PROBE_V6: IpAddr = IpAddr::V6(Ipv6Addr::new(0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111));

#[cfg(windows)]
mod sys {
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::net::IpAddr;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::NetworkManagement::IpHelper::{
        GetBestInterfaceEx, GetIfEntry2, MIB_IF_ROW2,
    };
    use windows::Win32::NetworkManagement::WiFi::{
        WLAN_API_VERSION_2_0, WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO_LIST,
        WLAN_INTF_OPCODE, WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory, WlanOpenHandle,
        WlanQueryInterface, WlanSetInterface, wlan_interface_state_connected,
        wlan_intf_opcode_channel_number, wlan_intf_opcode_current_connection,
        wlan_intf_opcode_media_streaming_mode, wlan_intf_opcode_rssi,
    };
    use windows::Win32::Networking::WinSock::SOCKADDR;
    use windows::core::GUID;

    use super::{IfInfo, ModeBackend, PROBE_V4, PROBE_V6, WlanDetails};

    /// `sockaddr_in`·`sockaddr_in6`를 담는 4바이트 정렬 버퍼.
    #[repr(C, align(4))]
    struct SockBuf([u8; 28]);

    fn sockaddr(ip: IpAddr) -> SockBuf {
        let mut b = SockBuf([0; 28]);
        match ip {
            IpAddr::V4(v4) => {
                b.0[0..2].copy_from_slice(&2u16.to_ne_bytes()); // AF_INET
                b.0[4..8].copy_from_slice(&v4.octets());
            }
            IpAddr::V6(v6) => {
                b.0[0..2].copy_from_slice(&23u16.to_ne_bytes()); // AF_INET6
                b.0[8..24].copy_from_slice(&v6.octets());
            }
        }
        b
    }

    fn wide(s: &[u16]) -> String {
        let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
        String::from_utf16_lossy(&s[..end])
    }

    /// `target`으로 가는 최적 인터페이스(`GetBestInterfaceEx` → `GetIfEntry2`). 경로 조회만 한다.
    pub fn best_if(target: IpAddr) -> Option<IfInfo> {
        let addr = sockaddr(target);
        let mut index = 0u32;
        // SAFETY: addr은 family에 맞는 sockaddr를 담은 정렬된 버퍼다.
        if unsafe { GetBestInterfaceEx(addr.0.as_ptr().cast::<SOCKADDR>(), &mut index) } != 0 {
            return None;
        }
        // SAFETY: MIB_IF_ROW2는 0으로 채워도 되는 C 구조체다.
        let mut row: MIB_IF_ROW2 = unsafe { std::mem::zeroed() };
        row.InterfaceIndex = index;
        // SAFETY: row는 쓸 수 있는 구조체다.
        if unsafe { GetIfEntry2(&mut row) }.0 != 0 {
            return None;
        }
        Some(IfInfo {
            if_type: row.Type,
            guid: row.InterfaceGuid.to_u128(),
            description: wide(&row.Description),
        })
    }

    /// 기본 경로(공용 주소로 가는 경로)의 어댑터. 주소 종류가 안 되면 다른 종류로 다시 본다.
    pub fn default_if(prefer_v6: bool) -> Option<IfInfo> {
        let (first, second) = if prefer_v6 {
            (PROBE_V6, PROBE_V4)
        } else {
            (PROBE_V4, PROBE_V6)
        };
        best_if(first).or_else(|| best_if(second))
    }

    struct Wlan(HANDLE);

    impl Wlan {
        fn open() -> Result<Self, u32> {
            let mut negotiated = 0u32;
            let mut handle = HANDLE::default();
            // SAFETY: 출력 인자는 모두 쓸 수 있는 지역 변수다.
            let rc =
                unsafe { WlanOpenHandle(WLAN_API_VERSION_2_0, None, &mut negotiated, &mut handle) };
            if rc == 0 { Ok(Self(handle)) } else { Err(rc) }
        }

        /// 값을 읽는다. 못 읽거나 크기가 모자라면 `None`.
        fn query<T: Copy>(&self, guid: &GUID, opcode: WLAN_INTF_OPCODE) -> Option<T> {
            let mut size = 0u32;
            let mut data: *mut c_void = std::ptr::null_mut();
            // SAFETY: 출력 인자는 지역 변수다. 돌려받은 메모리는 아래에서 WlanFreeMemory로 놓는다.
            let rc = unsafe {
                WlanQueryInterface(self.0, guid, opcode, None, &mut size, &mut data, None)
            };
            if rc != 0 || data.is_null() {
                return None;
            }
            let value = (size as usize >= std::mem::size_of::<T>())
                // SAFETY: size가 T 이상이고 data는 그 크기만큼 읽을 수 있다(정렬은 모르므로 unaligned).
                .then(|| unsafe { std::ptr::read_unaligned(data.cast::<T>()) });
            // SAFETY: WlanQueryInterface가 준 메모리다.
            unsafe { WlanFreeMemory(data) };
            value
        }
    }

    impl Drop for Wlan {
        fn drop(&mut self) {
            // SAFETY: open이 준 핸들이다.
            unsafe { WlanCloseHandle(self.0, None) };
        }
    }

    /// 신호 품질·rssi·수신 속도·채널(읽기 전용). 못 읽은 값은 `None`.
    pub fn wlan_details(iface: u128) -> WlanDetails {
        let Ok(wlan) = Wlan::open() else {
            return WlanDetails::default();
        };
        let guid = GUID::from_u128(iface);
        let mut d = WlanDetails::default();
        if let Some(c) =
            wlan.query::<WLAN_CONNECTION_ATTRIBUTES>(&guid, wlan_intf_opcode_current_connection)
            && c.isState == wlan_interface_state_connected
        {
            let a = c.wlanAssociationAttributes;
            d.signal_quality = Some(a.wlanSignalQuality.min(100) as u8);
            d.rx_rate_mbps = Some(a.ulRxRate / 1000);
        }
        d.rssi_dbm = wlan.query::<i32>(&guid, wlan_intf_opcode_rssi);
        d.channel = wlan.query::<u32>(&guid, wlan_intf_opcode_channel_number);
        d
    }

    /// 연결된 WLAN 인터페이스(GUID, 설명)(읽기 전용).
    pub fn connected_wlans() -> Vec<(u128, String)> {
        let Ok(wlan) = Wlan::open() else {
            return Vec::new();
        };
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        // SAFETY: 출력 인자는 지역 변수다. 돌려받은 목록은 아래에서 WlanFreeMemory로 놓는다.
        if unsafe { WlanEnumInterfaces(wlan.0, None, &mut list) } != 0 || list.is_null() {
            return Vec::new();
        }
        // SAFETY: list는 dwNumberOfItems개의 항목을 이어 담고 있다.
        let found = unsafe {
            let n = (*list).dwNumberOfItems as usize;
            std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), n)
                .iter()
                .filter(|i| i.isState == wlan_interface_state_connected)
                .map(|i| (i.InterfaceGuid.to_u128(), wide(&i.strInterfaceDescription)))
                .collect()
        };
        // SAFETY: WlanEnumInterfaces가 준 메모리다.
        unsafe { WlanFreeMemory(list.cast::<c_void>()) };
        found
    }

    /// `WlanOpenHandle` 핸들. 켜 두는 동안 열어 두고, 끌 때 닫는다.
    struct OpenHandle(Wlan);

    // SAFETY: WLAN 클라이언트 핸들은 스레드 사이에 넘겨도 된다(어느 스레드에서나 쓸 수 있다).
    unsafe impl Send for OpenHandle {}

    /// 실제 `WlanSetInterface`(`wlan_intf_opcode_media_streaming_mode`).
    #[derive(Default)]
    pub struct SysBackend {
        handles: HashMap<u128, OpenHandle>,
    }

    fn set_mode(wlan: &Wlan, guid: &GUID, on: bool) -> Result<(), String> {
        let value: i32 = i32::from(on); // BOOL
        // SAFETY: pdata는 4바이트 BOOL 하나를 가리킨다.
        let rc = unsafe {
            WlanSetInterface(
                wlan.0,
                guid,
                wlan_intf_opcode_media_streaming_mode,
                std::mem::size_of::<i32>() as u32,
                (&value as *const i32).cast::<c_void>(),
                None,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(format!("WlanSetInterface 오류 {rc}"))
        }
    }

    impl ModeBackend for SysBackend {
        fn set(&mut self, iface: u128, on: bool) -> Result<(), String> {
            let guid = GUID::from_u128(iface);
            if on {
                let wlan = Wlan::open().map_err(|rc| format!("WlanOpenHandle 오류 {rc}"))?;
                set_mode(&wlan, &guid, true)?;
                self.handles.insert(iface, OpenHandle(wlan));
                Ok(())
            } else {
                // 핸들을 닫으면 mode도 풀리지만, 명시적으로 되돌린다.
                let handle = self.handles.remove(&iface);
                match handle {
                    Some(h) => set_mode(&h.0, &guid, false),
                    None => Ok(()),
                }
            }
        }
    }
}

#[cfg(not(windows))]
mod sys {
    use std::net::IpAddr;

    use super::{IfInfo, ModeBackend, WlanDetails};

    pub fn best_if(_target: IpAddr) -> Option<IfInfo> {
        None
    }

    pub fn default_if(_prefer_v6: bool) -> Option<IfInfo> {
        None
    }

    pub fn wlan_details(_iface: u128) -> WlanDetails {
        WlanDetails::default()
    }

    pub fn connected_wlans() -> Vec<(u128, String)> {
        Vec::new()
    }

    #[derive(Default)]
    pub struct SysBackend;

    impl ModeBackend for SysBackend {
        fn set(&mut self, _iface: u128, _on: bool) -> Result<(), String> {
            Err("Windows에서만 쓸 수 있습니다".to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(if_type: u32, guid: u128) -> Option<IfInfo> {
        Some(IfInfo {
            if_type,
            guid,
            description: format!("if{guid}"),
        })
    }

    #[test]
    fn if_type_mapping() {
        assert_eq!(kind_of_if_type(71), Some(NetKind::Wifi));
        assert_eq!(kind_of_if_type(6), Some(NetKind::Wired));
        assert_eq!(kind_of_if_type(24), Some(NetKind::Other));
        // 터널(131), PPP(23), propVirtual(53), other(1)은 가상 어댑터로 본다.
        for t in [131, 23, 53, 1, 0] {
            assert_eq!(kind_of_if_type(t), None, "{t}");
        }
    }

    #[test]
    fn judge_direct_adapters() {
        let (kind, tunnel, wifi) = judge(iface(71, 7), || panic!("기본 경로를 볼 필요 없다"));
        assert_eq!(
            (kind, tunnel, wifi.map(|w| w.guid)),
            (NetKind::Wifi, false, Some(7))
        );
        let (kind, tunnel, wifi) = judge(iface(6, 7), || panic!("기본 경로를 볼 필요 없다"));
        assert_eq!((kind, tunnel, wifi), (NetKind::Wired, false, None));
        let (kind, tunnel, wifi) = judge(iface(24, 1), || panic!("기본 경로를 볼 필요 없다"));
        assert_eq!((kind, tunnel, wifi), (NetKind::Other, false, None));
    }

    #[test]
    fn judge_tunnel_uses_the_default_route_adapter() {
        let (kind, tunnel, wifi) = judge(iface(131, 9), || iface(71, 7));
        assert_eq!(
            (kind, tunnel, wifi.map(|w| w.guid)),
            (NetKind::Wifi, true, Some(7))
        );
        let (kind, tunnel, wifi) = judge(iface(53, 9), || iface(6, 5));
        assert_eq!((kind, tunnel, wifi), (NetKind::Wired, true, None));
        // 기본 경로도 가상이거나 못 보면 other(터널 표시 없음).
        let (kind, tunnel, _) = judge(iface(131, 9), || iface(131, 8));
        assert_eq!((kind, tunnel), (NetKind::Other, false));
        let (kind, tunnel, _) = judge(iface(131, 9), || None);
        assert_eq!((kind, tunnel), (NetKind::Other, false));
    }

    #[test]
    fn judge_unknown_when_the_lookup_fails() {
        let (kind, tunnel, wifi) = judge(None, || panic!("볼 필요 없다"));
        assert_eq!((kind, tunnel, wifi), (NetKind::Unknown, false, None));
    }

    #[test]
    fn network_path_json_uses_camel_case_and_skips_missing_values() {
        let unknown = NetworkPath::unknown();
        assert_eq!(
            serde_json::to_value(&unknown).unwrap(),
            serde_json::json!({
                "kind": "unknown", "viaTunnel": false, "signalQuality": null, "rssiDbm": null,
                "rxRateMbps": null, "channel": null, "streamingMode": false
            })
        );
        let wifi = NetworkPath {
            kind: NetKind::Wifi,
            via_tunnel: true,
            signal_quality: Some(80),
            rssi_dbm: Some(-55),
            rx_rate_mbps: Some(866),
            channel: Some(36),
            streaming_mode: true,
        };
        assert_eq!(
            serde_json::to_value(&wifi).unwrap(),
            serde_json::json!({
                "kind": "wifi", "viaTunnel": true, "signalQuality": 80, "rssiDbm": -55,
                "rxRateMbps": 866, "channel": 36, "streamingMode": true
            })
        );
        for (kind, text) in [(NetKind::Wired, "wired"), (NetKind::Other, "other")] {
            let json = serde_json::to_value(NetworkPath::of(kind, false)).unwrap();
            assert_eq!(json["kind"], text);
        }
    }

    #[test]
    fn summary_lists_what_was_read() {
        let mut p = NetworkPath::of(NetKind::Wifi, false);
        p.signal_quality = Some(70);
        p.channel = Some(6);
        let s = p.summary();
        assert!(
            s.contains("Wifi") && s.contains("signal=70%") && s.contains("channel=6"),
            "{s}"
        );
        assert!(
            s.contains("streaming_mode=false") && !s.contains("rssi"),
            "{s}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn loopback_route_is_other_and_never_touches_streaming_mode() {
        let path = route_path(IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(path.kind, NetKind::Other, "{path:?}");
        assert!(!path.via_tunnel && !path.streaming_mode);
        let path = route_path(IpAddr::V6(Ipv6Addr::LOCALHOST));
        assert!(
            matches!(path.kind, NetKind::Other | NetKind::Unknown),
            "{path:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn public_route_lookup_does_not_panic() {
        // 이 PC의 실제 경로(읽기 전용). 종류는 PC마다 다르므로 값은 보지 않는다.
        for ip in [PROBE_V4, PROBE_V6] {
            let path = route_path(ip);
            assert!(!path.streaming_mode, "{path:?}");
            if path.kind != NetKind::Wifi {
                assert!(
                    path.signal_quality.is_none() && path.channel.is_none(),
                    "{path:?}"
                );
            }
        }
    }

    /// 켜고 끈 기록만 남기는 가짜(`WlanSetInterface`를 부르지 않는다).
    #[derive(Default)]
    struct Fake {
        log: Vec<(u128, bool)>,
        fail_on: bool,
    }

    impl ModeBackend for Fake {
        fn set(&mut self, iface: u128, on: bool) -> Result<(), String> {
            if self.fail_on && on {
                return Err("연결 안 됨".to_owned());
            }
            self.log.push((iface, on));
            Ok(())
        }
    }

    fn watch(key: &str, session: u64) -> Holder {
        Holder::Watch {
            key: key.to_owned(),
            session,
        }
    }

    #[test]
    fn same_holder_twice_counts_once() {
        let mut m = Manager::new(Fake::default());
        assert!(m.acquire(&watch("a", 1), 7, "wifi", "시청"));
        assert!(m.acquire(&watch("a", 1), 7, "wifi", "시청"));
        assert_eq!(m.holder_count(7), 1);
        assert_eq!(m.backend.log, [(7, true)]);
        m.release_where(|h| *h == watch("a", 1), "끝");
        assert!(!m.is_on(7));
        assert_eq!(m.backend.log, [(7, true), (7, false)]);
    }

    #[test]
    fn last_holder_turns_the_mode_off() {
        let mut m = Manager::new(Fake::default());
        m.acquire(&watch("a", 1), 7, "wifi", "시청");
        m.acquire(&Holder::Broadcast, 7, "wifi", "송출");
        m.acquire(&watch("b", 2), 8, "wifi2", "시청");
        assert_eq!(m.backend.log, [(7, true), (8, true)]);
        m.release_where(|h| *h == watch("a", 1), "끝");
        assert!(m.is_on(7) && m.is_on(8));
        m.release_where(|h| *h == Holder::Broadcast, "끝");
        assert!(!m.is_on(7) && m.is_on(8));
        assert_eq!(m.backend.log.last(), Some(&(7, false)));
        // 놓지 않은 holder를 놓아도 아무 일 없다.
        m.release_where(|h| *h == watch("zzz", 9), "끝");
        assert!(m.is_on(8));
    }

    #[test]
    fn release_by_key_and_all() {
        let mut m = Manager::new(Fake::default());
        m.acquire(&watch("a", 1), 7, "wifi", "시청");
        m.acquire(&watch("a", 2), 7, "wifi", "시청");
        m.acquire(&Holder::Broadcast, 7, "wifi", "송출");
        m.release_where(
            |h| matches!(h, Holder::Watch { key, .. } if key == "a"),
            "연결 끊김",
        );
        assert_eq!(m.holder_count(7), 1);
        m.release_where(|_| true, "앱 종료");
        assert!(!m.is_on(7));
        assert_eq!(m.backend.log, [(7, true), (7, false)]);
    }

    #[test]
    fn failing_to_turn_on_is_not_an_error_and_holds_nothing() {
        let mut m = Manager::new(Fake {
            fail_on: true,
            ..Fake::default()
        });
        assert!(!m.acquire(&watch("a", 1), 7, "wifi", "시청"));
        assert!(!m.is_on(7));
        // 놓아도 끄는 호출이 나가지 않는다.
        m.release_where(|_| true, "끝");
        assert!(m.backend.log.is_empty());
    }
}
