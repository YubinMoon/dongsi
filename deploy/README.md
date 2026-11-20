# dongsi 전용 서버 배포 (Docker, Linux x64)

dongsi 전용 서버(`dongsi-server`)를 Docker 이미지로 돌리는 방법이다. 서버는 누구나 접속하는 방송 서버이고(비밀번호는 방송마다 방송자가 건다), 클라이언트는 **초대 코드**나 주소로 서버를 추가한다.

## 요구 사항

- BuildKit(캐시 마운트)과 compose 플러그인이 있는 Docker Engine. 이 문서는 Docker Engine 29.10, compose v5.8로 확인했다(다른 버전은 미검증).
- Linux x64 호스트. 이미지는 `linux/amd64`만 만든다.
- 호스트의 **TCP·UDP 같은 번호 포트 하나**(기본 7884)를 열 수 있어야 한다.
- compose 파일은 2026-11-19에 WSL2의 Docker Engine 29.10, compose v5.8.0에서 `up -d --build`와 `down -v`까지 확인했다(브리지 `DONGSI_PORT=8356`, host `DONGSI_PORT=8357`, 프로젝트 이름을 서로 다르게 `-p w1-compose-bridge`·`-p w1-compose-host`). 두 컨테이너 모두 `healthy`가 되었고 로그에 초대 코드(`dongsi1:...`)가 나왔으며 Windows에서 `192.168.20.47:8356`·`:8357`로 TCP 접속이 되었다.
- 처음 이미지를 빌드할 때 Rust 크레이트를 내려받는다(인터넷 필요). 빌드에는 4~6분이 걸린다(아래). 릴리스한 이미지를 받아 쓰면 빌드할 필요가 없다.

## 이미지 받기 (GHCR)

릴리스한 이미지를 GitHub Container Registry에서 받는다. 저장소를 받지 않아도 된다.

```sh
docker pull ghcr.io/yubinmoon/dongsi-server:0.1.0      # 릴리스 버전
docker pull ghcr.io/yubinmoon/dongsi-server:latest     # 가장 최근 릴리스
```

- 태그는 릴리스 버전(`0.1.0`)과 `latest`다. `linux/amd64`만 있다.
- 이미지 안 `/usr/share/doc/dongsi-server/`에 `LICENSE`(MIT)와 `THIRD_PARTY_NOTICES.md`(제3자 고지)가 있다.
- 아래 예는 이 이미지 이름을 쓴다. 직접 빌드했다면 빌드할 때 붙인 이름(예: `dongsi-server:latest`)으로 바꾼다.

## 이미지 빌드

저장소 루트에서:

```sh
docker build -t dongsi-server:latest .
```

- 빌더는 `rust:1-slim-trixie`, 실행 이미지는 `debian:trixie-slim`이다.
- 의존성과 `target`을 BuildKit 캐시 마운트에 두므로, 소스를 조금 고친 뒤 다시 빌드하면 바뀐 크레이트만 컴파일한다(측정: 처음 5분 43초, 서버 앱 소스 한 줄을 바꾼 뒤 1분 37초).
- 실행 이미지는 비루트 사용자 `dongsi`(uid/gid 10001)로 돈다.

## 실행

### docker run (브리지 네트워크)

```sh
docker run -d --name dongsi-server --restart unless-stopped \
  -p 7884:7884/tcp -p 7884:7884/udp \
  -v dongsi-data:/data \
  -e DONGSI_NAME="우리 서버" \
  -e DONGSI_ADDRESSES=203.0.113.7 \
  ghcr.io/yubinmoon/dongsi-server:latest
```

`DONGSI_ADDRESSES`에는 **클라이언트가 이 서버에 접속할 호스트 주소**(도메인, 공인 IP, LAN·Tailscale IP)를 넣는다. 브리지 네트워크의 컨테이너는 호스트 주소를 알 수 없고, 컨테이너 안에서 보이는 주소(172.17.x.x)는 밖에서 닿지 않기 때문이다.

