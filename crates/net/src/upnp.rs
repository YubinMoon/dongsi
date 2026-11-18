//! UPnP IGD 포트 매핑(PC 직접 호스팅, D-018).
//!
//! 공유기에 TCP·UDP 같은 번호의 외부 포트를 요청한다. 기본은 내부 포트와 같은 번호(7884)이고,
//! 이미 쓰이고 있으면 공유기가 고른 다른 외부 포트를 받는다.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use igd_next::aio::Gateway;
use igd_next::aio::tokio::{Tokio, search_gateway};
use igd_next::{AddPortError, PortMappingProtocol, SearchOptions};
use tracing::{info, warn};

use crate::NetError;

const LEASE_SECS: u32 = 3600;
const DESCRIPTION: &str = "dongsi";

/// 열어 둔 포트 매핑. [`UpnpMapping::remove`]로 닫는다.
pub struct UpnpMapping {
    gateway: Gateway<Tokio>,
    pub external_ip: IpAddr,
    pub external_port: u16,
    pub local: SocketAddr,
    pub permanent: bool,
}

impl std::fmt::Debug for UpnpMapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpnpMapping")
            .field("external_ip", &self.external_ip)
            .field("external_port", &self.external_port)
            .field("local", &self.local)
            .finish()
    }
}

/// 외부 IP가 실제 공인 주소인지(이중 NAT·CGNAT이면 사설 대역이나 100.64.0.0/10이 나온다).
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || (o[0] == 100 && (o[1] & 0b1100_0000) == 64))
        }
        IpAddr::V6(_) => true,
    }
}

/// 공유기를 찾아 `local_ip:port`로 TCP·UDP 포트를 연다.
pub async fn map_ports(local_ip: Ipv4Addr, port: u16) -> Result<UpnpMapping, NetError> {
    let mut options = SearchOptions::default();
    options.bind_addr = SocketAddr::new(IpAddr::V4(local_ip), 0);
    options.timeout = Some(Duration::from_secs(3));
    options.single_search_timeout = Some(Duration::from_secs(2));
    let gateway = search_gateway(options)
        .await
        .map_err(|e| NetError::Upnp(format!("UPnP 공유기를 찾지 못했습니다: {e}")))?;
    let external_ip = gateway
        .get_external_ip()
        .await
        .map_err(|e| NetError::Upnp(format!("외부 IP를 확인하지 못했습니다: {e}")))?;
    let local = SocketAddr::new(IpAddr::V4(local_ip), port);

    // 1) 같은 번호로 시도 2) 영구 임대만 되는 공유기 3) 다른 외부 포트
    let mut permanent = false;
    let mut external_port = port;
    let mut result = add_pair(&gateway, external_port, local, LEASE_SECS).await;
    if matches!(result, Err(AddPortError::OnlyPermanentLeasesSupported)) {
        permanent = true;
        result = add_pair(&gateway, external_port, local, 0).await;
    }
    if matches!(result, Err(AddPortError::PortInUse)) {
        let lease = if permanent { 0 } else { LEASE_SECS };
        for candidate in (port.saturating_add(1)..).take(20) {
            external_port = candidate;
            result = add_pair(&gateway, external_port, local, lease).await;
            if !matches!(result, Err(AddPortError::PortInUse)) {
                break;
            }
        }
    }
    result.map_err(|e| NetError::Upnp(format!("포트를 열지 못했습니다: {e}")))?;

    info!(%external_ip, external_port, %local, "UPnP 포트 매핑");
    if !is_public_ip(external_ip) {
        warn!(%external_ip, "외부 IP가 공인 주소가 아닙니다(이중 NAT 또는 CGNAT)");
    }
    Ok(UpnpMapping {
        gateway,
        external_ip,
        external_port,
        local,
        permanent,
    })
}

async fn add_pair(
    gateway: &Gateway<Tokio>,
    external_port: u16,
    local: SocketAddr,
    lease: u32,
) -> Result<(), AddPortError> {
    gateway
        .add_port(
            PortMappingProtocol::TCP,
            external_port,
            local,
            lease,
            DESCRIPTION,
        )
        .await?;
    if let Err(e) = gateway
        .add_port(
            PortMappingProtocol::UDP,
            external_port,
            local,
            lease,
            DESCRIPTION,
        )
        .await
    {
        let _ = gateway
            .remove_port(PortMappingProtocol::TCP, external_port)
            .await;
        return Err(e);
    }
    Ok(())
}

impl UpnpMapping {
    /// 임대를 갱신한다(임대 시간의 절반마다 부른다).
    pub async fn renew(&self) -> Result<(), NetError> {
        if self.permanent {
            return Ok(());
        }
        add_pair(&self.gateway, self.external_port, self.local, LEASE_SECS)
            .await
            .map_err(|e| NetError::Upnp(format!("포트 임대를 갱신하지 못했습니다: {e}")))
    }

    pub fn renew_interval() -> Duration {
        Duration::from_secs(u64::from(LEASE_SECS) / 2)
    }

    /// 매핑을 지운다.
    pub async fn remove(&self) {
        for proto in [PortMappingProtocol::TCP, PortMappingProtocol::UDP] {
            if let Err(e) = self.gateway.remove_port(proto, self.external_port).await {
                warn!("UPnP 매핑을 지우지 못했습니다({proto:?}): {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_ip_detection() {
        assert!(is_public_ip("203.0.113.7".parse().unwrap()));
        assert!(!is_public_ip("192.168.0.1".parse().unwrap()));
        assert!(!is_public_ip("100.70.0.1".parse().unwrap()));
        assert!(!is_public_ip("10.0.0.1".parse().unwrap()));
    }
}
