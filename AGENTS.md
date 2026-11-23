# AGENTS.md

dongsi는 Rust로 만드는 Windows 화면 공유 앱이다. Discord 화면 공유(Go Live)만 떼어 낸 형태로, 한 사람이 화면이나 앱을 송출하고 여럿이 방송처럼 본다. 대화는 Discord 음성 채널에서 하므로 **스트림 오디오에 Discord 소리가 섞이면 안 된다.**

## 구조

| 경로 | 내용 |
|---|---|
| `crates/core` | UI·네트워크와 무관한 공통 로직: 서버 주소, 인증서 지문, 설치·서버 ID, 초대 코드(`dongsi1:` + base64url JSON), 시그널링 메시지(`proto.rs`), 로컬 감지 패킷, 코덱·화질 설정과 simulcast 트랙 상·하(`media.rs`, D-102), 스레드 우선순위·타이머(`priority.rs`) |
| `crates/media` | WGC 화면·창 캡처, 테스트 패턴, 미리 보기·썸네일 JPEG, WASAPI 루프백(Discord 제외), Opus, libavcodec(ffmpeg 8) 인코딩, simulcast 하 트랙 인코더(`simulcast.rs`, FHD 상자·3Mbps, D-102, 60fps 상한 D-100), NVENC zero-copy(D3D11 NV12 hw 풀 `ff.rs`, 여러 크기로 Blt `convert.rs`, D-099), GPU 테스트 패턴(`gpu_pattern.rs`). 단독 시험용 예제 `record` |
| `crates/net` | TLS 지문 확인, 시그널링·감지 클라이언트, UPnP, WebRTC 송출(str0m, BWE), 목표 비트레이트·simulcast 트랙 예산(`rate.rs`, 업로드가 모자라면 상만 낮춤, D-102) |
| `crates/server` | 서버 코어(시그널링, 방송 목록, 비밀번호, SFU, simulcast 트랙 고르기(`simulcast.rs`, 비트레이트 순서), 감지 응답, 썸네일, 지표). 내장 서버와 전용 서버가 같이 쓴다. `tests/e2e/`가 서버 E2E |
| `apps/server` | 전용 서버 실행 파일 `dongsi-server`. 설정 우선순위는 명령줄 > 환경 변수(`DONGSI_*`) > `dongsi-server.toml` > 기본값. Prometheus 지표는 `--metrics-addr`(기본 꺼짐) |
| `apps/desktop` | Tauri 2 앱. `src/`는 SvelteKit(Svelte 5 + TypeScript, adapter-static, 해시 라우터), `src-tauri/`는 Rust 백엔드(`hub.rs` 서버 연결, `live.rs`·`viewing.rs` 시청·감지, `broadcast.rs` 방송, `embedded.rs` 내장 서버·UPnP, `store.rs` 상태 저장). `tests/visual/` 화면 비교 하네스, `e2e/` 실제 앱 CDP 하네스 |
| `Dockerfile`, `deploy/` | 리눅스 x64 전용 서버 이미지와 compose. 설치·운영은 `deploy/README.md` |
| `scripts/` | `test-full.ps1` 전체 시험, `e2e-linux-server.ps1` 리눅스 서버 E2E, `ffmpeg/` LGPL ffmpeg 빌드, `licenses/` 제3자 고지 생성기, `package-installer.ps1`·`package-portable.ps1` 배포물(공통 함수 `lib/release.ps1`) |
| `LICENSE`, `THIRD_PARTY_NOTICES.md`(생성물), `docs/licensing.md` | 라이선스. 릴리스 절차는 `docs/licensing.md` 5절 |

- 프런트엔드 타입(`src/lib/types.ts`)은 Rust 쪽 DTO(`commands.rs`, `live.rs`, `viewing.rs`, `broadcast.rs`, `error.rs`)와 손으로 맞춘다. 한쪽을 바꾸면 다른 쪽도 바꾼다.
- 해시 라우터라서 `ssr` 같은 페이지 옵션을 쓰면 빌드가 실패한다.

## 프로젝트 메모리: `.docs/`