인자를 덧붙일 수도 있다(`ENTRYPOINT`가 `dongsi-server`이다): `docker run ... ghcr.io/yubinmoon/dongsi-server:latest --name "우리 서버" --address example.com`.

### docker compose

```sh
cd deploy
DONGSI_ADDRESSES=203.0.113.7 DONGSI_NAME="우리 서버" docker compose up -d          # 릴리스 이미지(GHCR)를 받아 실행
DONGSI_ADDRESSES=203.0.113.7 docker compose up -d --build                         # 저장소 소스로 직접 빌드해 실행
DONGSI_PORT=9000 DONGSI_ADDRESSES=203.0.113.7 docker compose up -d   # 다른 포트
docker compose down        # 컨테이너 정리(볼륨·인증서는 남는다). 볼륨까지 지우려면 down -v
```

- compose 파일의 이미지는 `ghcr.io/yubinmoon/dongsi-server:${DONGSI_IMAGE_TAG:-latest}`다. `--build` 없이 실행하면 이 이미지를 받아 쓰고(로컬에 없을 때), `--build`를 주면 저장소 소스(`build.context: ..`)로 빌드해 같은 이름을 붙인다. 버전을 고정하려면 `DONGSI_IMAGE_TAG=0.1.0`.
- `deploy/docker-compose.yml`: 브리지 네트워크, 호스트 `${DONGSI_PORT:-7884}` TCP·UDP를 컨테이너의 같은 번호로 공개, 볼륨 `dongsi-data`, `restart: unless-stopped`. **포트는 `DONGSI_PORT` 하나로 정한다**(ports와 서버 설정에 같은 값이 들어간다). `DONGSI_PORT`·`DONGSI_ADDRESSES`·`DONGSI_NAME`은 셸 환경 변수나 같은 폴더의 `.env` 파일로 준다.
- `deploy/docker-compose.host.yml`: `network_mode: host`. 아래 "네트워크 모드 비교"를 보고 고른다. `docker compose -f docker-compose.host.yml up -d`(직접 빌드는 `--build`). 포트는 마찬가지로 `DONGSI_PORT`(서버가 호스트에서 직접 연다).
- 같은 호스트에 서버를 둘 띄우려면 프로젝트 이름(`docker compose -p <이름> ...`)과 `DONGSI_PORT`를 서로 다르게 준다. 프로젝트마다 볼륨(`<이름>_dongsi-data`)이 따로 생기므로 인증서·지문도 따로다.

## 초대 코드 보기

서버는 시작할 때 초대 코드를 한 줄로 출력한다. 클라이언트의 "서버 추가"에 붙여 넣는다.

```sh
docker logs dongsi-server 2>&1 | grep -o 'dongsi1:[A-Za-z0-9_-]*' | tail -1
# compose: docker compose logs dongsi-server | grep -o 'dongsi1:[A-Za-z0-9_-]*' | tail -1
```

로그에는 서버 이름, 포트, 주소, **지문**(인증서 SHA-256, base64url)도 나온다. 초대 코드에는 주소 목록, 지문, 서버 이름이 들어 있고(`dongsi1:` 뒤는 base64url JSON), 클라이언트는 이 지문으로 서버를 확인한다.

## 설정

우선순위는 **명령줄 > 환경 변수 > `dongsi-server.toml` > 기본값**이다. 설정 파일은 처음 실행할 때 데이터 폴더(`/data`)에 기본 내용으로 만들어지며, 환경 변수가 정한 값이 파일보다 우선한다. 값이 잘못되면 한국어 오류와 종료 코드 2로 끝난다. 비어 있는 환경 변수(`DONGSI_NAME=`)는 정하지 않은 것으로 본다.

