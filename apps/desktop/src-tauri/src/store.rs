//! 앱 상태 파일(`<앱 데이터 폴더>/state.json`).
//!
//! - 처음 실행하면 설치 ID를 만들어 바로 저장한다.
//! - 파일을 읽을 수 없으면 `state.json.broken-<시각>`으로 옮겨 두고 새로 시작한다.
//! - 저장은 임시 파일에 쓴 뒤 이름을 바꿔서, 중간에 꺼져도 파일이 깨지지 않게 한다.
//!
//! 기억한 방송 비밀번호도 이 파일에 평문으로 들어간다. 사용자 프로필 폴더라 다른 계정은
//! 읽을 수 없지만, 나중에 DPAPI 등으로 암호화할 대상이다.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use dongsi_core::{InstallId, ServerList};
use serde::{Deserialize, Serialize};

use crate::error::{CmdError, CmdResult};

const FILE_NAME: &str = "state.json";
const STATE_VERSION: u32 = 1;
pub const MAX_DISPLAY_NAME_CHARS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub version: u32,
    pub install_id: InstallId,
    /// 방송 목록에 보이는 이름.
    pub display_name: String,
    #[serde(default)]
    pub servers: ServerList,
    /// 방송 소리 증폭(%, 100 = 그대로). 방송 중 화면에서 마지막으로 정한 값을 다음 방송에도 쓴다.
    #[serde(default = "default_audio_boost")]
    pub audio_boost: u32,
}

fn default_audio_boost() -> u32 {
    100
}

impl State {
    fn fresh() -> Self {
        let install_id = InstallId::generate();
        let tag: String = install_id.to_string().chars().take(4).collect();
        Self {
            version: STATE_VERSION,
            install_id,
            display_name: format!("사용자-{tag}"),
            servers: ServerList::new(),
            audio_boost: default_audio_boost(),
        }
    }

    pub fn set_display_name(&mut self, name: &str) -> CmdResult<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(CmdError::new("이름을 입력해 주세요"));
        }
        if name.chars().count() > MAX_DISPLAY_NAME_CHARS {
            return Err(CmdError::new(format!(
                "이름이 너무 깁니다(최대 {MAX_DISPLAY_NAME_CHARS}자)"
            )));
        }
        self.display_name = name.to_owned();
        Ok(())
    }
}

pub struct Store {
    path: PathBuf,
    state: State,
}

impl Store {
    pub fn open(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(FILE_NAME);
        let state = match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<State>(&bytes) {
                Ok(state) => state,
                Err(err) => {
                    let backup = path.with_extension(format!("json.broken-{}", unix_time()));
                    eprintln!(
                        "상태 파일을 읽을 수 없어 {}(으)로 옮기고 새로 시작합니다: {err}",
                        backup.display()
                    );
                    fs::rename(&path, &backup)?;
                    State::fresh()
                }
            },
            Err(err) if err.kind() == io::ErrorKind::NotFound => State::fresh(),
            Err(err) => return Err(err),
        };
        let store = Self { path, state };
        store.save()?;
        Ok(store)
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// 상태를 바꾸고 저장한다. `f`가 실패하거나 저장에 실패하면 바꾸기 전 상태로 되돌린다.
    pub fn update<T>(&mut self, f: impl FnOnce(&mut State) -> CmdResult<T>) -> CmdResult<T> {
        let before = self.state.clone();
        let result = f(&mut self.state);
        if result.is_err() {
            self.state = before;
            return result;
        }
        if let Err(err) = self.save() {
            self.state = before;
            return Err(CmdError::new(format!("설정을 저장하지 못했습니다: {err}")));
        }
        result
    }

    fn save(&self) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(&self.state)?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &self.path)
    }
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("dongsi-store-test-{}", InstallId::generate()));
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn creates_state_and_keeps_install_id_across_restarts() {
        let dir = TempDir::new();
        let first = Store::open(&dir.0).unwrap();
        let id = first.state().install_id;
        assert!(first.state().display_name.starts_with("사용자-"));
        drop(first);

        let second = Store::open(&dir.0).unwrap();
        assert_eq!(second.state().install_id, id);
    }

    #[test]
    fn update_persists_and_rolls_back_on_error() {
        let dir = TempDir::new();
        let mut store = Store::open(&dir.0).unwrap();
        store.update(|s| s.set_display_name("  하늘  ")).unwrap();
        assert!(store.update(|s| s.set_display_name("   ")).is_err());
        assert_eq!(store.state().display_name, "하늘");

        let reopened = Store::open(&dir.0).unwrap();
        assert_eq!(reopened.state().display_name, "하늘");
    }

    #[test]
    fn broken_file_is_backed_up_and_replaced() {
        let dir = TempDir::new();
        fs::create_dir_all(&dir.0).unwrap();
        fs::write(dir.0.join(FILE_NAME), b"{ not json").unwrap();

        let store = Store::open(&dir.0).unwrap();
        assert_eq!(store.state().version, STATE_VERSION);
        let backups = fs::read_dir(&dir.0)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains("broken"))
            .count();
        assert_eq!(backups, 1);
    }

    #[test]
    fn display_name_limits() {
        let mut state = State::fresh();
        let long = "가".repeat(MAX_DISPLAY_NAME_CHARS + 1);
        assert!(state.set_display_name(&long).is_err());
        state
            .set_display_name(&"가".repeat(MAX_DISPLAY_NAME_CHARS))
            .unwrap();
    }
}
