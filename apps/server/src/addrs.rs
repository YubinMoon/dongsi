//! 초대 코드에 넣을 주소를 고른다.

use std::net::{IpAddr, Ipv4Addr};

use dongsi_core::{Host, ServerAddr};
use dongsi_net::ifaces::{IfaceKind, Ipv4Iface};
use tracing::warn;

pub const MAX_INVITE_ADDRS: usize = 6;

pub struct InviteAddrs {
    pub addrs: Vec<ServerAddr>,
    /// 쓸 만한 주소가 없어 127.0.0.1을 넣었다(같은 PC에서만 닿는다).
    pub loopback_fallback: bool,
}

/// 초대 코드에 넣을 주소: 설정한 주소, Tailscale, LAN 순서.
///
/// - 설정한 주소(`configured`)에 포트를 붙이지 않았으면 `public_port`(밖에서 보이는 포트)를 쓴다.
/// - 인터페이스 주소(`auto`일 때만)는 실제로 듣는 `port`를 쓴다.
pub fn invite_addrs(
    configured: &[String],
    auto: bool,
    public_port: u16,
    port: u16,
    ifaces: Vec<Ipv4Iface>,
) -> InviteAddrs {
    let mut out: Vec<ServerAddr> = Vec::new();
    for a in configured {
        match ServerAddr::parse_with_default_port(a, public_port) {
            Ok(addr) => out.push(addr),
            Err(e) => warn!("주소를 무시합니다({a}): {e}"),
        }
    }
    if auto {
        let mut ifaces = ifaces;
        ifaces.sort_by_key(|i| match i.kind {
            IfaceKind::Tailscale => 0,
            IfaceKind::Lan => 1,
            _ => 2,
        });
        for i in ifaces {
            if matches!(i.kind, IfaceKind::Tailscale | IfaceKind::Lan) {
                out.push(ServerAddr::new(Host::Ip(IpAddr::V4(i.ip)), port));
            }
        }
    }
    let mut loopback_fallback = false;
    if out.is_empty() {
        loopback_fallback = true;
        out.push(ServerAddr::new(
            Host::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            public_port,
        ));
    }
    let mut seen = Vec::new();
    out.retain(|a| {
        let key = a.to_string();
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
    out.truncate(MAX_INVITE_ADDRS);
    InviteAddrs {
        addrs: out,
        loopback_fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(ip: [u8; 4], kind: IfaceKind) -> Ipv4Iface {
        Ipv4Iface {
            name: "t".into(),
            ip: Ipv4Addr::from(ip),
            netmask: Ipv4Addr::new(255, 255, 255, 0),
            broadcast: None,
            kind,
        }
    }

    fn strings(r: &InviteAddrs) -> Vec<String> {
        r.addrs.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn configured_first_and_use_public_port() {
        let r = invite_addrs(
            &["example.com".into(), "203.0.113.7:9000".into()],
            false,
            7885,
            7884,
            vec![],
        );
        assert_eq!(strings(&r), ["example.com:7885", "203.0.113.7:9000"]);
        assert!(!r.loopback_fallback);
    }

    #[test]
    fn default_public_port_hides_port() {
        let r = invite_addrs(&["example.com".into()], false, 7884, 7884, vec![]);
        assert_eq!(strings(&r), ["example.com"]);
    }

    #[test]
    fn auto_addresses_use_actual_port_after_configured() {
        let ifaces = vec![
            iface([192, 168, 0, 5], IfaceKind::Lan),
            iface([100, 90, 1, 2], IfaceKind::Tailscale),
            iface([10, 9, 9, 9], IfaceKind::Other),
        ];
        let r = invite_addrs(&["example.com".into()], true, 9000, 8353, ifaces);
        assert_eq!(
            strings(&r),
            ["example.com:9000", "100.90.1.2:8353", "192.168.0.5:8353"]
        );
    }

    #[test]
    fn auto_off_ignores_interfaces() {
        let ifaces = vec![iface([172, 17, 0, 2], IfaceKind::Lan)];
        let r = invite_addrs(&["example.com".into()], false, 7884, 7884, ifaces);
        assert_eq!(strings(&r), ["example.com"]);
    }

    #[test]
    fn falls_back_to_loopback() {
        let r = invite_addrs(
            &[],
            false,
            9000,
            7884,
            vec![iface([172, 17, 0, 2], IfaceKind::Lan)],
        );
        assert_eq!(strings(&r), ["127.0.0.1:9000"]);
        assert!(r.loopback_fallback);
        let r = invite_addrs(&[], true, 7884, 7884, vec![]);
        assert!(r.loopback_fallback);
    }

    #[test]
    fn dedup_and_limit() {
        let r = invite_addrs(
            &["a.example".into(), "a.example".into()],
            false,
            7884,
            7884,
            vec![],
        );
        assert_eq!(strings(&r), ["a.example"]);
        let many: Vec<String> = (0..10).map(|i| format!("h{i}.example")).collect();
        assert_eq!(
            invite_addrs(&many, false, 7884, 7884, vec![]).addrs.len(),
            MAX_INVITE_ADDRS
        );
    }

    #[test]
    fn invalid_configured_is_skipped() {
        let r = invite_addrs(
            &["http://x".into(), "ok.example".into()],
            false,
            7884,
            7884,
            vec![],
        );
        assert_eq!(strings(&r), ["ok.example"]);
    }
}
