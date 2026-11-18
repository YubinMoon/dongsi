//! 이 PC의 네트워크 인터페이스(IPv4) 목록.

use std::net::{IpAddr, Ipv4Addr};

/// IPv4 인터페이스 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ipv4Iface {
    pub name: String,
    pub ip: Ipv4Addr,
    pub netmask: Ipv4Addr,
    /// 서브넷 브로드캐스트 주소(/31, /32는 없음).
    pub broadcast: Option<Ipv4Addr>,
    pub kind: IfaceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfaceKind {
    Loopback,
    /// Tailscale(100.64.0.0/10).
    Tailscale,
    Lan,
    Other,
}

/// 루프백을 뺀 IPv4 인터페이스.
pub fn ipv4_interfaces() -> Vec<Ipv4Iface> {
    let Ok(list) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for iface in list {
        let if_addrs::IfAddr::V4(v4) = &iface.addr else {
            continue;
        };
        let kind = classify(v4.ip);
        if kind == IfaceKind::Loopback {
            continue;
        }
        let prefix = u32::from(v4.netmask).leading_ones();
        let broadcast = if prefix < 31 {
            Some(Ipv4Addr::from(u32::from(v4.ip) | !u32::from(v4.netmask)))
        } else {
            None
        };
        out.push(Ipv4Iface {
            name: iface.name.clone(),
            ip: v4.ip,
            netmask: v4.netmask,
            broadcast,
            kind,
        });
    }
    out
}

pub fn classify(ip: Ipv4Addr) -> IfaceKind {
    let o = ip.octets();
    if ip.is_loopback() {
        IfaceKind::Loopback
    } else if o[0] == 100 && (o[1] & 0b1100_0000) == 64 {
        IfaceKind::Tailscale
    } else if ip.is_private() || ip.is_link_local() {
        IfaceKind::Lan
    } else {
        IfaceKind::Other
    }
}

/// 이 PC의 IP인지.
pub fn is_own_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || ipv4_interfaces().iter().any(|i| i.ip == v4),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

/// WebRTC 후보로 쓸 대표 LAN IP: 사설 대역 중 기본 경로에 가까운 것(가상 어댑터는 뒤로).
pub fn primary_lan_ip() -> Option<Ipv4Addr> {
    // 라우팅 테이블이 고르는 출발 주소를 쓴다(패킷은 보내지 않는다).
    if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0")
        && sock.connect("8.8.8.8:53").is_ok()
        && let Ok(local) = sock.local_addr()
        && let IpAddr::V4(v4) = local.ip()
        && !v4.is_unspecified()
    {
        return Some(v4);
    }
    ipv4_interfaces()
        .into_iter()
        .find(|i| i.kind == IfaceKind::Lan)
        .map(|i| i.ip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_ranges() {
        assert_eq!(classify("127.0.0.1".parse().unwrap()), IfaceKind::Loopback);
        assert_eq!(
            classify("100.90.1.2".parse().unwrap()),
            IfaceKind::Tailscale
        );
        assert_eq!(classify("192.168.1.2".parse().unwrap()), IfaceKind::Lan);
        assert_eq!(classify("8.8.8.8".parse().unwrap()), IfaceKind::Other);
    }

    #[test]
    fn interfaces_have_broadcast() {
        for i in ipv4_interfaces() {
            if let Some(b) = i.broadcast {
                let mask = u32::from(i.netmask);
                assert_eq!(u32::from(b) & mask, u32::from(i.ip) & mask);
            }
        }
    }
}