`.docs/`는 로컬 전용 작업 메모리다(`.git/info/exclude`). 커밋하지 않고 `git add -f`로도 넣지 않는다. 작업을 시작할 때 `.docs/README.md`(인덱스)와 관련 문서를 읽는다. 결정은 `decisions.md`, 진행 상태와 남은 일은 `plan.md`가 기준이다. 작업을 마치면 아래에 기록한다.

| 바뀐 것 | 기록할 곳 |
|---|---|
| 새 결정, 결정 변경 | `decisions.md` (기존 항목은 지우지 않고 "대체됨"으로 표시한 뒤 새 항목 추가) |
| 계획, 단계 진행 상태 | `plan.md` |
| 확인한 기술 사실, 실험·측정 결과 | `research.md` (출처나 측정 조건과 함께) |
| 세션 요약 | `log.md` 맨 위에 날짜와 함께 |

- 날짜는 절대 날짜(YYYY-MM-DD)로 쓴다. 확인하지 않은 내용은 "미검증"으로 표시한다.
- 미결 사항은 사용자에게 묻고 `decisions.md`에 기록한 뒤 진행한다. 임의로 확정하지 않는다.

## 핵심 제약

바꾸려면 먼저 `.docs/decisions.md`에 기록한다.

- 대상 OS는 Windows 11. 화면 캡처는 Windows.Graphics.Capture, 오디오는 WASAPI 프로세스 루프백.
- 스트림 오디오에서 Discord 프로세스 소리를 뺀다. 마이크는 넣지 않는다. Discord와 연동(OAuth, Rich Presence, 봇, Activity)하지 않고, ToS를 어기는 방식(셀프봇, 클라이언트 주입·수정)도 쓰지 않는다.
- 공개 서버도 "방"도 없다. 클라이언트는 주소나 초대 코드로 서버를 추가해 두고, 서버에서 방송 목록을 보고 시청하거나 방송한다.
- 방송을 시작할 때 호스팅 방식을 고른다: 추가한 서버(전용 서버, 누구나 방송) / 로컬 랜 / PC 직접 호스팅(UPnP 자동 포트 열기). 뒤의 둘은 앱 내장 서버라 호스트만 방송하고, 방송이 끝나면 서버가 닫힌다.
- 서버 비밀번호는 없다. 비밀번호는 방송자가 방송마다 건다(선택, 전용·내장 서버가 같다).
- 같은 LAN이나 tailnet에서 시작된 방송은 자동으로 감지해 보여 준다.
- 기본 포트는 7884다(TCP 시그널링, UDP 미디어·로컬 감지).
- 전송은 WebRTC. 서버(SFU)와 송출은 `str0m`, 시청은 앱의 WebView2(시청도 앱 설치 필수).
- 전용 서버에서는 여러 명이 동시에 방송한다. 방송 하나당 시청자 2~10명, 최고 화질 1440p(QHD, D-097). 4K는 없다.
- 저지연을 지향하지만 고정 지연 목표는 두지 않는다(D-095). 시청 버퍼가 끊김에 따라 150~500ms 사이에서 움직인다(D-094).
- 우리 코드는 MIT, ffmpeg는 LGPL 빌드(`--enable-gpl`·`--enable-nonfree` 없음, DLL로 연결, 소스 묶음을 릴리스에 첨부)로 배포한다(D-061·D-062). 클라이언트는 NSIS 설치 파일, 서버는 GHCR Docker 이미지로 GitHub 릴리스와 함께 낸다(D-063).

## 작업 규칙

- 문서와 설명은 한국어로 쓰고, 코드 식별자와 기술 용어는 원문을 쓴다.
- 아직 정식 릴리스 전이다(GitHub의 0.1.0은 내부 시험용). 이전 버전과의 호환이나 마이그레이션은 신경 쓰지 않는다. 시그널링을 호환되지 않게 바꾸면 `PROTOCOL_VERSION`(`crates/core/src/proto.rs`)만 올린다(D-078).
- Rust stable, cargo workspace(edition 2024). 새 크레이트는 `.docs/plan.md`의 구조(D-047)를 따른다.
- 라이선스 의존성(`Cargo.lock`, `apps/desktop/package-lock.json`, 화면에 쓰는 npm 패키지)이나 ffmpeg 빌드가 바뀌면 `node scripts/licenses/generate.mjs --ffmpeg <FFmpeg 폴더>`로 `THIRD_PARTY_NOTICES.md`를 다시 만들어 같이 커밋한다(`--check`로 최신인지 본다). GPL·AGPL 의존성과 GPL 전용 ffmpeg 부품(libx264 등)은 넣지 않는다. 그 밖의 copyleft·MPL은 `docs/licensing.md`대로 확인해 적는다.
- 커밋은 작업 브랜치에 의미 단위로 한다. push는 사용자가 요청할 때만 한다.

