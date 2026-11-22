//! 전용 서버 설정: 명령줄 인자, 환경 변수, 설정 파일(`dongsi-server.toml`)을 합친다.
//!
//! 우선순위는 **명령줄 > 환경 변수 > 설정 파일 > 기본값**이다. 파싱은 모두 순수 함수라서
//! 인자 목록이나 환경 변수 조회 함수를 받아 시험한다.

use std::path::{Path, PathBuf};

use dongsi_core::DEFAULT_PORT;
use dongsi_server::ServerConfig;
use serde::Deserialize;

pub const CONFIG_FILE: &str = "dongsi-server.toml";

pub const DEFAULT_CONFIG: &str = r#"# dongsi 전용 서버 설정
#
# 우선순위: 명령줄 > 환경 변수(DONGSI_*) > 이 파일 > 기본값.
# 환경 변수와 명령줄 인자 목록은 dongsi-server --help 로 본다.

# 목록과 로컬 감지에 보이는 서버 이름
name = "dongsi 서버"

# 서버가 실제로 듣는 TCP(시그널링)·UDP(미디어, 로컬 감지) 포트. 둘 다 열어야 한다.
port = 7884

# 밖에서 보이는 포트. 컨테이너나 공유기에서 다른 번호로 매핑할 때만 쓴다
# (예: 호스트 9000 -> 컨테이너 7884이면 public_port = 9000).
# 정하지 않으면 port와 같다. 포트를 붙이지 않은 addresses에도 이 값을 쓴다.
# public_port = 7884

# 초대 코드에 넣을 주소(도메인이나 공인 IP). 포트를 붙이지 않으면 공개 포트(public_port, 없으면 port)를 쓴다.
addresses = []

# true이면 이 기계의 LAN·Tailscale 인터페이스 주소를 초대 코드에 더 넣는다.
# 브리지 네트워크의 컨테이너처럼 인터페이스 주소가 밖에서 닿지 않으면 false로 하고 addresses만 쓴다.
# 둘 다 비면 127.0.0.1을 넣는다(같은 PC에서만 닿는다).
auto_addresses = true

# 동시에 할 수 있는 방송 수, 방송 하나당 시청자 수
max_broadcasts = 16
max_viewers_per_broadcast = 16

# 같은 LAN·tailnet의 감지 질의에 응답할지
discovery = true
"#;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileConfig {
    pub name: String,
    pub port: u16,
    pub public_port: Option<u16>,
    pub addresses: Vec<String>,
    pub auto_addresses: bool,
    pub max_broadcasts: usize,
    pub max_viewers_per_broadcast: usize,
    pub discovery: bool,
}

impl Default for FileConfig {
    fn default() -> Self {
        let d = ServerConfig::default();
        Self {
            name: d.name,
            port: DEFAULT_PORT,
            public_port: None,
            addresses: Vec::new(),
            auto_addresses: true,
            max_broadcasts: d.max_broadcasts,
            max_viewers_per_broadcast: d.max_viewers_per_broadcast,
            discovery: d.discovery,
        }
    }
}

/// 설정 파일을 읽는다. 없으면 기본 파일을 만들고(`create`가 true일 때) 기본값을 돌려준다.
pub fn load_file(dir: &Path, create: bool) -> Result<FileConfig, String> {
    let path = dir.join(CONFIG_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if create {
                std::fs::write(&path, DEFAULT_CONFIG)
                    .map_err(|e| format!("{}을 만들지 못했습니다: {e}", path.display()))?;
                tracing::info!(path = %path.display(), "기본 설정 파일을 만들었습니다");
            }
            Ok(FileConfig::default())
        }
        Err(e) => Err(format!("{}을 읽지 못했습니다: {e}", path.display())),
    }
}

/// 명령줄 인자.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Cli {
    pub help: bool,
    /// 서버가 TCP 접속을 받는지만 확인하고 끝난다(컨테이너 HEALTHCHECK용).
    pub healthcheck: bool,
    pub data: Option<PathBuf>,
    pub port: Option<u16>,
    pub public_port: Option<u16>,
    pub name: Option<String>,
    pub addresses: Vec<String>,
    pub no_auto_addresses: bool,
}