| 환경 변수 | 명령줄 | toml 키 | 기본값 | 설명 |
|---|---|---|---|---|
| `DONGSI_DATA` | `--data` | (없음) | 이미지: `/data`, 직접 실행: `./dongsi-data` | 인증서와 설정을 둘 폴더 |
| `DONGSI_PORT` | `--port` | `port` | `7884` | 서버가 **컨테이너 안에서** 듣는 TCP·UDP 포트 |
| `DONGSI_PUBLIC_PORT` | `--public-port` | `public_port` | `port`와 같음 | 밖에서 보이는 포트. 호스트 포트를 **다른 번호**로 매핑할 때만 |
| `DONGSI_NAME` | `--name` | `name` | `dongsi 서버` | 목록·로컬 감지에 보이는 이름 |
| `DONGSI_ADDRESSES` | `--address`(여러 번) | `addresses` | 없음 | 초대 코드에 넣을 주소. 쉼표·공백으로 구분. 세 곳의 주소를 **합친다**(순서: 명령줄, 환경 변수, 파일). 포트를 안 붙이면 공개 포트를 쓴다 |
| `DONGSI_AUTO_ADDRESSES` | `--no-auto-addresses`(끔) | `auto_addresses` | `true` | 이 기계의 LAN·Tailscale 인터페이스 주소를 초대 코드에 더 넣을지(`true`/`false`/`1`/`0`) |
| `DONGSI_MAX_BROADCASTS` | (없음) | `max_broadcasts` | `16` | 동시 방송 수 |
| `DONGSI_MAX_VIEWERS` | (없음) | `max_viewers_per_broadcast` | `16` | 방송 하나당 시청자 수 |
| `DONGSI_DISCOVERY` | (없음) | `discovery` | `true` | 같은 LAN·tailnet의 감지 질의에 응답할지(`true`/`false`/`1`/`0`) |
| `DONGSI_IN_CONTAINER` | (없음) | (없음) | 이미지에서 `1` | 컨테이너 안임을 알린다(`/.dockerenv`가 있어도 같다). 브리지 네트워크 경고에만 쓴다 |
| `RUST_LOG` | | | `info` | 로그 수준 |

- 초대 코드의 주소는 **설정한 주소**(포트 없으면 공개 포트), 그다음 **자동 주소**(Tailscale, LAN 인터페이스, 실제 포트) 순서다. 둘 다 없으면 `127.0.0.1`을 넣고 경고를 낸다(같은 PC에서만 닿는다). 최대 6개.
- 컨테이너 안에서 설정한 주소가 없으면 "DONGSI_ADDRESSES에 호스트 주소를 넣어 달라"는 경고를 낸다.
- 설정 파일 예(`/data/dongsi-server.toml`, 기존 파일도 그대로 읽힌다):

  ```toml
  name = "우리 서버"
  port = 7884
  # public_port = 9000
  addresses = ["dongsi.example.com"]
  auto_addresses = false
  max_broadcasts = 16
  max_viewers_per_broadcast = 16
  discovery = true
  ```

  편집한 뒤 `docker restart dongsi-server`.
- `--healthcheck`는 TCP 접속만 확인하고 끝나는 하위 동작이다(이미지의 `HEALTHCHECK`가 쓴다).

## 데이터 볼륨(`/data`)

- `cert.pem`·`key.pem`(자체 서명 인증서와 개인 키)과 `dongsi-server.toml`이 들어 있다. **인증서가 서버의 신원**이다. 클라이언트는 초대 코드의 지문으로 서버를 확인한다.
- **지우면**(볼륨 삭제, 폴더 비움) 다음 시작에 새 인증서가 만들어져 **지문이 바뀐다**. 서버를 저장해 둔 클라이언트에는 지문 경고가 뜨고, 새 초대 코드로 다시 추가해야 한다. 볼륨 없이(`-v` 없이) 컨테이너를 지우고 다시 만들어도 같다.
- 서버는 시작할 때 `key.pem`을 0600(소유자만 읽기)으로 맞춘다.
- **백업**:

  ```sh
  docker run --rm -v dongsi-data:/data:ro -v "$PWD":/backup debian:trixie-slim \
    tar czf /backup/dongsi-data.tgz -C /data .
  ```

  복원은 빈 볼륨에 `tar xzf`로 풀고 소유자를 맞춘다(`chown -R 10001:10001`).