## 빌드 환경

`dongsi-media`는 ffmpeg 8 **LGPL** 개발 패키지(include, lib, bin의 DLL)와 bindgen용 libclang이 필요하다. ffmpeg는 `scripts/ffmpeg/build.sh`(WSL Docker, MinGW-w64)로 만든다. 저장소의 `third_party/`(git 제외)에 정션을 두면 `.cargo/config.toml`이 `FFMPEG_DIR`·`LIBCLANG_PATH`로 쓴다(환경 변수를 직접 주면 그것이 우선).

```powershell
New-Item -ItemType Directory -Force third_party
New-Item -ItemType Junction -Path third_party\ffmpeg -Target <ffmpeg LGPL 빌드 결과 폴더>
New-Item -ItemType Junction -Path third_party\libclang -Target <LLVM의 bin 폴더(libclang.dll)>
```

- 실행할 때 ffmpeg의 `bin`(`avcodec-62.dll` 등)이 PATH에 있어야 한다.
- **정션이 든 폴더(worktree, 복사본)를 `git worktree remove`·`rm -rf`·`Remove-Item -Recurse`로 지우지 않는다.** 정션을 따라가 원본(ffmpeg, LLVM 설치 폴더)을 지운다. 먼저 `cmd /c rmdir <정션>`으로 링크만 끊는다. worktree에도 같은 정션을 만들어야 하므로 특히 조심한다.
- cargo target은 저장소(worktree는 그 worktree)의 기본 `target/`을 쓴다. `CARGO_TARGET_DIR`로 다른 드라이브에 두지 않는다. 빌드 결과·시험 출력도 마찬가지다.
- GPL ffmpeg 빌드(BtbN `gpl-shared` 등)로도 개발·시험은 되지만 **배포하지 않는다**. 설치 파일은 `scripts/package-installer.ps1 -FfmpegDir <LGPL 빌드 폴더>`, 포터블은 `scripts/package-portable.ps1 -FfmpegDir <LGPL 빌드 폴더>`로 만든다. `tauri.conf.json`은 `bundle.active = false`라 `tauri build`만으로는 설치 파일이 생기지 않는다.

## 시험

빠른 시험, 대상 시험, 전체 시험의 세 단계다. 시간은 이 PC(12스레드)에서 2026-11-22에 잰 대략값이다.

- 자동 시험은 모두 이 PC 한 대에서 돈다. 다른 PC 사이 연결, tailnet, UPnP 실환경, 실제 화면·창 캡처, Discord 소리 제외, 지연 체감은 사람이 확인한다(`.docs/checklist.md`).
- **고치는 동안**에는 빠른 시험과 고친 곳의 대상 시험만 돌린다.
- **작업을 마칠 때(커밋·합치기 직전)** 전체 시험을 한 번 돌린다. 실패한 시험은 원인을 보고 그 시험만 대상 시험으로 다시 돌린다.
- 같은 시험을 되풀이해 재지 않는다. 간헐 실패도 3번 안쪽에서 멈추고 결과를 그대로 보고한다. 지연·A/V처럼 되풀이 측정이 필요하면 먼저 사용자에게 묻는다.
- **창이나 소리가 나는 시험**(CDP 하네스 `npm run e2e`, `npm run tauri dev`, 창을 띄우는 `--ignored` 시험)은 사람이 이 PC를 쓰는 중이므로 먼저 사용자에게 알리고 돌린다. 실제 화면을 캡처하는 시험은 전체 시험에만 둔다.
- 새 시험이 창·소리·실제 화면·창 캡처·LAN 주소 포트·외부 서버를 쓰거나 몇 초 넘게 걸리면 전체 시험에 넣는다. 시험 파일이면 `Cargo.toml`에 `[[test]] … required-features = ["full-tests"]`, 단위 시험 하나면 `#[cfg_attr(not(feature = "full-tests"), ignore = "<이유>")]`(예: `crates/media/src/thumbnail.rs`). 시험을 지우거나 단언을 약하게 만들어 빠르게 하지 않는다.