pub fn usage() -> &'static str {
    "사용법: dongsi-server [옵션]\n\
     \n\
     옵션(명령줄이 환경 변수와 설정 파일보다 우선):\n\
     \x20 --data <폴더>         인증서와 설정을 둘 폴더 [DONGSI_DATA, 기본 ./dongsi-data]\n\
     \x20 --port <번호>         서버가 듣는 TCP·UDP 포트 [DONGSI_PORT, 기본 7884]\n\
     \x20 --public-port <번호>  밖에서 보이는 포트. 다른 번호로 매핑할 때만 [DONGSI_PUBLIC_PORT, 기본 --port와 같음]\n\
     \x20 --name <이름>         서버 이름 [DONGSI_NAME]\n\
     \x20 --address <주소>      초대 코드에 넣을 주소(도메인이나 공인 IP). 여러 번 쓸 수 있고, 포트를\n\
     \x20                       붙이지 않으면 공개 포트를 쓴다 [DONGSI_ADDRESSES: 쉼표·공백으로 구분]\n\
     \x20 --no-auto-addresses   이 기계의 인터페이스 주소를 초대 코드에 넣지 않는다 [DONGSI_AUTO_ADDRESSES=false]\n\
     \x20 --healthcheck         서버가 TCP 접속을 받는지만 확인하고 끝난다(0: 정상, 1: 응답 없음)\n\
     \x20 -h, --help            이 도움말\n\
     \n\
     환경 변수만 있는 설정: DONGSI_MAX_BROADCASTS, DONGSI_MAX_VIEWERS, DONGSI_DISCOVERY(true/false).\n\
     설정 파일(<데이터 폴더>/dongsi-server.toml)은 처음 실행할 때 만들어진다. 로그 수준은 RUST_LOG(기본 info)."
}

/// 명령줄 인자(프로그램 이름 제외)를 읽는다. `--flag=값`도 받는다.
pub fn parse_cli<I: IntoIterator<Item = String>>(args: I) -> Result<Cli, String> {
    let mut cli = Cli::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut inline = inline;
        let mut value = |flag: &str| match inline.take() {
            Some(v) => Ok(v),
            None => it
                .next()
                .ok_or_else(|| format!("{flag} 다음에 값이 필요합니다")),
        };
        match flag.as_str() {
            "-h" | "--help" => cli.help = true,
            "--healthcheck" => cli.healthcheck = true,
            "--no-auto-addresses" => cli.no_auto_addresses = true,
            "--data" => cli.data = Some(PathBuf::from(value("--data")?)),
            "--port" => cli.port = Some(parse_port("--port", &value("--port")?, true)?),
            "--public-port" => {
                cli.public_port = Some(parse_port(
                    "--public-port",
                    &value("--public-port")?,
                    false,
                )?);
            }
            "--name" => cli.name = Some(value("--name")?),
            "--address" => cli.addresses.push(value("--address")?),
            other => return Err(format!("알 수 없는 인자: {other}")),
        }
        if inline.is_some() {
            return Err(format!("{flag}에는 값을 줄 수 없습니다"));
        }
    }
    Ok(cli)
}

fn parse_port(what: &str, v: &str, allow_zero: bool) -> Result<u16, String> {
    match v.trim().parse::<u16>() {
        Ok(0) if !allow_zero => Err(format!("{what} 값이 올바르지 않습니다: {v} (1~65535)")),
        Ok(p) => Ok(p),
        Err(_) => Err(format!(
            "{what} 값이 올바르지 않습니다: {v} ({}~65535의 숫자)",
            u8::from(!allow_zero)
        )),
    }
}

/// 환경 변수(`DONGSI_*`)로 준 설정. 비어 있는 값은 정하지 않은 것으로 본다.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct EnvConfig {
    pub data: Option<PathBuf>,
    pub port: Option<u16>,
    pub public_port: Option<u16>,
    pub name: Option<String>,
    pub addresses: Vec<String>,
    pub auto_addresses: Option<bool>,
    pub max_broadcasts: Option<usize>,
    pub max_viewers: Option<usize>,
    pub discovery: Option<bool>,
}