- **호스트 폴더를 마운트**하려면(`-v /srv/dongsi:/data`) 폴더의 소유자를 컨테이너 사용자에 맞춘다: `sudo chown 10001:10001 /srv/dongsi`. 이름 있는 볼륨은 처음 만들 때 자동으로 맞춰진다.

## 포트와 방화벽

- 서버는 **TCP와 UDP에 같은 번호**를 쓴다(기본 7884). TCP는 시그널링(TLS 위 WebSocket), UDP는 WebRTC 미디어와 로컬 감지다. **둘 다** 열어야 한다.
- 호스트 방화벽(예: ufw)에서 둘 다 허용한다: `sudo ufw allow 7884/tcp && sudo ufw allow 7884/udp`. Docker가 게시한 포트는 ufw 규칙을 우회할 수 있으니 `iptables`/`nftables` 결과도 확인한다.
- 다른 번호를 쓰려면 **호스트와 컨테이너를 같은 번호**로 하는 것을 권장한다: `-e DONGSI_PORT=9000 -p 9000:9000/tcp -p 9000:9000/udp`(compose는 `DONGSI_PORT=9000` 하나).
- 호스트 번호와 컨테이너 번호가 다르면(`-p 9000:7884`; compose는 `ports`를 직접 고쳐야 한다) `DONGSI_PUBLIC_PORT=9000`도 준다. 이 값이 없으면 서버가 컨테이너 안의 번호(7884)를 클라이언트에 알려 미디어가 닿지 않는다. 한계: 로컬 감지 응답은 실제로 듣는 포트(`DONGSI_PORT`)를 알려 주므로 다른 번호로 매핑하면 **감지 결과의 포트가 틀린다**(초대 코드·저장한 서버는 영향 없음).

## 네트워크 모드 비교

| | 브리지(기본, `-p`로 포트 공개) | host(`--network host`) |
|---|---|---|
| 서버가 보는 인터페이스 | 컨테이너 것(172.17.x.x)뿐 | 호스트의 모든 인터페이스 |
| 초대 코드의 주소 | **`DONGSI_ADDRESSES`로 직접** 줘야 한다. 자동 주소는 컨테이너 내부 주소라 쓸모없다(`DONGSI_AUTO_ADDRESSES=false` 권장) | 호스트의 LAN·Tailscale 주소가 자동으로 들어간다. 도메인·공인 IP는 `DONGSI_ADDRESSES`로 더한다 |
| 클라이언트에 알리는 미디어(WebRTC) 후보 주소 | 클라이언트가 **접속한 호스트 주소**(클라이언트가 Hello에 담아 보냄). 컨테이너 내부 주소가 나가지 않는다 | 같음 |
| 로컬 감지(LAN 브로드캐스트) | **되지 않는다**: 질의는 docker-proxy를 거쳐 컨테이너에 닿지만 응답이 질의자에게 돌아오지 않는다(아래 시험). Docker 호스트 자신에서는 docker0 브로드캐스트로 찾아진다 | **된다** |
| 로컬 감지(유니캐스트: 같은 PC, 호스트 IP, Tailscale 피어) | 된다 | 된다 |
| 다른 번호로 매핑 | 가능, `DONGSI_PUBLIC_PORT` 필요 | 매핑이 없다. 번호는 `DONGSI_PORT`로 바꾼다 |
| 같은 호스트의 다른 컨테이너·포트와 격리 | 된다 | 안 된다(호스트 포트를 직접 연다) |
| 권장 | 기본. 공개 서버·여러 서비스가 있는 호스트 | 같은 LAN에서 방송을 자동 감지하게 하고 싶을 때(Linux 호스트만. Docker Desktop은 host 네트워크가 VM에 붙는다) |

### 로컬 감지 시험 결과 (2026-11-19)

