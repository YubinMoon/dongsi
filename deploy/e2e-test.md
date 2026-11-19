# 리눅스 서버 통신 시험 (클라이언트 ↔ 서버 E2E)

전용 서버(`dongsi-server`)를 **리눅스(WSL의 네이티브 프로세스, Docker 브리지·host 네트워크)**에서 띄우고, Windows와 WSL의 시험 클라이언트가 실제로 접속해 방송·시청하는 것까지 확인하는 시험이다. 서버를 Docker로 배포하는 방법은 [README.md](README.md)에 있다.

## 무엇을 증명하나

시험 본체는 [`crates/server/tests/e2e/main.rs`](../crates/server/tests/e2e/main.rs)의 시나리오 12개다(머리의 문서에 환경 변수 표가 있다). 시험 클라이언트는 앱과 같은 코드(`dongsi-net`의 시그널링·송출, str0m 시청)를 쓰므로, 통과하면 **이 서버에 앱이 접속해서 방송하고 시청할 수 있다**는 뜻이다.

| 시나리오 | 확인하는 것 |
|---|---|
| `signaling_handshake` | TLS 지문 확인(고정·불일치·TOFU), 프로토콜 버전, 잘못된 첫 메시지·큰 SDP 거절 |
| `publish_and_watch` | 방송 → 서버(SFU) → 시청의 미디어 흐름, 영상·오디오 A/V 차이 5ms 미만, 코덱 변경, 중복 방송 거절 |
| `multiple_broadcasters` | 방송 3개 동시, 시청자는 자기 방송의 프레임만 받음 |
| `many_viewers` | 시청자 10명, 시청자 수 집계, 나가고 끊기는 경로 |
| `passwords` | 방송 비밀번호(없음·틀림·맞음) |
| `rate_limit` (파괴적) | 비밀번호 실패 출발지 IP당 1분 8번 제한 |
| `broadcast_end_paths` | 방송이 끝나는 세 경로(Unpublish, 시그널링 닫음, 송출 WebRTC만 닫음)와 재방송 |
| `reconnect` | 같은 ID로 재접속 |
| `server_restart` (파괴적) | 서버를 정상 종료·재시작해도 지문이 같고, 다시 방송·시청 가능 |
| `discovery` | UDP 로컬 감지 질의(유니캐스트) 응답 |
| `limits` | 내부 서버 전용(외부 서버에서는 "건너뜀"으로 통과) |
| `update_publish` | 방송 정보 갱신과 권한 |

모드(서버를 띄우는 방식)와 클라이언트(시험을 돌리는 쪽)의 곱마다 이 시험을 돌려서, **같은 서버 이미지가 네트워크 방식마다 맞게 동작하는지**(특히 컨테이너가 알리는 미디어 주소·포트)를 확인한다.

## 준비물

Windows 11 + WSL2(Ubuntu 24.04에서 확인). 다음이 모두 있어야 한다. 스크립트가 처음에 확인하고, 없으면 무엇을 설치할지 알리고 끝낸다.

### 1. WSL: mirrored 네트워킹

Windows와 WSL이 같은 IP·포트를 쓰게 한다. `%UserProfile%\.wslconfig`:

```ini
[wsl2]
networkingMode=mirrored

[experimental]
hostAddressLoopback=true
```

적용: `wsl --shutdown` 뒤 WSL을 다시 연다. (Windows와 WSL이 서로 `127.0.0.1`로 접속된다.)

### 2. WSL: Docker Engine

Docker Desktop이 아니라 WSL 안의 Docker Engine이다(공식 apt 저장소, Ubuntu 기준).

```sh
sudo apt-get update && sudo apt-get install -y ca-certificates curl git
sudo install -m 0755 -d /etc/apt/keyrings
sudo curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc
echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/ubuntu $(. /etc/os-release && echo $VERSION_CODENAME) stable" \
  | sudo tee /etc/apt/sources.list.d/docker.list
sudo apt-get update
sudo apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
sudo usermod -aG docker $USER      # 적용하려면 WSL을 다시 연다(wsl --shutdown)
docker info                         # sudo 없이 되어야 한다
```

### 3. WSL: rustup과 빌드 도구

```sh
sudo apt-get install -y build-essential cmake clang pkg-config
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. ~/.cargo/env && cargo --version
```

### 4. Windows

- git, Rust(rustup, 시험 클라이언트를 Windows에서 빌드한다). `cargo test -p dongsi-server --test e2e --no-run`이 되어야 한다.
- Windows PowerShell 5.1(기본)과 pwsh 7 모두 된다.