/// `get`은 환경 변수 조회 함수다(시험에서는 맵을 쓴다).
pub fn parse_env(get: impl Fn(&str) -> Option<String>) -> Result<EnvConfig, String> {
    let get = |key: &str| get(key).filter(|v| !v.trim().is_empty());
    let mut env = EnvConfig::default();
    if let Some(v) = get("DONGSI_DATA") {
        env.data = Some(PathBuf::from(v));
    }
    if let Some(v) = get("DONGSI_PORT") {
        env.port = Some(parse_port("환경 변수 DONGSI_PORT", &v, true)?);
    }
    if let Some(v) = get("DONGSI_PUBLIC_PORT") {
        env.public_port = Some(parse_port("환경 변수 DONGSI_PUBLIC_PORT", &v, false)?);
    }
    if let Some(v) = get("DONGSI_NAME") {
        env.name = Some(v);
    }
    if let Some(v) = get("DONGSI_ADDRESSES") {
        env.addresses = v
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
    }
    if let Some(v) = get("DONGSI_AUTO_ADDRESSES") {
        env.auto_addresses = Some(parse_bool("DONGSI_AUTO_ADDRESSES", &v)?);
    }
    if let Some(v) = get("DONGSI_MAX_BROADCASTS") {
        env.max_broadcasts = Some(parse_count("DONGSI_MAX_BROADCASTS", &v)?);
    }
    if let Some(v) = get("DONGSI_MAX_VIEWERS") {
        env.max_viewers = Some(parse_count("DONGSI_MAX_VIEWERS", &v)?);
    }
    if let Some(v) = get("DONGSI_DISCOVERY") {
        env.discovery = Some(parse_bool("DONGSI_DISCOVERY", &v)?);
    }
    Ok(env)
}

fn parse_bool(key: &str, v: &str) -> Result<bool, String> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(format!(
            "환경 변수 {key} 값이 올바르지 않습니다: {v} (true, false, 1, 0 중 하나)"
        )),
    }
}

fn parse_count(key: &str, v: &str) -> Result<usize, String> {
    match v.trim().parse::<usize>() {
        Ok(n) if n >= 1 => Ok(n),
        _ => Err(format!(
            "환경 변수 {key} 값이 올바르지 않습니다: {v} (1 이상의 숫자)"
        )),
    }
}

/// 컨테이너 안에서 도는지(`/.dockerenv`가 있거나 `DONGSI_IN_CONTAINER=1`).
pub fn in_container(get: impl Fn(&str) -> Option<String>, dockerenv_exists: bool) -> bool {
    dockerenv_exists
        || get("DONGSI_IN_CONTAINER")
            .is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true"))
}

/// 데이터 폴더. 설정 파일을 읽기 전에 정해야 하므로 따로 푼다.
pub fn data_dir(cli: &Cli, env: &EnvConfig) -> PathBuf {
    cli.data
        .clone()
        .or_else(|| env.data.clone())
        .unwrap_or_else(|| PathBuf::from("dongsi-data"))
}

/// 우선순위를 적용한 최종 설정.
#[derive(Debug, PartialEq, Eq)]
pub struct Settings {
    pub name: String,
    pub port: u16,
    /// 밖에서 보이는 포트(정한 경우만).
    pub public_port: Option<u16>,
    /// 설정한 주소: 명령줄, 환경 변수, 설정 파일 순서.
    pub addresses: Vec<String>,
    pub auto_addresses: bool,
    pub max_broadcasts: usize,
    pub max_viewers_per_broadcast: usize,
    pub discovery: bool,
}

impl Settings {
    pub fn resolve(cli: &Cli, env: &EnvConfig, file: FileConfig) -> Result<Self, String> {
        let name = cli
            .name
            .clone()
            .or_else(|| env.name.clone())
            .unwrap_or(file.name);
        let name = dongsi_core::proto::sanitize_display_name(&name)
            .ok_or_else(|| "서버 이름이 비어 있습니다".to_string())?;
        let mut addresses = cli.addresses.clone();
        addresses.extend(env.addresses.iter().cloned());
        addresses.extend(file.addresses);
        let public_port = cli.public_port.or(env.public_port).or(file.public_port);
        if file.public_port == Some(0) {
            return Err("public_port 값이 올바르지 않습니다: 0 (1~65535)".into());
        }
        Ok(Self {
            name,
            port: cli.port.or(env.port).unwrap_or(file.port),
            public_port,
            addresses,
            auto_addresses: if cli.no_auto_addresses {
                false
            } else {
                env.auto_addresses.unwrap_or(file.auto_addresses)
            },
            max_broadcasts: env.max_broadcasts.unwrap_or(file.max_broadcasts).max(1),
            max_viewers_per_broadcast: env
                .max_viewers
                .unwrap_or(file.max_viewers_per_broadcast)
                .max(1),
            discovery: env.discovery.unwrap_or(file.discovery),
        })
    }

    /// 공개 포트가 실제 포트와 다를 때만 `ServerConfig.external_port`로 넘긴다.
    pub fn external_port(&self) -> Option<u16> {
        self.public_port.filter(|p| *p != self.port)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    fn env_of(pairs: &[(&str, &str)]) -> Result<EnvConfig, String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        parse_env(|k| map.get(k).cloned())
    }

