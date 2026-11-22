//! 로컬 방송 감지 패킷(D-030, D-039).
//!
//! 클라이언트가 UDP 7884로 질의를 보내면 서버가 유니캐스트로 응답한다. 서버는 같은 UDP 소켓을
//! WebRTC(STUN·DTLS·RTP)와 같이 쓰므로, 첫 바이트로 구분한다. RFC 7983에서 첫 바이트
//! 80~127은 WebRTC가 쓰지 않는 범위라 `d`(100)로 시작하는 매직 값을 쓴다.
//!
//! - 질의: `dsq1` + nonce(8바이트, big endian) = 12바이트
//! - 응답: `dsr1` + JSON([`DiscoveryResponse`]) 한 조각 이상
//!
//! # 조각(D-039 갱신)
//!
//! 응답 패킷 하나는 [`MAX_RESPONSE_BYTES`](1200)를 넘지 않는다(IP 조각화를 피한다). 방송이 많아 한 패킷에
//! 다 담기지 않으면 서버가 질의 하나에 응답 패킷을 여러 개(조각) 보낸다. 조각마다 서버 정보(지문·이름·포트·
//! `hostOnly`·nonce)를 모두 담고 방송 목록만 나눠 담는다. JSON 필드 `part`(0부터)와 `parts`(≥1)가
//! 몇 번째 조각인지 알린다(조각이 하나여도 쓴다).
//!
//! - 조각 수는 [`MAX_PARTS`]까지다. 방송 이름은 [`MAX_DISPLAY_NAME_CHARS`], 서버 이름은
//!   [`MAX_SERVER_NAME_CHARS`]글자로 잘라, 방송 하나는 어떤 경우에도 조각 하나에 들어간다. 그래도
//!   [`MAX_PARTS`]개에 안 담기는 방송(32개를 넘는 경우)은 뒤에서부터 버린다.
//! - 받는 쪽은 (서버 지문, 응답 출발지 IP)마다 [`PartAssembler`]로 조각을 모은다. 조각은 순서가 바뀌거나
//!   겹쳐서(재전송) 와도 된다. 다 모이지 않은 채 끝나면 모인 만큼만 [`PartAssembler::merge`]로 합쳐 쓸 수 있다.

use std::collections::BTreeMap;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::fingerprint::Fingerprint;
use crate::id::InstallId;
use crate::media::VideoCodec;
use crate::proto::MAX_DISPLAY_NAME_CHARS;

pub const QUERY_MAGIC: [u8; 4] = *b"dsq1";
pub const RESPONSE_MAGIC: [u8; 4] = *b"dsr1";
pub const QUERY_LEN: usize = 12;

/// 응답 패킷 하나의 최대 크기. IP 조각화를 피하려고 작게 잡는다.
pub const MAX_RESPONSE_BYTES: usize = 1200;

/// 질의 하나에 보내는 응답 조각 수의 상한.
pub const MAX_PARTS: u8 = 16;

/// 서버 이름의 최대 글자 수(응답에 담을 때 자른다).
pub const MAX_SERVER_NAME_CHARS: usize = 64;

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

/// 응답 조각 하나(디코딩 결과). `response.broadcasts`는 이 조각에 담긴 방송만이다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponsePart {
    pub response: DiscoveryResponse,
    /// 0부터 센 조각 번호.
    pub part: u8,
    /// 전체 조각 수(1~[`MAX_PARTS`]).
    pub parts: u8,
}

/// 전선 위의 JSON(보내는 쪽).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireOut<'a> {
    nonce: u64,
    fingerprint: Fingerprint,
    name: &'a str,
    port: u16,
    host_only: bool,
    part: u8,
    parts: u8,
    broadcasts: &'a [&'a DiscoveredBroadcast],
}

/// 전선 위의 JSON(받는 쪽).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireIn {
    nonce: u64,
    fingerprint: Fingerprint,
    name: String,
    port: u16,
    host_only: bool,
    part: u8,
    parts: u8,
    broadcasts: Vec<DiscoveredBroadcast>,
}

