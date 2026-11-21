//! 게임 창 따라가기(D-069): 롤(League of Legends) 클라이언트나 게임 창을 방송하면, 게임이 시작될 때
//! 게임 창으로, 게임이 끝나면 클라이언트 창으로 캡처 대상을 바꾼다. 소리가 "캡처하는 앱의 소리만"이면
//! 소리도 그 창의 프로세스로 바꾼다.
//!
//! 롤 전용 규칙이다(사용자 결정). 창은 실행 파일 이름으로 가린다. 이름은 공개 자료로 짐작한 것이고
//! 실제 롤에서 확인하지 않았다(research.md "롤 창 따라가기").

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use dongsi_media::{AudioControl, AudioSource, SourceInfo, SourceKind, VideoPipeline};
use serde::Serialize;
use tracing::{info, warn};

/// 롤 클라이언트 창의 실행 파일. 창은 LeagueClientUx.exe가 연다고 알려져 있고, LeagueClient.exe는 혹시 몰라 같이 본다.
pub const LOL_CLIENT_EXES: [&str; 2] = ["LeagueClientUx.exe", "LeagueClient.exe"];
/// 롤 게임 창의 실행 파일.
pub const LOL_GAME_EXES: [&str; 1] = ["League of Legends.exe"];

/// 창 목록을 다시 보는 간격.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(1);
/// 지금 창이 닫히고 다음 창이 뜨기까지 방송을 끝내지 않고 기다리는 시간. "게임 중 클라이언트 닫기"를
/// 켜 두면 게임이 끝나고 클라이언트가 다시 뜨기까지 몇 초~수십 초가 걸린다. 넘으면 방송이 끝난다.
pub const HOLD: Duration = Duration::from_secs(60);

/// 따라가기 규칙.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FollowKind {
    /// 롤: 게임 창이 있으면 게임 창, 없으면 클라이언트 창.
    Lol,
}

fn exe_is(s: &SourceInfo, names: &[&str]) -> bool {
    s.kind == SourceKind::Window
        && s.process_name
            .as_deref()
            .is_some_and(|n| names.iter().any(|x| x.eq_ignore_ascii_case(n)))
}

/// 이 캡처 대상에 쓸 따라가기 규칙. 롤 클라이언트·게임 창이면 `Lol`.
pub fn follow_kind(s: &SourceInfo) -> Option<FollowKind> {
    (exe_is(s, &LOL_CLIENT_EXES) || exe_is(s, &LOL_GAME_EXES)).then_some(FollowKind::Lol)
}

/// 지금 방송할 롤 창: 게임 창이 있으면 게임 창, 없으면 클라이언트 창(여럿이면 가장 큰 것). 둘 다 없으면 `None`.
pub fn lol_target(windows: &[SourceInfo]) -> Option<&SourceInfo> {
    let biggest = |names: &[&str]| {
        windows
            .iter()
            .filter(|s| exe_is(s, names))
            .max_by_key(|s| u64::from(s.width) * u64::from(s.height))
    };
    biggest(&LOL_GAME_EXES).or_else(|| biggest(&LOL_CLIENT_EXES))
}

/// 지금 캡처 대상(방송 상태와 따라가기가 같이 쓴다).
pub type CurrentSource = Arc<Mutex<SourceInfo>>;

pub fn lock_source(s: &CurrentSource) -> MutexGuard<'_, SourceInfo> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

/// 방송 감시 스레드가 부르는 따라가기 상태.
pub struct Follower {
    kind: FollowKind,
    current: CurrentSource,
    /// 소리가 앱 소리 모드면 소리 대상을 바꿀 손잡이.
    audio: Option<AudioControl>,
    next_check: Instant,
}

impl Follower {
    pub fn new(kind: FollowKind, current: CurrentSource, audio: Option<AudioControl>) -> Self {
        Self {
            kind,
            current,
            audio,
            next_check: Instant::now() + CHECK_INTERVAL,
        }
    }