### 빠른 시험

창·소리·실제 캡처·외부 서버·고정 포트를 쓰지 않는 단위 시험이다(127.0.0.1 포트 0만 쓰는 짧은 시험 포함).

| 명령 | 어디서 | 시간 |
|---|---|---|
| `cargo test --workspace`, `cargo test -p <crate> --lib` | 루트 | 빌드돼 있으면 약 7초, 한 파일 고친 뒤 40~50초 |
| `npm test`(vitest), `npm run check`(svelte-check) | `apps/desktop` | 5~30초, 15~25초 |
| `cargo fmt --all`, `cargo clippy --workspace --all-targets` | 루트 | clippy는 빌드돼 있으면 약 12초 |

### 대상 시험

전체 시험용 시험 파일은 `--features full-tests`를 줘야 빌드된다. `--` 뒤에 시험 이름 일부를 주면 그 시험만 돈다(목록은 `-- --list`). `-p`로 처음 돌리면 30~80초 다시 빌드한다.

| 고친 곳 | 명령 | 주의 |
|---|---|---|
| core·net·서버 코어 단위·전용 서버 앱 | `cargo test -p dongsi-core` / `-p dongsi-net` / `-p dongsi-server --lib` / `-p dongsi-server-app` | |
| 서버 시그널링·SFU·감지·썸네일 실제 경로 | `cargo test -p dongsi-server --features full-tests --test e2e -- <이름>` | 같은 프로세스에 서버(포트 0). 밖의 서버를 대상으로 하려면 `tests/e2e/main.rs` 머리 문서 |
| 데스크톱 백엔드 | `cargo test -p dongsi-desktop --lib` | |
| 내장 서버·로컬 랜 방송·감지 흐름 | `cargo test -p dongsi-desktop --features full-tests --test lan_broadcast -- <이름>` | LAN 주소 8200~8299. 새 exe면 방화벽이 물을 수 있다 |
| 미디어 단위 | `cargo test -p dongsi-media --lib` | |
| 캡처·인코딩·주사율·캡처 대상 바꾸기·썸네일·simulcast 하 트랙·NVENC zero-copy | `cargo test -p dongsi-media --features full-tests --test source_fps` / `--test switch_source` / `--test simulcast` / `--test zero_copy` / `--lib thumbnail`, `cargo run -p dongsi-media --example record -- --source test --codec h264 --secs 3` | 실제 화면을 캡처한다(창은 안 뜸) |
| 창 닫힘·롤 창 따라가기 | `cargo test -p dongsi-media --features full-tests --test switch_source -- --ignored`, `cargo test -p dongsi-desktop --features full-tests --test follow_lol -- --ignored --test-threads 1` | **작은 창을 띄운다.** 전체 시험에도 없다. 먼저 알린다 |
| 화면 | `npm run check`, `npm test -- <파일>`, `npm run build` 뒤 `npm run shots -- --only <시나리오>`(`tests/visual/scenarios.mjs`) | 헤드리스 Edge라 창이 안 뜬다 |
| CDP 하네스 도우미 | `node --test e2e/lib/<파일>.test.mjs` | |
| `Dockerfile`·`deploy/`·리눅스 서버 | `scripts/e2e-linux-server.ps1 -Modes bridge -Clients windows -SkipDestructive` | WSL Docker, 커밋한 HEAD만 시험한다 |

