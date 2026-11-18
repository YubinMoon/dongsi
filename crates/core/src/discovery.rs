//! 로컬 방송 감지 패킷(D-030).
//!
//! 클라이언트가 UDP 7884로 질의를 보내면 서버가 유니캐스트로 응답한다. 서버는 같은 UDP 소켓을
//! WebRTC(STUN·DTLS·RTP)와 같이 쓰므로, 첫 바이트로 구분한다. RFC 7983에서 첫 바이트
//! 80~127은 WebRTC가 쓰지 않는 범위라 `d`(100)로 시작하는 매직 값을 쓴다.
//!
//! - 질의: `dsq1` + nonce(8바이트, big endian) = 12바이트
//! - 응답: `dsr1` + JSON([`DiscoveryResponse`])

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::fingerprint::Fingerprint;
use crate::id::InstallId;
use crate::media::VideoCodec;

pub const QUERY_MAGIC: [u8; 4] = *b"dsq1";
pub const RESPONSE_MAGIC: [u8; 4] = *b"dsr1";
pub const QUERY_LEN: usize = 12;

/// 응답 패킷 최대 크기. IP 조각화를 피하려고 작게 잡는다.
pub const MAX_RESPONSE_BYTES: usize = 1200;

/// 감지 패킷인지(WebRTC 패킷이 아닌지) 첫 바이트로 확인한다.
pub fn is_discovery_packet(buf: &[u8]) -> bool {
    buf.len() >= 4 && (buf[..4] == QUERY_MAGIC || buf[..4] == RESPONSE_MAGIC)
}

pub fn encode_query(nonce: u64) -> [u8; QUERY_LEN] {
    let mut out = [0u8; QUERY_LEN];
    out[..4].copy_from_slice(&QUERY_MAGIC);
    out[4..].copy_from_slice(&nonce.to_be_bytes());
    out
}

pub fn decode_query(buf: &[u8]) -> Option<u64> {
    if buf.len() != QUERY_LEN || buf[..4] != QUERY_MAGIC {
        return None;
    }
    Some(u64::from_be_bytes(buf[4..].try_into().ok()?))
}

/// 감지 응답.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryResponse {
    pub nonce: u64,
    pub fingerprint: Fingerprint,
    pub name: String,
    /// 시그널링 TCP 포트.
    pub port: u16,
    /// 내장 서버(호스트만 방송)인지.
    pub host_only: bool,
    pub broadcasts: Vec<DiscoveredBroadcast>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredBroadcast {
    pub broadcaster: InstallId,
    pub name: String,
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub locked: bool,
}

/// 응답을 인코딩한다. 크기 제한을 넘으면 방송 목록을 뒤에서부터 줄인다.
pub fn encode_response(resp: &DiscoveryResponse) -> Vec<u8> {
    let mut resp = resp.clone();
    loop {
        let mut out = RESPONSE_MAGIC.to_vec();
        serde_json::to_writer(&mut out, &resp).expect("직렬화는 실패하지 않는다");
        if out.len() <= MAX_RESPONSE_BYTES || resp.broadcasts.is_empty() {
            return out;
        }
        resp.broadcasts.pop();
    }
}

pub fn decode_response(buf: &[u8]) -> Option<DiscoveryResponse> {
    if buf.len() < 4 || buf[..4] != RESPONSE_MAGIC || buf.len() > MAX_RESPONSE_BYTES * 2 {
        return None;
    }
    serde_json::from_slice(&buf[4..]).ok()
}

/// 감지 질의에 응답해도 되는 출발지인지: 사설 대역, 링크 로컬, 루프백, CGNAT(Tailscale 100.64.0.0/10).
///
/// 인터넷에 열린 내장 서버가 증폭 공격에 쓰이지 않도록, 이 범위 밖의 질의에는 응답하지 않는다.
pub fn is_local_source(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || (o[0] == 100 && (o[1] & 0b1100_0000) == 64)
        }
        IpAddr::V6(v6) => {
            let seg0 = v6.segments()[0];
            v6.is_loopback()
                || (seg0 & 0xfe00) == 0xfc00 // fc00::/7 (Tailscale fd7a:115c:a1e0::/48 포함)
                || (seg0 & 0xffc0) == 0xfe80 // fe80::/10
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|v4| is_local_source(IpAddr::V4(v4)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(n: usize) -> DiscoveryResponse {
        DiscoveryResponse {
            nonce: 7,
            fingerprint: Fingerprint::from_bytes([3; 32]),
            name: "거실 PC".into(),
            port: 7884,
            host_only: true,
            broadcasts: (0..n)
                .map(|i| DiscoveredBroadcast {
                    broadcaster: InstallId::from_bytes([i as u8; 16]),
                    name: format!("방송자 이름이 꽤 긴 사람 {i}"),
                    codec: VideoCodec::H264,
                    width: 1920,
                    height: 1080,
                    fps: 60,
                    locked: i % 2 == 0,
                })
                .collect(),
        }
    }

    #[test]
    fn query_roundtrip() {
        let q = encode_query(0x0102_0304_0506_0708);
        assert!(is_discovery_packet(&q));
        assert_eq!(decode_query(&q), Some(0x0102_0304_0506_0708));
        assert_eq!(decode_query(&q[..11]), None);
        // STUN binding request(첫 바이트 0x00), DTLS(22), RTP(0x80)는 감지 패킷이 아니다.
        assert!(!is_discovery_packet(&[0x00, 0x01, 0x00, 0x00]));
        assert!(!is_discovery_packet(&[22, 254, 253, 0]));
        assert!(!is_discovery_packet(&[0x80, 0x60, 0, 0]));
    }

    #[test]
    fn response_roundtrip_and_size_limit() {
        let r = sample(2);
        let bytes = encode_response(&r);
        assert_eq!(decode_response(&bytes), Some(r));

        let big = sample(40);
        let bytes = encode_response(&big);
        assert!(bytes.len() <= MAX_RESPONSE_BYTES);
        let decoded = decode_response(&bytes).unwrap();
        assert!(!decoded.broadcasts.is_empty() && decoded.broadcasts.len() < 40);
    }

    #[test]
    fn local_sources() {
        for ip in [
            "192.168.0.10",
            "10.1.2.3",
            "172.20.0.1",
            "169.254.1.1",
            "127.0.0.1",
            "100.90.152.28",
            "fd7a:115c:a1e0::1",
            "fe80::1",
            "::1",
        ] {
            assert!(is_local_source(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "203.0.113.7", "100.128.0.1", "2001:db8::1"] {
            assert!(!is_local_source(ip.parse().unwrap()), "{ip}");
        }
    }
}