    /// 간격마다 창 목록을 보고, 방송할 창이 바뀌었으면 캡처(와 소리)를 바꾼다. 방송할 창이 없으면
    /// 그대로 기다린다(비디오는 [`HOLD`]까지 마지막 화면을 둔다).
    pub fn tick(&mut self, video: &VideoPipeline) {
        let now = Instant::now();
        if now < self.next_check {
            return;
        }
        self.next_check = now + CHECK_INTERVAL;
        let list = match dongsi_media::list_sources() {
            Ok(l) => l,
            Err(e) => {
                warn!("게임 창 따라가기: 창 목록을 읽지 못했습니다: {e}");
                return;
            }
        };
        let target = match self.kind {
            FollowKind::Lol => lol_target(&list),
        };
        let Some(target) = target else {
            return;
        };
        let current = lock_source(&self.current).clone();
        if target.id == current.id && !video.is_waiting_source() {
            if target.name != current.name
                || target.width != current.width
                || target.height != current.height
            {
                *lock_source(&self.current) = target.clone();
            }
            return;
        }
        match video.switch_source(target.id.clone()) {
            Ok(()) => {
                info!(from = %current.name, to = %target.name, "게임 창 따라가기: 캡처 대상을 바꿨습니다");
                if let (Some(audio), Some(pid)) = (&self.audio, target.process_id) {
                    audio.set_source(AudioSource::ProcessTree(pid));
                }
                *lock_source(&self.current) = target.clone();
            }
            Err(e) => {
                warn!(to = %target.name, "게임 창 따라가기: 캡처 대상을 바꾸지 못했습니다: {e}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use dongsi_media::SourceId;

    use super::*;

    fn win(h: isize, exe: &str, w: u32, ht: u32) -> SourceInfo {
        SourceInfo {
            id: SourceId::Window(h),
            kind: SourceKind::Window,
            name: exe.to_owned(),
            width: w,
            height: ht,
            primary: false,
            process_id: Some(h as u32),
            process_name: Some(exe.to_owned()),
        }
    }

    #[test]
    fn follow_kind_is_lol_for_client_and_game_windows_only() {
        assert_eq!(
            follow_kind(&win(1, "LeagueClientUx.exe", 1600, 900)),
            Some(FollowKind::Lol)
        );
        assert_eq!(
            follow_kind(&win(2, "leagueclient.exe", 1600, 900)),
            Some(FollowKind::Lol)
        );
        assert_eq!(
            follow_kind(&win(3, "League of Legends.exe", 1920, 1080)),
            Some(FollowKind::Lol)
        );
        assert_eq!(follow_kind(&win(4, "Riot Client.exe", 1200, 700)), None);
        assert_eq!(follow_kind(&win(5, "chrome.exe", 1920, 1040)), None);
        let monitor = SourceInfo {
            id: SourceId::Monitor(1),
            kind: SourceKind::Monitor,
            process_name: Some("League of Legends.exe".into()),
            ..win(6, "x", 1, 1)
        };
        assert_eq!(follow_kind(&monitor), None);
    }

    #[test]
    fn lol_target_prefers_the_game_then_the_biggest_client_window() {
        let client = win(10, "LeagueClientUx.exe", 1600, 900);
        let small_client = win(11, "LeagueClientUx.exe", 400, 300);
        let game = win(20, "League of Legends.exe", 2560, 1440);
        let other = win(30, "Discord.exe", 1920, 1080);
        let lobby = [small_client.clone(), other.clone(), client.clone()];
        assert_eq!(lol_target(&lobby).map(|s| &s.id), Some(&client.id));
        let in_game = [client.clone(), game.clone(), other.clone()];
        assert_eq!(lol_target(&in_game).map(|s| &s.id), Some(&game.id));
        // 게임 중 클라이언트를 닫아 둔 경우
        assert_eq!(
            lol_target(std::slice::from_ref(&game)).map(|s| &s.id),
            Some(&game.id)
        );
        // 게임이 끝나고 클라이언트가 다시 뜨기 전
        assert!(lol_target(&[other]).is_none());
    }
}