### 전체 시험

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-full.ps1        # fmt·clippy·cargo test-full·화면, 약 4분
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-full.ps1 -App   # + 실제 앱 CDP 하네스(앱 창 두 개가 4~5분 뜬다. 먼저 알린다)
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-full.ps1 -All   # + CDP 하네스 + 리눅스 서버 E2E(서버·Docker를 바꿨을 때, 10분 넘게)
```

- `cargo test-full` = `cargo test --workspace --features full-tests --no-fail-fast`(`.cargo/config.toml` alias). 빠른 시험에 서버 E2E, 로컬 랜 방송, 실제 화면 캡처 시험이 더해진다.
- 단계가 실패해도 끝까지 돌고 `target/test-full/<시각>/summary.md`에 남긴다. 단계는 `-Skip shots,build`처럼 뺀다. `apps/desktop`에서 `npm install`을 먼저 해 둔다.
- `#[ignore]` 시험(측정용 `bench_scale` 등, 창을 띄우는 시험)은 전체 시험에도 들지 않는다. 각 시험의 머리 문서대로 따로 돌린다.
- 부하에 민감한 시험(서버 E2E의 `discovery_unicast_under_media_load`·`discovery_rate_limit`, `lan_broadcast`의 A/V 차이)은 다른 빌드와 겹치면 가끔 실패한다. 그 시험만 한 번 다시 돌리고 두 결과를 그대로 보고한다.

## 실행과 시험용 설정

- 앱은 `apps/desktop`에서 `npm run tauri dev`(창이 뜬다, vite 1420)로 띄운다. `npm run tauri build`는 release exe만 만든다.
- 전용 서버: `cargo run -p dongsi-server-app -- --data <폴더> --name <이름>`. Docker는 `deploy/README.md`를 본다. 리눅스 서버 E2E(`scripts/e2e-linux-server.ps1`)의 준비물·인자는 스크립트 머리 도움말에 있다.
- 앱 상태는 `%APPDATA%\com.dongsi.desktop\state.json`이다(지우면 첫 실행 상태, 설치 ID도 새로 생김). 내장 서버 인증서는 같은 폴더의 `host\`에 있고, 지우면 지문이 바뀌어 시청자에게 지문 경고가 뜬다.
- `DONGSI_PROFILE=<이름>`은 앱 데이터와 WebView2 데이터를 `profiles\<이름>\`로 나누고 창 제목을 `dongsi [<이름>]`으로 한다(한 PC에서 앱 두 개를 띄울 때). `DONGSI_PORT=<포트>`는 내장 서버·감지 질의 포트를 바꾼다(기본 7884). 값이 틀리면 종료 코드 2로 끝난다. 자동 시험은 사람의 앱 데이터와 7884를 쓰지 않는다.
- 비디오 입력 경로: GPU 변환이고 인코더가 nvenc면 zero-copy(D3D11 텍스처를 인코더에 바로)다. `DONGSI_MEDIA_NO_ZEROCOPY=1`은 GPU 변환 + 읽어 오기, `DONGSI_MEDIA_CPU_CONVERT=1`은 CPU 변환(swscale)으로 고른다. 파이프라인을 시작할 때 읽는다.
- 메인 창은 숨겨서 만들고, 프런트엔드가 `app_ready`를 부르면 보인다(못 부르면 10초 뒤). 창을 찾는 자동 시험은 이 시간을 기다린다.
- 개발 빌드에만 있는 것: 예시 초대 코드 넣기, "개발용: 테스트 패턴으로 바로 시작", 시청 통계의 목표 지연·stream id. 시청 화면은 2초마다 통계를 앱 로그(`dongsi_desktop_lib::webview: watch ...`)에 남긴다.
- 화면을 자동으로 조작할 때는 UIA로 통계를 자주 읽지 않는다(WebView2가 느려져 측정이 틀어진다). `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222 --disable-features=CalculateNativeWinOcclusion --disable-backgrounding-occluded-windows --disable-renderer-backgrounding"`로 띄우면 CDP(`http://127.0.0.1:9222/json/list`)로 조작할 수 있다. 사용자 화면 대신 테스트 패턴을 쓴다.
- CDP 통합 하네스는 루트에서 `cargo build -p dongsi-desktop` 뒤 `apps/desktop`에서 `npm run e2e -- --scenario all`로 돌린다. vite가 1420 고정이라 하네스 두 개를 동시에 돌리지 않는다. 자세히는 `apps/desktop/e2e/README.md`.