질의는 UDP `dsq1`+nonce 12바이트, 응답은 `dsr1{...}`다. Docker Engine 29.10, WSL2(mirrored 네트워킹, LAN 192.168.20.47/24)의 같은 기계에서 시험했다.

| 질의 대상 | 브리지(`-p 8350:8350/udp`) | host |
|---|---|---|
| 127.0.0.1 유니캐스트 | 응답 | 응답 |
| 192.168.20.47(호스트 LAN IP) 유니캐스트 | 응답 | 응답 |
| 192.168.20.255(서브넷 브로드캐스트) | **응답 없음** | 응답 |
| 255.255.255.255(WSL에서만) | 응답 없음 | 응답 |

- WSL(python3)과 Windows(PowerShell `UdpClient`) 양쪽 결과가 같았다.
- 브리지에서 LAN 브로드캐스트(192.168.20.255)가 안 되는 것은 **질의가 닿지 않아서가 아니다**. 측정으로는(서버 쪽 에코 시험, 2026-11-19) 질의가 docker-proxy를 거쳐 컨테이너에 도착하고 컨테이너가 본 출발지는 docker-proxy의 172.17.0.1이다. 그런데 **응답이 질의자(Windows·WSL)에게 돌아오지 않는다**. 응답이 안 돌아오는 정확한 원인은 확인하지 않았다.
- 반대로 **Docker 호스트 자신**에서는 클라이언트의 `scan()`이 docker0 브로드캐스트(172.17.255.255)로 브리지 컨테이너를 찾았다(WSL 클라이언트 시험, 서버 주소 172.17.0.2).
- **미검증**: 같은 LAN의 다른 PC에서 보낸 브로드캐스트·유니캐스트. 위 시험은 같은 기계(Windows 호스트와 WSL)에서만 했다.
- 따라서 브리지 모드에서는 LAN 자동 감지에 기대지 말고, 초대 코드나 주소로 서버를 추가하도록 안내한다.

## 공유기 포트포워딩·NAT

인터넷에서 접속하게 하려면:

1. 공유기에서 서버 호스트의 **TCP·UDP 같은 번호**(예: 7884)를 호스트로 포워딩한다. TCP만 열면 시그널링은 되지만 영상이 오지 않는다.
2. `DONGSI_ADDRESSES`에 공인 IP나 도메인을 넣는다(포트를 안 붙이면 공개 포트를 쓴다).
3. 공유기에서 **다른 번호**로 포워딩하면(예: 바깥 9000 → 호스트 7884) `DONGSI_PUBLIC_PORT=9000`을 주고, 컨테이너도 `-p 7884:7884/...`로 연다. 서버는 공인 주소로 접속한 클라이언트에게 외부 포트(9000)를 미디어 후보로 알린다.
4. 같은 공유기 안(LAN)의 클라이언트가 공인 IP로 접속하려면 공유기의 헤어핀 NAT(NAT 루프백)가 필요하다. 안 되면 LAN 주소도 `DONGSI_ADDRESSES`에 더한다.
5. CGNAT(통신사 공유 IP) 뒤에서는 포워딩이 불가능하다. Tailscale 같은 VPN 주소를 쓴다.

UPnP 자동 포트 열기는 앱에 내장된 서버(PC 직접 호스팅)에만 있고, 이 서버에는 없다.

## 업그레이드

릴리스 이미지를 쓸 때:

```sh
docker pull ghcr.io/yubinmoon/dongsi-server:latest
docker rm -f dongsi-server
docker run -d --name dongsi-server ...      # 같은 -v dongsi-data:/data로 다시 실행
# compose: cd deploy && docker compose pull && docker compose up -d
```

직접 빌드할 때:

```sh
git pull
docker build -t dongsi-server:latest .      # 또는 cd deploy && docker compose up -d --build
docker rm -f dongsi-server                  # compose를 쓰면 위 명령이 알아서 다시 만든다
docker run -d --name dongsi-server ...      # 같은 -v dongsi-data:/data로 다시 실행
```

