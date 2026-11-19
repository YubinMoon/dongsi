//! 앱이 쓰는 폴더와 시험용 환경 변수(`DONGSI_PROFILE`, `DONGSI_PORT`).
//!
//! - `DONGSI_PROFILE=<이름>`: 같은 PC에서 앱 두 개를 따로 띄워 시험할 때 쓴다. 앱 데이터는
//!   `<앱 데이터>/profiles/<이름>/`, WebView2 데이터는 `<앱 로컬 데이터>/profiles/<이름>/EBWebView`로 나뉜다.
//!   없거나 비어 있으면 지금까지와 똑같다.
//! - `DONGSI_PORT=<포트>`: 내장 서버 포트와 로컬 감지 질의 포트를 바꾼다(1024~65535). 없으면 7884.
//!
//! 규칙에 안 맞는 값이면 폴더를 만들기 전에 종료한다(실제 사용자 데이터를 건드리지 않게).
//! 앱 데이터 경로는 여기서만 정한다(`app_data_dir()`를 다른 곳에서 부르지 않는다).

use std::path::{Path, PathBuf};

use dongsi_core::DEFAULT_PORT;

pub const PROFILE_ENV: &str = "DONGSI_PROFILE";
pub const PORT_ENV: &str = "DONGSI_PORT";
pub const MAX_PROFILE_CHARS: usize = 64;
pub const MIN_PORT: u16 = 1024;

/// 환경 변수에서 읽은 실행 설정.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppEnv {
    pub profile: Option<String>,
    /// 내장 서버와 로컬 감지가 쓰는 포트.
    pub port: u16,
}

impl Default for AppEnv {
    fn default() -> Self {
        Self {
            profile: None,
            port: DEFAULT_PORT,
        }
    }
}

impl AppEnv {
    /// 환경 변수를 읽는다. 값이 규칙에 안 맞으면 사용자에게 보여 줄 오류 문장을 돌려준다.
    pub fn from_env() -> Result<Self, String> {
        Self::parse(
            std::env::var(PROFILE_ENV).ok().as_deref(),
            std::env::var(PORT_ENV).ok().as_deref(),
        )
    }

    pub fn parse(profile: Option<&str>, port: Option<&str>) -> Result<Self, String> {
        Ok(Self {
            profile: parse_profile(profile)?,
            port: parse_port(port)?,
        })
    }
}

/// 프로필 이름 검사. 영숫자·`-`·`_`만, 1~64자. `None`이나 빈 문자열은 프로필 없음.
pub fn parse_profile(value: Option<&str>) -> Result<Option<String>, String> {
    let Some(name) = value.filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let ok = name.chars().count() <= MAX_PROFILE_CHARS
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(Some(name.to_owned()))
    } else {
        Err(format!(
            "{PROFILE_ENV} 값이 올바르지 않습니다: 영문자·숫자·'-'·'_'만 쓸 수 있고 1~{MAX_PROFILE_CHARS}자여야 합니다."
        ))
    }
}

/// 포트 검사. 1024~65535. `None`이나 빈 문자열은 기본 포트.
pub fn parse_port(value: Option<&str>) -> Result<u16, String> {
    let Some(text) = value.filter(|v| !v.is_empty()) else {
        return Ok(DEFAULT_PORT);
    };
    match text.parse::<u16>() {
        Ok(p) if p >= MIN_PORT => Ok(p),
        _ => Err(format!(
            "{PORT_ENV} 값이 올바르지 않습니다: {MIN_PORT}~65535 사이의 숫자여야 합니다."
        )),
    }
}

/// 프로필이 있으면 `<base>/profiles/<이름>`, 없으면 `base` 그대로.
pub fn profile_dir(base: &Path, profile: Option<&str>) -> PathBuf {
    match profile {
        Some(name) => base.join("profiles").join(name),
        None => base.to_path_buf(),
    }
}

