//! 서버 주소: `host[:port]`.
//!
//! - host는 IPv4, IPv6, 도메인(또는 LAN·MagicDNS 호스트 이름) 중 하나다.
//! - IPv6에 포트를 붙일 때는 `[::1]:7884`처럼 대괄호로 감싼다.
//! - 포트를 생략하면 기본 포트 [`DEFAULT_PORT`](7884, D-028)를 쓴다.
//! - 문자열로 바꿀 때 기본 포트는 생략한다(초대 코드 형식, D-029).

use std::fmt;
use std::net::{IpAddr, Ipv6Addr};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// 기본 포트 (D-028). TCP는 시그널링, UDP는 미디어와 로컬 감지에 쓴다.
pub const DEFAULT_PORT: u16 = 7884;

const MAX_DOMAIN_LEN: usize = 253;
const MAX_LABEL_LEN: usize = 63;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Host {
    Ip(IpAddr),
    /// 소문자로 정규화한 도메인 또는 호스트 이름.
    Domain(String),
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Host::Ip(IpAddr::V6(ip)) => write!(f, "[{ip}]"),
            Host::Ip(IpAddr::V4(ip)) => write!(f, "{ip}"),
            Host::Domain(name) => f.write_str(name),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ServerAddr {
    host: Host,
    port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AddrError {
    #[error("주소를 입력해 주세요")]
    Empty,
    #[error("http:// 같은 접두어는 빼고 주소만 입력해 주세요")]
    HasScheme,
    #[error("포트가 올바르지 않습니다: {0} (1~65535)")]
    InvalidPort(String),
    #[error("주소 형식이 올바르지 않습니다: {0}")]
    InvalidHost(String),
}

impl ServerAddr {
    pub fn new(host: Host, port: u16) -> Self {
        Self { host, port }
    }

    pub fn host(&self) -> &Host {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl fmt::Display for ServerAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.host)?;
        if self.port != DEFAULT_PORT {
            write!(f, ":{}", self.port)?;
        }
        Ok(())
    }
}

impl FromStr for ServerAddr {
    type Err = AddrError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.is_empty() {
            return Err(AddrError::Empty);
        }
        if input.contains("://") {
            return Err(AddrError::HasScheme);
        }

        // `[IPv6]` 또는 `[IPv6]:port`
        if let Some(rest) = input.strip_prefix('[') {
            let (inside, after) = rest
                .split_once(']')
                .ok_or_else(|| AddrError::InvalidHost(input.to_owned()))?;
            let ip: Ipv6Addr = inside
                .parse()
                .map_err(|_| AddrError::InvalidHost(input.to_owned()))?;
            let port = match after {
                "" => DEFAULT_PORT,
                _ => {
                    let port = after
                        .strip_prefix(':')
                        .ok_or_else(|| AddrError::InvalidHost(input.to_owned()))?;
                    parse_port(port)?
                }
            };
            return Ok(Self::new(Host::Ip(IpAddr::V6(ip)), port));
        }

        // 콜론이 두 개 이상이면 대괄호 없는 IPv6로 본다(포트 없음).
        if input.matches(':').count() >= 2 {
            let ip: Ipv6Addr = input
                .parse()
                .map_err(|_| AddrError::InvalidHost(input.to_owned()))?;
            return Ok(Self::new(Host::Ip(IpAddr::V6(ip)), DEFAULT_PORT));
        }

        let (host, port) = match input.split_once(':') {
            Some((host, port)) => (host, parse_port(port)?),
            None => (input, DEFAULT_PORT),
        };
        Ok(Self::new(parse_host(host)?, port))
    }
}

impl TryFrom<String> for ServerAddr {
    type Error = AddrError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<ServerAddr> for String {
    fn from(addr: ServerAddr) -> Self {
        addr.to_string()
    }
}

fn parse_port(text: &str) -> Result<u16, AddrError> {
    match text.parse::<u16>() {
        Ok(port) if port != 0 => Ok(port),
        _ => Err(AddrError::InvalidPort(text.to_owned())),
    }
}

fn parse_host(text: &str) -> Result<Host, AddrError> {
    if let Ok(ip) = text.parse::<IpAddr>() {
        return Ok(Host::Ip(ip));
    }
    // 숫자와 점만 있는데 IPv4로 읽히지 않으면 잘못 입력한 IP로 본다(예: 192.168.0.300).
    if text.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Err(AddrError::InvalidHost(text.to_owned()));
    }
    let valid_label = |label: &str| {
        !label.is_empty()
            && label.len() <= MAX_LABEL_LEN
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    if text.len() > MAX_DOMAIN_LEN || !text.split('.').all(valid_label) {
        return Err(AddrError::InvalidHost(text.to_owned()));
    }
    Ok(Host::Domain(text.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(text: &str) -> ServerAddr {
        text.parse().unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    #[test]
    fn parses_ipv4_with_and_without_port() {
        assert_eq!(addr("192.168.0.10").port(), DEFAULT_PORT);
        assert_eq!(addr("192.168.0.10:9000").port(), 9000);
        assert_eq!(addr("  10.0.0.1  ").to_string(), "10.0.0.1");
    }

    #[test]
    fn parses_ipv6_forms() {
        assert_eq!(addr("::1").to_string(), "[::1]");
        assert_eq!(addr("[::1]").port(), DEFAULT_PORT);
        assert_eq!(
            addr("[fd7a:115c:a1e0::1]:8000").to_string(),
            "[fd7a:115c:a1e0::1]:8000"
        );
    }

    #[test]
    fn parses_domains_and_hostnames() {
        assert_eq!(addr("Example.COM:7885").to_string(), "example.com:7885");
        assert_eq!(addr("my-pc").host(), &Host::Domain("my-pc".into()));
        assert_eq!(addr("laptop.tail1234.ts.net").port(), DEFAULT_PORT);
    }

    #[test]
    fn default_port_is_omitted_in_display() {
        assert_eq!(addr("example.com:7884").to_string(), "example.com");
    }

    #[test]
    fn rejects_invalid_input() {
        assert_eq!("".parse::<ServerAddr>(), Err(AddrError::Empty));
        assert!(matches!(
            "host:0".parse::<ServerAddr>(),
            Err(AddrError::InvalidPort(_))
        ));
        assert!(matches!(
            "host:99999".parse::<ServerAddr>(),
            Err(AddrError::InvalidPort(_))
        ));
        assert!(matches!(
            "192.168.0.300".parse::<ServerAddr>(),
            Err(AddrError::InvalidHost(_))
        ));
        assert!(matches!(
            "-bad.com".parse::<ServerAddr>(),
            Err(AddrError::InvalidHost(_))
        ));
        assert!(matches!(
            "a..b".parse::<ServerAddr>(),
            Err(AddrError::InvalidHost(_))
        ));
        assert_eq!(
            "http://x.com".parse::<ServerAddr>(),
            Err(AddrError::HasScheme)
        );
        assert!(matches!(
            "[::1".parse::<ServerAddr>(),
            Err(AddrError::InvalidHost(_))
        ));
        assert!(matches!(
            "한글.com".parse::<ServerAddr>(),
            Err(AddrError::InvalidHost(_))
        ));
    }

    #[test]
    fn serde_roundtrip_as_string() {
        let original = addr("[::1]:9000");
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, "\"[::1]:9000\"");
        assert_eq!(serde_json::from_str::<ServerAddr>(&json).unwrap(), original);
    }
}