데이터 볼륨을 그대로 쓰므로 인증서(지문)와 설정이 유지된다. `docker stop`은 SIGTERM을 보내고, 서버는 접속자에게 닫힘을 알린 뒤 종료 코드 0으로 끝난다(몇 초 안). 접속 중인 방송은 끊긴다.

## 레지스트리에 올리기 (릴리스할 때)

릴리스 절차는 [docs/licensing.md](../docs/licensing.md) 5절이다. 요약:

```sh
# Windows에서: 커밋한 상태로 이미지를 만들고 E2E로 시험한다(이미지는 WSL의 Docker에 남는다)
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/e2e-linux-server.ps1 -Modes bridge,host -ImageTag ghcr.io/yubinmoon/dongsi-server:<버전>
# WSL에서: latest 태그를 더하고 올린다(write:packages 권한의 토큰)
gh auth token | docker login ghcr.io -u <사용자> --password-stdin
docker tag ghcr.io/yubinmoon/dongsi-server:<버전> ghcr.io/yubinmoon/dongsi-server:latest
docker push ghcr.io/yubinmoon/dongsi-server:<버전>
docker push ghcr.io/yubinmoon/dongsi-server:latest
```

이미지는 OCI index로 올라가므로 GHCR가 저장소 연결에 쓰는 `org.opencontainers.image.source`를 index annotation으로도 붙인다(`docker buildx imagetools create --annotation "index:..."`, 명령 전체는 docs/licensing.md 5절). 처음 게시한 패키지는 비공개이고 API로는 바꿀 수 없으므로 패키지 설정(Danger Zone → Change visibility)에서 공개로 바꾼다. 서버는 미디어를 디코딩하지 않으므로 이미지에 FFmpeg는 없다.

## 문제 해결

- **`docker ps`가 `unhealthy`**: `HEALTHCHECK`는 환경 변수·설정 파일의 포트로 `127.0.0.1`에 TCP 접속을 시도한다. 명령줄(`docker run ... --port 9000`)로 포트를 바꾸면 헬스체크는 그것을 몰라 실패하므로 `DONGSI_PORT`를 쓴다. host 네트워크에서 같은 포트를 다른 프로세스가 쓰고 있어도 이유가 된다.
- **로그에 "컨테이너 안에서 실행 중입니다" 경고**: `DONGSI_ADDRESSES`가 비어 있다는 뜻이다. host 네트워크라면 무시해도 된다.
- **초대 코드에 `172.17.x.x`가 보인다**: 브리지 네트워크에서 `DONGSI_AUTO_ADDRESSES=false`로 끄고 `DONGSI_ADDRESSES`만 쓴다(compose 파일은 그렇게 해 두었다).
- **영상은 안 나오고 접속은 된다**: UDP 포트가 막혔다. 방화벽·공유기에서 UDP를 같은 번호로 열었는지, 번호를 다르게 매핑했다면 `DONGSI_PUBLIC_PORT`를 줬는지 본다.
- **WSL 안에서 호스트의 LAN IP로 접속하면 TLS가 끊긴다(WSL2에서만 관찰)**: WSL2 셸에서 브리지 컨테이너에 호스트의 LAN·Tailscale IP(192.168.20.47 등)로 접속하면 TCP는 열리지만 TLS 중에 reset됐다(`127.0.0.1`, `172.17.0.1`은 정상). WSL2의 iptables에 이 경로(자기 IP → docker0)를 되돌리는 규칙이 없어 보이며(추정) 서버 문제는 아니다. 같은 기계의 **Windows 호스트에서 192.168.20.47로 접속**한 E2E 시험은 미디어까지 모두 통과했다. 다른 PC에서의 접속은 **미검증**이다. WSL 안에서는 `127.0.0.1`로 접속한다.
- 로그 수준을 올리려면 `-e RUST_LOG=debug`. 로그는 색 코드 없이 나온다(터미널에서 직접 돌릴 때만 색이 있다).

## 통신 시험

[통신 시험 안내](e2e-test.md)를 본다(`scripts/e2e-linux-server.ps1`).