    #[test]
    fn default_config_parses() {
        let c: FileConfig = toml::from_str(DEFAULT_CONFIG).unwrap();
        assert_eq!(c.port, DEFAULT_PORT);
        assert_eq!(c.public_port, None);
        assert!(c.addresses.is_empty());
        assert!(c.auto_addresses);
        assert!(c.discovery);
    }

    #[test]
    fn partial_config_uses_defaults() {
        let c: FileConfig = toml::from_str(
            "name = \"a\"\nport = 9000\naddresses = [\"x.example\"]\nmax_broadcasts = 3\nmax_viewers_per_broadcast = 4\ndiscovery = false\n",
        )
        .unwrap();
        assert_eq!(c.port, 9000);
        assert!(c.auto_addresses);
        assert_eq!(c.public_port, None);
        assert!(toml::from_str::<FileConfig>("nope = 1").is_err());
    }

    #[test]
    fn cli_parses_all_flags() {
        let cli = parse_cli(args(&[
            "--data",
            "/d",
            "--port",
            "9000",
            "--public-port=9001",
            "--name",
            "서버",
            "--address",
            "a.example",
            "--address",
            "b.example:1",
            "--no-auto-addresses",
        ]))
        .unwrap();
        assert_eq!(cli.data, Some(PathBuf::from("/d")));
        assert_eq!(cli.port, Some(9000));
        assert_eq!(cli.public_port, Some(9001));
        assert_eq!(cli.name.as_deref(), Some("서버"));
        assert_eq!(cli.addresses, ["a.example", "b.example:1"]);
        assert!(cli.no_auto_addresses);
        assert!(parse_cli(args(&["--help"])).unwrap().help);
        assert!(parse_cli(args(&["--healthcheck"])).unwrap().healthcheck);
    }

    #[test]
    fn cli_rejects_bad_input() {
        assert!(
            parse_cli(args(&["--port"]))
                .unwrap_err()
                .contains("값이 필요")
        );
        assert!(parse_cli(args(&["--port", "abc"])).is_err());
        assert!(parse_cli(args(&["--port", "70000"])).is_err());
        assert!(parse_cli(args(&["--public-port", "0"])).is_err());
        assert!(
            parse_cli(args(&["--bogus"]))
                .unwrap_err()
                .contains("--bogus")
        );
        assert!(parse_cli(args(&["--no-auto-addresses=1"])).is_err());
    }

    #[test]
    fn env_parses_values() {
        let env = env_of(&[
            ("DONGSI_DATA", "/data"),
            ("DONGSI_PORT", "8350"),
            ("DONGSI_PUBLIC_PORT", "9000"),
            ("DONGSI_NAME", "내 서버"),
            (
                "DONGSI_ADDRESSES",
                "a.example, 203.0.113.7:9000  b.example,,",
            ),
            ("DONGSI_AUTO_ADDRESSES", "FALSE"),
            ("DONGSI_MAX_BROADCASTS", "4"),
            ("DONGSI_MAX_VIEWERS", "8"),
            ("DONGSI_DISCOVERY", "0"),
        ])
        .unwrap();
        assert_eq!(env.data, Some(PathBuf::from("/data")));
        assert_eq!(env.port, Some(8350));
        assert_eq!(env.public_port, Some(9000));
        assert_eq!(env.name.as_deref(), Some("내 서버"));
        assert_eq!(
            env.addresses,
            ["a.example", "203.0.113.7:9000", "b.example"]
        );
        assert_eq!(env.auto_addresses, Some(false));
        assert_eq!(env.max_broadcasts, Some(4));
        assert_eq!(env.max_viewers, Some(8));
        assert_eq!(env.discovery, Some(false));
    }

    #[test]
    fn env_empty_values_are_unset() {
        let env = env_of(&[
            ("DONGSI_PORT", ""),
            ("DONGSI_ADDRESSES", "  "),
            ("DONGSI_NAME", ""),
        ])
        .unwrap();
        assert_eq!(env, EnvConfig::default());
    }

    #[test]
    fn env_rejects_bad_values() {
        for (k, v) in [
            ("DONGSI_PORT", "abc"),
            ("DONGSI_PORT", "65536"),
            ("DONGSI_PUBLIC_PORT", "0"),
            ("DONGSI_AUTO_ADDRESSES", "maybe"),
            ("DONGSI_DISCOVERY", "yes"),
            ("DONGSI_MAX_BROADCASTS", "0"),
            ("DONGSI_MAX_VIEWERS", "-1"),
        ] {
            let err = env_of(&[(k, v)]).unwrap_err();
            assert!(err.contains(k), "{err}");
        }
    }