/// WebView2 사용자 데이터 폴더. 프로필이 없으면 `None`(Tauri 기본 위치를 쓴다).
pub fn webview_dir(local_base: &Path, profile: Option<&str>) -> Option<PathBuf> {
    profile.map(|name| profile_dir(local_base, Some(name)).join("EBWebView"))
}

/// 앱이 쓰는 폴더 모음(managed state).
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub profile: Option<String>,
    /// `state.json`이 들어 있는 폴더.
    pub data_dir: PathBuf,
    /// 내장 서버 인증서 폴더.
    pub host_dir: PathBuf,
    /// WebView2 사용자 데이터 폴더(프로필이 있을 때만).
    pub webview_dir: Option<PathBuf>,
}

impl AppPaths {
    pub fn new(app_data: &Path, app_local_data: &Path, profile: Option<&str>) -> Self {
        let data_dir = profile_dir(app_data, profile);
        Self {
            profile: profile.map(str::to_owned),
            host_dir: data_dir.join("host"),
            data_dir,
            webview_dir: webview_dir(app_local_data, profile),
        }
    }

    /// 메인 창 제목.
    pub fn window_title(&self, base: &str) -> String {
        match &self.profile {
            Some(p) => format!("{base} [{p}]"),
            None => base.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_names() {
        assert_eq!(parse_profile(None), Ok(None));
        assert_eq!(parse_profile(Some("")), Ok(None));
        assert_eq!(parse_profile(Some("t2a")), Ok(Some("t2a".into())));
        assert_eq!(parse_profile(Some("A-b_9")), Ok(Some("A-b_9".into())));
        let longest = "a".repeat(64);
        assert_eq!(parse_profile(Some(&longest)), Ok(Some(longest.clone())));
        for bad in [
            "../x", "a/b", "a\\b", "a b", ".", "..", "한글", "a.b", "a:b", "con\0",
        ] {
            assert!(parse_profile(Some(bad)).is_err(), "{bad:?}");
        }
        assert!(parse_profile(Some(&"a".repeat(65))).is_err());
    }

    #[test]
    fn ports() {
        assert_eq!(parse_port(None), Ok(DEFAULT_PORT));
        assert_eq!(parse_port(Some("")), Ok(DEFAULT_PORT));
        assert_eq!(parse_port(Some("8250")), Ok(8250));
        assert_eq!(parse_port(Some("1024")), Ok(1024));
        assert_eq!(parse_port(Some("65535")), Ok(65535));
        for bad in ["1023", "0", "65536", "-1", "abc", " 8250", "8250 ", "80.5"] {
            assert!(parse_port(Some(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn env_parse_reports_the_first_bad_value() {
        assert_eq!(AppEnv::parse(None, None), Ok(AppEnv::default()));
        let env = AppEnv::parse(Some("t2a"), Some("8250")).unwrap();
        assert_eq!(env.profile.as_deref(), Some("t2a"));
        assert_eq!(env.port, 8250);
        assert!(AppEnv::parse(Some("../x"), None).is_err());
        assert!(AppEnv::parse(None, Some("1")).is_err());
    }

    #[test]
    fn paths_without_profile_stay_as_before() {
        let data = Path::new("C:/data/app");
        let local = Path::new("C:/local/app");
        let p = AppPaths::new(data, local, None);
        assert_eq!(p.data_dir, data);
        assert_eq!(p.host_dir, data.join("host"));
        assert_eq!(p.webview_dir, None);
        assert_eq!(p.window_title("dongsi"), "dongsi");
    }

    #[test]
    fn paths_with_profile_move_under_profiles() {
        let data = Path::new("C:/data/app");
        let local = Path::new("C:/local/app");
        let p = AppPaths::new(data, local, Some("t2a"));
        assert_eq!(p.data_dir, data.join("profiles").join("t2a"));
        assert_eq!(p.host_dir, data.join("profiles").join("t2a").join("host"));
        assert_eq!(
            p.webview_dir,
            Some(local.join("profiles").join("t2a").join("EBWebView"))
        );
        assert_eq!(p.window_title("dongsi"), "dongsi [t2a]");
    }
}