fn truncate_chars(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

fn encode_part(
    resp: &DiscoveryResponse,
    name: &str,
    part: u8,
    parts: u8,
    broadcasts: &[&DiscoveredBroadcast],
) -> Vec<u8> {
    let mut out = RESPONSE_MAGIC.to_vec();
    let wire = WireOut {
        nonce: resp.nonce,
        fingerprint: resp.fingerprint,
        name,
        port: resp.port,
        host_only: resp.host_only,
        part,
        parts,
        broadcasts,
    };
    serde_json::to_writer(&mut out, &wire).expect("직렬화는 실패하지 않는다");
    out
}

/// 응답을 패킷(조각)으로 인코딩한다. 패킷마다 [`MAX_RESPONSE_BYTES`] 이하이고, 방송이 없어도 조각이 하나는
/// 있다. 방송 이름과 서버 이름은 글자 수 상한으로 자른다. [`MAX_PARTS`]개에 안 담기는 방송은 뒤에서부터 버린다.
pub fn encode_response(resp: &DiscoveryResponse) -> Vec<Vec<u8>> {
    let name = truncate_chars(&resp.name, MAX_SERVER_NAME_CHARS);
    let broadcasts: Vec<DiscoveredBroadcast> = resp
        .broadcasts
        .iter()
        .map(|b| DiscoveredBroadcast {
            name: truncate_chars(&b.name, MAX_DISPLAY_NAME_CHARS).to_owned(),
            ..b.clone()
        })
        .collect();

    // 조각 번호 자릿수가 가장 큰 경우(두 자리)의 머리 크기. 방송 목록은 `[]`로 이미 센다.
    let overhead = encode_part(resp, name, MAX_PARTS - 1, MAX_PARTS, &[]).len();
    let mut groups: Vec<Vec<&DiscoveredBroadcast>> = vec![Vec::new()];
    let mut size = overhead;
    for b in &broadcasts {
        let len = serde_json::to_vec(b)
            .expect("직렬화는 실패하지 않는다")
            .len()
            + 1; // 쉼표
        if size + len > MAX_RESPONSE_BYTES {
            if groups.last().is_some_and(|g| g.is_empty()) {
                continue; // 이름 상한이 있어 일어나지 않는다. 혼자서도 안 들어가는 방송은 건너뛴다.
            }
            groups.push(Vec::new());
            size = overhead;
            if size + len > MAX_RESPONSE_BYTES {
                continue;
            }
        }
        groups.last_mut().expect("조각이 하나는 있다").push(b);
        size += len;
    }
    groups.truncate(usize::from(MAX_PARTS));
    let parts = groups.len() as u8;
    groups
        .iter()
        .enumerate()
        .map(|(i, g)| encode_part(resp, name, i as u8, parts, g))
        .collect()
}

/// 응답 패킷(조각 하나)을 디코딩한다. 조각 번호·개수가 맞지 않거나 JSON이 틀리면 `None`.
pub fn decode_part(buf: &[u8]) -> Option<ResponsePart> {
    if buf.len() < 4 || buf[..4] != RESPONSE_MAGIC || buf.len() > MAX_RESPONSE_BYTES * 2 {
        return None;
    }
    let wire: WireIn = serde_json::from_slice(&buf[4..]).ok()?;
    if wire.parts == 0 || wire.parts > MAX_PARTS || wire.part >= wire.parts {
        return None;
    }
    Some(ResponsePart {
        response: DiscoveryResponse {
            nonce: wire.nonce,
            fingerprint: wire.fingerprint,
            name: wire.name,
            port: wire.port,
            host_only: wire.host_only,
            broadcasts: wire.broadcasts,
        },
        part: wire.part,
        parts: wire.parts,
    })
}

/// 같은 서버(지문)·같은 출발지의 응답 조각을 모아 합친다.
///
/// 조각은 순서가 바뀌거나 겹쳐서 와도 된다(질의를 다시 보내면 같은 조각이 또 온다). 전체 조각 수가
/// 달라지거나 같은 번호 조각의 내용이 달라지면(그 사이 방송 목록이 바뀐 것) 새 응답으로 보고 처음부터 다시 모은다.
#[derive(Debug, Default)]
pub struct PartAssembler {
    parts: u8,
    /// 서버 정보(가장 나중에 받은 조각의 것). `broadcasts`는 비워 둔다.
    head: Option<DiscoveryResponse>,
    got: BTreeMap<u8, Vec<DiscoveredBroadcast>>,
}

impl PartAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// 조각을 넣는다. 이 조각으로 모든 조각이 모였으면 `true`.
    pub fn push(&mut self, mut part: ResponsePart) -> bool {
        let conflict = self.parts != part.parts
            || self
                .got
                .get(&part.part)
                .is_some_and(|prev| *prev != part.response.broadcasts);
        if conflict {
            self.got.clear();
        }
        self.parts = part.parts;
        self.got
            .insert(part.part, std::mem::take(&mut part.response.broadcasts));
        self.head = Some(part.response);
        self.is_complete()
    }

    /// 모든 조각이 모였는지.
    pub fn is_complete(&self) -> bool {
        self.parts > 0 && self.got.len() == usize::from(self.parts)
    }

    /// 아직 못 받은 조각 수(조각을 하나도 못 받았으면 0).
    pub fn missing(&self) -> usize {
        if self.head.is_none() {
            return 0;
        }
        usize::from(self.parts) - self.got.len()
    }

    /// 지금까지 모은 조각을 합친다(조각 번호 순서, 같은 방송자는 한 번만). 조각이 없으면 `None`.
    pub fn merge(&self) -> Option<DiscoveryResponse> {
        let mut resp = self.head.clone()?;
        let mut seen = std::collections::HashSet::new();
        resp.broadcasts = self
            .got
            .values()
            .flatten()
            .filter(|b| seen.insert(b.broadcaster))
            .cloned()
            .collect();
        Some(resp)
    }
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

    /// 글자 수가 `chars`인 한글 이름.
    fn korean(i: usize, chars: usize) -> String {
        let mut name = format!("방송{i:02}");
        while name.chars().count() < chars {
            name.push('가');
        }
        name
    }

    fn broadcast(i: usize, name: String) -> DiscoveredBroadcast {
        let mut id = [0u8; 16];
        id[..8].copy_from_slice(&(i as u64 + 1).to_be_bytes());
        id[8..].copy_from_slice(&[0xA5; 8]);
        DiscoveredBroadcast {
            broadcaster: InstallId::from_bytes(id),
            name,
            codec: VideoCodec::H264,
            width: 3840,
            height: 2160,
            fps: 120,
            locked: i.is_multiple_of(2),
        }
    }

    fn sample_with(names: Vec<String>) -> DiscoveryResponse {
        DiscoveryResponse {
            nonce: u64::MAX,
            fingerprint: Fingerprint::from_bytes([3; 32]),
            name: "거실 PC".into(),
            port: 7884,
            host_only: false,
            broadcasts: names
                .into_iter()
                .enumerate()
                .map(|(i, n)| broadcast(i, n))
                .collect(),
        }
    }

    fn sample(n: usize) -> DiscoveryResponse {
        sample_with((0..n).map(|i| korean(i, MAX_DISPLAY_NAME_CHARS)).collect())
    }

    fn decode_all(packets: &[Vec<u8>]) -> Vec<ResponsePart> {
        packets
            .iter()
            .map(|p| decode_part(p).expect("디코딩되어야 합니다"))
            .collect()
    }

    fn assemble(parts: Vec<ResponsePart>) -> (PartAssembler, bool) {
        let mut a = PartAssembler::new();
        let mut done = false;
        for p in parts {
            done = a.push(p);
        }
        (a, done)
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
    fn small_response_is_one_packet() {
        let r = sample(2);
        let packets = encode_response(&r);
        assert_eq!(packets.len(), 1);
        let part = decode_part(&packets[0]).unwrap();
        assert_eq!((part.part, part.parts), (0, 1));
        assert_eq!(part.response, r);
    }

    #[test]
    fn empty_response_is_one_packet() {
        let r = sample(0);
        let packets = encode_response(&r);
        assert_eq!(packets.len(), 1);
        let (a, done) = assemble(decode_all(&packets));
        assert!(done);
        assert_eq!(a.merge().unwrap(), r);
    }

    /// 고장 5: 방송 32개(최대 길이 한글 이름)가 조각으로 나뉘어 모두 전달된다.
    #[test]
    fn many_long_broadcasts_split_into_parts_and_reassemble() {
        let r = sample(32);
        let packets = encode_response(&r);
        let sizes: Vec<usize> = packets.iter().map(Vec::len).collect();
        assert!(
            packets.len() > 1 && packets.len() <= usize::from(MAX_PARTS),
            "{sizes:?}"
        );
        assert!(sizes.iter().all(|&n| n <= MAX_RESPONSE_BYTES), "{sizes:?}");
        eprintln!("방송 32개: 조각 {}개, 크기 {sizes:?}", packets.len());

        let parts = decode_all(&packets);
        for (i, p) in parts.iter().enumerate() {
            assert_eq!(
                (usize::from(p.part), usize::from(p.parts)),
                (i, packets.len())
            );
            // 조각마다 서버 정보 전체가 있다.
            assert_eq!(p.response.fingerprint, r.fingerprint);
            assert_eq!(
                (p.response.name.as_str(), p.response.port),
                ("거실 PC", 7884)
            );
            assert_eq!(p.response.nonce, u64::MAX);
        }
        let (a, done) = assemble(parts);
        assert!(done && a.is_complete() && a.missing() == 0);
        assert_eq!(a.merge().unwrap(), r, "모든 방송이 같은 순서로 와야 합니다");
    }

    #[test]
    fn parts_in_any_order_and_duplicates() {
        let r = sample(32);
        let mut parts = decode_all(&encode_response(&r));
        parts.reverse();
        // 질의를 다시 보내서 같은 조각이 또 왔다.
        parts.extend(parts.clone());
        let (a, done) = assemble(parts);
        assert!(done);
        let merged = a.merge().unwrap();
        // 조각 번호 순서로 합친다.
        assert_eq!(merged, r);
    }

    #[test]
    fn missing_part_gives_partial_result() {
        let r = sample(32);
        let mut parts = decode_all(&encode_response(&r));
        let total = parts.len();
        let lost = parts.remove(1);
        let kept: usize = parts.iter().map(|p| p.response.broadcasts.len()).sum();
        let (mut a, done) = assemble(parts);
        assert!(!done && !a.is_complete());
        assert_eq!(a.missing(), 1);
        let merged = a.merge().unwrap();
        assert_eq!(merged.broadcasts.len(), kept);
        assert!(merged.broadcasts.len() < 32);
        // 빠진 조각이 나중에 오면 완성된다.
        assert!(a.push(lost));
        assert_eq!(a.merge().unwrap(), r);
        assert_eq!(total, usize::from(a.parts));
    }

    #[test]
    fn changed_list_restarts_assembly() {
        let old = decode_all(&encode_response(&sample(32)));
        // 방송이 하나 끝나서 조각 구성이 달라졌다.
        let newer = sample(31);
        let new_parts = decode_all(&encode_response(&newer));
        let mut a = PartAssembler::new();
        for p in old.iter().take(old.len() - 1) {
            a.push(p.clone());
        }
        // 새 응답의 조각이 오면 옛 조각은 버리고 새로 모은다.
        let mut done = false;
        for p in new_parts {
            done = a.push(p);
        }
        assert!(done);
        assert_eq!(a.merge().unwrap(), newer);
    }

    #[test]
    fn extreme_names_still_fit() {
        // 이름 글자마다 4바이트(이모지), 따옴표·역슬래시(JSON에서 두 글자), 서버 이름도 매우 길다.
        let names = [
            "😀".repeat(100),
            "\"".repeat(100),
            "\\".repeat(100),
            "가".repeat(100),
        ];
        let mut r = sample_with((0..32).map(|i| names[i % 4].clone()).collect());
        r.name = "😀".repeat(500);
        let packets = encode_response(&r);
        assert!(packets.len() <= usize::from(MAX_PARTS));
        assert!(packets.iter().all(|p| p.len() <= MAX_RESPONSE_BYTES));
        let (a, done) = assemble(decode_all(&packets));
        assert!(done);
        let merged = a.merge().unwrap();
        assert_eq!(
            merged.broadcasts.len(),
            32,
            "이름을 잘라서 모두 담아야 합니다"
        );
        assert_eq!(merged.name.chars().count(), MAX_SERVER_NAME_CHARS);
        assert!(
            merged
                .broadcasts
                .iter()
                .all(|b| b.name.chars().count() == MAX_DISPLAY_NAME_CHARS)
        );
    }

    #[test]
    fn too_many_broadcasts_are_cut_at_max_parts() {
        let r = sample(200);
        let packets = encode_response(&r);
        assert_eq!(packets.len(), usize::from(MAX_PARTS));
        assert!(packets.iter().all(|p| p.len() <= MAX_RESPONSE_BYTES));
        let (a, done) = assemble(decode_all(&packets));
        assert!(done);
        let merged = a.merge().unwrap();
        assert!(merged.broadcasts.len() < 200);
        // 앞에서부터 이어서 남는다.
        assert_eq!(merged.broadcasts, r.broadcasts[..merged.broadcasts.len()]);
    }

    #[test]
    fn rejects_bad_part_numbers() {
        let json = |part: u8, parts: u8| {
            let mut buf = RESPONSE_MAGIC.to_vec();
            let mut v = serde_json::to_value(sample(1)).unwrap();
            v["part"] = part.into();
            v["parts"] = parts.into();
            serde_json::to_writer(&mut buf, &v).unwrap();
            buf
        };
        assert!(decode_part(&json(0, 2)).is_some());
        assert!(decode_part(&json(1, 2)).is_some());
        assert!(decode_part(&json(2, 2)).is_none(), "번호가 개수 이상");
        assert!(decode_part(&json(0, 0)).is_none(), "개수 0");
        assert!(
            decode_part(&json(0, MAX_PARTS + 1)).is_none(),
            "개수가 상한 초과"
        );
        assert!(decode_part(b"dsr1not json").is_none());
        assert!(decode_part(b"xxxx{}").is_none());
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