    #[test]
    fn precedence_cli_env_file_default() {
        let file = FileConfig {
            name: "파일".into(),
            port: 7000,
            public_port: Some(7001),
            addresses: vec!["file.example".into()],
            auto_addresses: true,
            discovery: true,
            ..Default::default()
        };
        // 기본값 + 파일만
        let s = Settings::resolve(&Cli::default(), &EnvConfig::default(), file.clone()).unwrap();
        assert_eq!(
            (s.name.as_str(), s.port, s.public_port),
            ("파일", 7000, Some(7001))
        );
        assert_eq!(s.addresses, ["file.example"]);
        assert!(s.auto_addresses && s.discovery);
        // 환경 변수가 파일을 이긴다
        let env = env_of(&[
            ("DONGSI_NAME", "환경"),
            ("DONGSI_PORT", "8000"),
            ("DONGSI_PUBLIC_PORT", "8001"),
            ("DONGSI_ADDRESSES", "env.example"),
            ("DONGSI_AUTO_ADDRESSES", "0"),
            ("DONGSI_DISCOVERY", "false"),
            ("DONGSI_MAX_BROADCASTS", "2"),
        ])
        .unwrap();
        let s = Settings::resolve(&Cli::default(), &env, file.clone()).unwrap();
        assert_eq!(
            (s.name.as_str(), s.port, s.public_port),
            ("환경", 8000, Some(8001))
        );
        assert_eq!(s.addresses, ["env.example", "file.example"]);
        assert!(!s.auto_addresses && !s.discovery);
        assert_eq!(s.max_broadcasts, 2);
        // 명령줄이 환경 변수를 이긴다
        let cli = parse_cli(args(&[
            "--name",
            "명령줄",
            "--port",
            "9000",
            "--public-port",
            "9001",
            "--address",
            "cli.example",
        ]))
        .unwrap();
        let s = Settings::resolve(&cli, &env, file.clone()).unwrap();
        assert_eq!(
            (s.name.as_str(), s.port, s.public_port),
            ("명령줄", 9000, Some(9001))
        );
        assert_eq!(s.addresses, ["cli.example", "env.example", "file.example"]);
        // --no-auto-addresses는 환경 변수가 true여도 끈다
        let env = env_of(&[("DONGSI_AUTO_ADDRESSES", "true")]).unwrap();
        let cli = parse_cli(args(&["--no-auto-addresses"])).unwrap();
        assert!(
            !Settings::resolve(&cli, &env, FileConfig::default())
                .unwrap()
                .auto_addresses
        );
    }

    #[test]
    fn external_port_only_when_different() {
        let mut s = Settings::resolve(
            &Cli::default(),
            &EnvConfig::default(),
            FileConfig::default(),
        )
        .unwrap();
        assert_eq!(s.external_port(), None);
        s.public_port = Some(DEFAULT_PORT);
        assert_eq!(s.external_port(), None);
        s.public_port = Some(9000);
        assert_eq!(s.external_port(), Some(9000));
    }

    #[test]
    fn empty_name_is_error() {
        let env = env_of(&[("DONGSI_NAME", "\u{7}\u{7}")]).unwrap();
        let err = Settings::resolve(&Cli::default(), &env, FileConfig::default()).unwrap_err();
        assert!(err.contains("이름"));
    }

    #[test]
    fn container_detection() {
        let none = |_: &str| None;
        assert!(!in_container(none, false));
        assert!(in_container(none, true));
        assert!(in_container(
            |k| (k == "DONGSI_IN_CONTAINER").then(|| "1".into()),
            false
        ));
        assert!(!in_container(
            |k| (k == "DONGSI_IN_CONTAINER").then(|| "0".into()),
            false
        ));
    }

    #[test]
    fn data_dir_precedence() {
        let env = env_of(&[("DONGSI_DATA", "/env")]).unwrap();
        assert_eq!(
            data_dir(&Cli::default(), &EnvConfig::default()),
            PathBuf::from("dongsi-data")
        );
        assert_eq!(data_dir(&Cli::default(), &env), PathBuf::from("/env"));
        let cli = parse_cli(args(&["--data", "/cli"])).unwrap();
        assert_eq!(data_dir(&cli, &env), PathBuf::from("/cli"));
    }
}
