//! 예시 데이터. 서버 연결(Phase 2)과 로컬 방송 감지(D-030)를 구현하기 전까지 화면을 채운다.
//! 화면에는 항상 "예시"라고 표시한다.

use dongsi_core::{Fingerprint, InstallId, Invite, ServerAddr, ServerId};

const NAMES: [&str; 6] = ["하늘", "바다", "구름", "노을", "별빛", "새벽"];
const MODES: [(u32, u32, u32); 4] = [
    (1920, 1080, 60),
    (2560, 1440, 60),
    (3840, 2160, 30),
    (1280, 720, 30),
];

pub struct SampleBroadcast {
    pub broadcaster: InstallId,
    pub name: &'static str,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub locked: bool,
}

/// 서버마다 늘 같은 예시가 나오도록 서버 ID에서 값을 뽑는다(0~3개).
pub fn broadcasts_for(server: ServerId) -> Vec<SampleBroadcast> {
    let seed = *server.as_bytes();
    let count = usize::from(seed[0] % 4);
    (0..count)
        .map(|i| {
            let mut id = seed;
            id[15] ^= u8::try_from(i + 1).unwrap_or(1);
            let (width, height, fps) = MODES[(usize::from(seed[2]) + i) % MODES.len()];
            SampleBroadcast {
                broadcaster: InstallId::from_bytes(id),
                name: NAMES[(usize::from(seed[1]) + i) % NAMES.len()],
                width,
                height,
                fps,
                locked: (usize::from(seed[3]) + i) % 2 == 0,
            }
        })
        .collect()
}

pub struct SampleLocal {
    pub host_name: &'static str,
    pub addr: &'static str,
    pub via: &'static str,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub locked: bool,
}

pub fn local_broadcasts() -> Vec<SampleLocal> {
    vec![
        SampleLocal {
            host_name: "거실 PC",
            addr: "192.168.0.23",
            via: "lan",
            width: 1920,
            height: 1080,
            fps: 60,
            locked: false,
        },
        SampleLocal {
            host_name: "노트북",
            addr: "100.101.102.103",
            via: "tailscale",
            width: 2560,
            height: 1440,
            fps: 60,
            locked: true,
        },
    ]
}

/// 개발용 예시 방송 초대 코드. 지문이 고정이라 두 번 넣으면 "갱신"으로 처리된다.
pub fn invite_code() -> String {
    let addrs: Vec<ServerAddr> = ["192.168.0.23", "100.101.102.103", "203.0.113.7"]
        .iter()
        .map(|a| a.parse().expect("고정된 예시 주소"))
        .collect();
    Invite::for_broadcast(
        addrs,
        Fingerprint::from_bytes([0x5A; Fingerprint::LEN]),
        Some("예시 서버 (개발용)".into()),
        InstallId::from_bytes([0xA5; 16]),
        Some("1234".into()),
    )
    .expect("고정된 예시 값")
    .encode()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_stable_per_server() {
        let id = ServerId::generate();
        let a: Vec<_> = broadcasts_for(id).iter().map(|b| b.broadcaster).collect();
        let b: Vec<_> = broadcasts_for(id).iter().map(|b| b.broadcaster).collect();
        assert_eq!(a, b);
        assert!(a.len() <= 3);
    }

    #[test]
    fn sample_invite_decodes() {
        let invite = Invite::decode(&invite_code()).unwrap();
        assert_eq!(invite.addrs().len(), 3);
        assert_eq!(invite.password(), Some("1234"));
    }
}