## 스크립트로 한 번에 돌리기

저장소 루트에서:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\e2e-linux-server.ps1
```

기본값은 모드 4개 × 클라이언트 2개를 모두 돌린다(첫 실행은 Docker 이미지와 WSL 빌드 때문에 10분 넘게 걸린다). 하는 일:

1. **사전 확인**: `wsl.exe`, WSL의 git·docker·cargo, Windows의 cargo.
2. **코드 옮기기**: **커밋한 상태(HEAD)**를 `git bundle`로 WSL의 클론(`-WslDir`)에 옮긴다. 커밋하지 않은 변경은 시험하지 않는다(경고만 한다). 시험한 커밋 해시가 결과에 남는다.
3. **빌드**: `docker build`, WSL의 `cargo build -p dongsi-server-app --release`, 양쪽 클라이언트의 `cargo test ... --no-run`.
4. **모드마다 서버를 새로 띄운다**(이 스크립트가 만든 컨테이너·볼륨·데이터 폴더만 지우고 만든다).
5. **클라이언트마다** 일반 시험 전체를 돌리고(실행 사이에 서버를 재시작해 비밀번호 실패 기록을 지운다), 이어서 **파괴적 시험**(`rate_limit`, `server_restart`)을 클라이언트마다 한 번 돌린다.
6. **정리**: 띄운 서버를 끈다(컨테이너 제거, 네이티브 중지. 이미지는 남긴다). 중간에 실패해도 정리한다.

### 모드

| 모드 | 서버 | 포트(PortBase 8310 기준) |
|---|---|---|
| `native` | WSL의 네이티브 프로세스(`scripts/linux-native-server.sh`) | 8310 |
| `bridge` | Docker 브리지, 호스트·컨테이너 같은 번호 | 8312 |
| `host` | Docker `--network host` | 8314 |
| `remap` | Docker 브리지, 호스트 8316 → 컨테이너 8317(`DONGSI_PUBLIC_PORT`) | 8316(접속) / 8317(서버) |

### 클라이언트와 주소

| 클라이언트 | 접속 주소 |
|---|---|
| `windows` | `127.0.0.1:<포트>`, `<LAN IP>:<포트>` 각각 한 번씩 |
| `wsl` | `127.0.0.1:<포트>` (이유는 아래 "알려진 한계") |

파괴적 시험은 클라이언트마다 `127.0.0.1`로 한 번 돌린다.

### 매개변수

| 매개변수 | 기본값 | 뜻 |
|---|---|---|
| `-Modes` | `native,bridge,host,remap` | 돌릴 모드(쉼표로 구분) |
| `-Clients` | `windows,wsl` | 돌릴 클라이언트 |
| `-WslDir` | `~/project/dongsi/e2e` | WSL 쪽 클론 위치. 공백 없는 리눅스 경로. 네이티브 서버 데이터는 `<WslDir>-data/native`. Git Bash 등에서 부르면 `~`가 Windows 경로로 바뀌니 따옴표로 감싼다 |
| `-PortBase` | `8310` | 포트 시작 번호(위 표). **7884 근처는 거부**한다 |
| `-LanIp` | 기본 경로 인터페이스의 IPv4 | Windows 클라이언트가 쓸 LAN IP |
| `-SkipBuild` | 끔 | 빌드를 건너뛴다(이미지·바이너리·시험 파일이 이미 있을 때) |
| `-SkipDestructive` | 끔 | 파괴적 시험을 건너뛴다(전체 시간이 크게 준다) |
| `-KeepServers` | 끔 | 끝나도 서버를 끄지 않는다(손으로 더 시험하려고) |
| `-ImageTag` | `dongsi-server:e2e` | 이미지 태그 |
| `-Prefix` | `dongsi-e2e` | 컨테이너·볼륨 이름 접두사(이름은 `<접두사>-<모드>`). 같은 PC에서 시험을 둘 이상 동시에 돌릴 때 서로 안 겹치게 |

환경 변수 `DONGSI_STOP_SIGNAL=INT`를 주면 네이티브 서버를 끌 때 SIGINT를 쓴다(SIGTERM을 처리하지 않는 옛 서버용).

예:

```powershell
# 네이티브만, Windows 클라이언트만, 파괴적 시험 없이(빠르게)
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\e2e-linux-server.ps1 -Modes native -Clients windows -SkipDestructive

# 포트 대역과 WSL 폴더를 바꿔서 전체
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\e2e-linux-server.ps1 -PortBase 8390 -WslDir '~/project/dongsi/w3' -ImageTag dongsi-server:w3 -Prefix dongsi-w3
```

### 걸리는 시간

- 파괴적 시험이 가장 길다. `rate_limit`은 실패 기록이 지워지길 61초 이상 기다려서, 실행 하나가 90~160초다. 모드 하나당 클라이언트 둘이면 파괴적 시험만 4~5분이다.
- 일반 시험 실행 하나는 약 20초(서버 재시작 포함).
- 빌드는 처음에 Docker 이미지 약 6분, WSL 서버·시험 빌드 각 몇 분. 이후는 캐시로 빠르다.

## 결과 읽는 법

결과는 `target/e2e-linux-server/<시각>/`(git 무시)에 남는다.

- `summary.md`: 시험한 커밋, 총 시간, 표(모드 × 클라이언트 × 주소 × 일반/파괴적 → 결과). 콘솔 끝에도 같은 표가 나온다.
- `run-<모드>-<클라이언트>-<주소>-<normal|destructive>.log`: 실행마다 `cargo test -- --nocapture` 전체 출력.
- `NN-serverlog-<모드>.log`: 서버(컨테이너 또는 네이티브)의 로그. `NN-*.sh`는 WSL에서 실행한 명령 스크립트와 그 출력(`.log`).
- `build-*.log`: 빌드 로그.

표의 열:

| 열 | 뜻 |
|---|---|
| 결과 | `PASS`: 종료 코드 0, 실패 0, 결과 줄이 있음. 파괴적 실행은 `rate_limit`·`server_restart`가 **건너뛰어지지 않고** 둘 다 통과해야 `PASS` |
| 통과/실패/무시 | 시험 결과 줄(`test result:`)의 수. 일반 실행은 12/0/0, 파괴적 실행은 2/0/0이 정상 |
| 건너뜀 | `[e2e] 건너뜀: <시험>` 출력. 일반 실행에서 `rate_limit,server_restart,limits`가 나오는 것은 정상(외부 서버에서 파괴적·내부 전용 시험은 건너뜀) |
| A/V 최대 | `publish_and_watch`의 영상·오디오 캡처 시각 차이 최댓값. 기준은 5ms 미만이고 보통 0.03ms 안팎 |
| 비고 | 실패한 시험 이름, 시간 초과 등 |

하나라도 실패하면 스크립트 종료 코드가 1이다. 실패 원인은 해당 `run-*.log`에서 `panicked`나 `FAILED`를 찾고, 서버 쪽은 `serverlog-*.log`를 본다.

## 손으로 하기

서버를 띄우고 환경 변수만 주면 어디서든 같은 시험을 돌릴 수 있다.

```sh
# 1. 서버를 띄운다: Docker 브리지 예
docker run -d --name dongsi-try -p 8330:8330/tcp -p 8330:8330/udp \
  -e DONGSI_PORT=8330 dongsi-server:e2e
docker logs dongsi-try 2>&1 | grep -o 'dongsi1:[A-Za-z0-9_-]*' | tail -1   # 초대 코드

# 네이티브는 도우미로(초대 코드를 출력한다)
bash scripts/linux-native-server.sh start --port 8330 --data ~/dongsi-try-data \
  --bin target/release/dongsi-server
```

```powershell
# 2. 시험 클라이언트(Windows). 초대 코드에서 지문을 읽는다.
$env:DONGSI_E2E_SERVER = '127.0.0.1:8330'
$env:DONGSI_E2E_INVITE = 'dongsi1:...'
cargo test -p dongsi-server --test e2e -- --nocapture

# 파괴적 시험까지: 서버를 재시작하는 명령을 알려 준다(Windows는 cmd /C로 실행됨)
$env:DONGSI_E2E_DESTRUCTIVE = '1'
$env:DONGSI_E2E_RESTART_CMD = 'wsl.exe -e docker restart -t 10 dongsi-try'
cargo test -p dongsi-server --test e2e -- --nocapture rate_limit server_restart
```

환경 변수 전체 표는 [main.rs 머리](../crates/server/tests/e2e/main.rs)에 있다: `DONGSI_E2E_SERVER`(주소), `DONGSI_E2E_INVITE`(초대 코드), `DONGSI_E2E_FINGERPRINT`(지문), `DONGSI_E2E_DESTRUCTIVE=1`, `DONGSI_E2E_RESTART_CMD`, `DONGSI_E2E_DISCOVERY=off`, `DONGSI_E2E_DISCOVERY_PORT`(감지 응답에서 기대하는 포트. 호스트·컨테이너 번호가 다른 remap처럼 매핑한 서버에서는 **컨테이너 포트**. 기본은 접속한 포트).

주의:

- 서버는 **비밀번호 실패를 출발지 IP마다 1분에 8번**까지만 받는다. 일반 시험 전체는 2번만 쓰지만, 같은 서버에 시험을 연달아 돌리면 쌓인다. 실행 사이에 서버를 재시작하면 지워진다. `rate_limit`은 시작할 때와 끝난 뒤에 61초씩 기다린다.
- 포트 7884는 사람이 쓰는 기본 포트다. 시험에는 다른 번호를 쓴다.
- 서버를 끝낼 때: `docker rm -f dongsi-try && docker volume rm ...` 또는 `bash scripts/linux-native-server.sh stop --data ~/dongsi-try-data`.

### 네이티브 도우미(`scripts/linux-native-server.sh`)

```sh
linux-native-server.sh start|stop|restart|status [--name 이름] [--port 번호] [--data 폴더] [--bin 실행파일]
```

- 서버를 `setsid`로 백그라운드에 띄우고, pid 파일(`dongsi-server.pid`)과 로그(`dongsi-server.log`)를 데이터 폴더에 둔다. `start`는 로그에 초대 코드가 나올 때까지 기다렸다가 그 한 줄을 출력한다.
- `stop`은 SIGTERM(`DONGSI_STOP_SIGNAL=INT`로 바꿀 수 있다)을 보내고 15초까지 기다린 뒤 안 죽으면 SIGKILL하고 경고한다. pid 파일의 프로세스가 그 실행 파일이 아니면 건드리지 않는다.

## 알려진 한계

- **브리지에서는 LAN 브로드캐스트 감지가 안 된다.** 브리지 모드에서 서브넷 브로드캐스트(`192.168.20.255`) 질의는 docker-proxy(컨테이너가 본 출발지는 `172.17.0.1`)를 거쳐 컨테이너에 **닿지만, 응답이 질의자에게 돌아오지 않았다**(2026-11-19, 같은 기계의 Windows·WSL). 반대로 Docker 호스트 자신에서는 클라이언트의 `scan()`이 docker0 브로드캐스트(`172.17.255.255`)로 브리지 컨테이너를 찾았다. 응답이 안 돌아오는 **원인은 미검증**이다([README.md의 "로컬 감지 시험 결과"](README.md)와 같다). host 네트워크와 네이티브에서는 된다. 이 시험의 `discovery`는 **유니캐스트** 질의라서 브리지에서도 통과한다. 즉 "브리지에서도 통과"가 "같은 LAN 자동 감지가 된다"는 뜻은 아니다. 같은 LAN의 **다른 PC**에서의 감지는 이 시험으로 확인하지 못한다(기계 하나).
- **같은 PC의 클라이언트는 서버에 하나의 IP로 보인다.** 브리지에서 `127.0.0.1`로 접속하면 docker-proxy를 거쳐 컨테이너는 출발지를 `172.17.0.1`로 본다. 그래서 같은 PC의 모든 클라이언트가 비밀번호 실패 횟수 제한을 **함께** 쓴다. 스크립트가 실행 사이에 서버를 재시작하는 이유다. (LAN IP·Tailscale IP로 접속하면 출발지가 유지된다.)
- **WSL 클라이언트는 `127.0.0.1`만 쓴다.** WSL 안에서 브리지 컨테이너에 **호스트의 LAN IP**로 접속하면 TCP는 열려도 TLS 중에 reset되거나 UDP 응답의 출발 포트가 바뀌어 와서(연결형 소켓이면 못 받음) ICE가 깨질 수 있다. WSL2 mirrored에서 호스트→자기 IP→docker0로 되돌리는 규칙이 없는 환경 문제로 보이며 서버 코드와 무관하다고 추정한다(미검증).
- **remap 모드의 감지 포트**: 로컬 감지 응답은 서버가 실제로 듣는 포트(컨테이너 포트)를 알린다. 이는 의도된 동작이고, 시험은 `DONGSI_E2E_DISCOVERY_PORT`로 그 포트를 기대한다.
- **한 PC에서만 시험한다.** 다른 PC·다른 LAN·인터넷 구간·공유기 NAT, 시청자 16명(상한), 4K, 장시간 운영, arm64는 이 시험의 범위 밖이다.
- 시험은 **커밋한 HEAD**만 본다. 코드를 고쳤으면 커밋한 뒤 돌린다.
- 시험 서버는 네이티브 모드의 데이터(인증서)를 매번 새로 만든다. 지문 유지(재시작해도 같은 지문)는 `server_restart`가 확인한다.
