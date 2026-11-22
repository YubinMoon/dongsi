# AGENTS.md

이 저장소(dongsi)는 Rust로 만드는 Windows 화면 공유 앱이다. Discord의 화면 공유(Go Live)만 떼어 낸 형태로, 한 사람이 화면이나 앱을 송출하고 다른 사람들이 방송처럼 시청한다. 대화는 Discord 음성 채널에서 하므로 **스트림 오디오에 Discord 소리가 섞이면 안 된다.**

## 현재 상태

- 단계: 0.2.0 통합 완료(2026-11-19). 0.1.0(전용 서버·내장 서버, 방송, WebView2 시청, 로컬 감지, 방송 중 화질·코덱 변경)에 더해 B 디자인(라이브 보드) 클라이언트(D-052~D-055: 서버 탭, 접속 실패·지문 경고, 도중 끊김 화면), 감지된 방송의 비밀번호 기억(D-057), 리눅스 x64 Docker 전용 서버, 시험용 `DONGSI_PROFILE`·`DONGSI_PORT`, 미디어·감지 경로 고장 수정(미디어 전 SR 차단, 키프레임 요청 재시도, 송출 침묵 3초면 방송 종료, 감지 응답 조각 나눔: D-058·D-059), 시청 립싱크 보정과 첫 시청 예열(D-060), 라이선스 정리(우리 코드 MIT + ffmpeg 최소 LGPL 빌드를 앱과 함께 배포, `scripts/ffmpeg/`, `scripts/package-portable.ps1`, `docs/licensing.md`: D-061·D-062)가 들어갔다. 0.1.0을 GitHub 릴리스로 배포했다(2026-11-20, D-063): 클라이언트는 NSIS 설치 파일(`scripts/package-installer.ps1`), 서버는 GHCR 이미지 `ghcr.io/yubinmoon/dongsi-server`(태그 `0.1.0`·`latest`). 코드 서명·자동 업데이트는 없다.
- 0.1.0 뒤(2026-11-20~22, 아직 릴리스 전): 내 방송은 시청하지 않음(D-065), 방송 중 송출 화면·캡처 대상 미리 보기(D-066·D-068), 롤 게임 창 따라가기(D-069), 시작 창(D-070), 홈 "방송 중"과 저장한 서버 자동 연결(D-071), 시청 버퍼 100~300ms(D-072), 홈은 방송 목록만·저장한 서버는 모두 위쪽 탭(D-073), 해상도(FHD·QHD·원본)·주사율(30·60·144·원본) 따로·비트레이트 슬라이더·부족/적당/충분 구간·자동 적용(D-074), 코덱은 H.264·AV1만(D-075, 프로토콜·서버·ffmpeg 빌드까지), 방송 목록 썸네일(D-076, 1분마다, 비밀번호 방송은 숨김), 시청 화면 정리(D-077: 막대 3초 뒤 숨김, 빨간 "나가기", 배포 빌드는 통계 꺼짐), 이전 버전 호환 코드 정리와 프로토콜 버전 2(D-078: 0.1.0과는 접속하지 않음), 서버 화면의 ⋯ 메뉴 없앰(D-079: 연결 끊기·다시 연결·이름 변경·삭제는 위쪽 탭 메뉴에서만).
- 자동 시험: `cargo test --workspace`(단위 시험), `cargo test-full`(단위 시험 + 서버 E2E, 로컬 랜 방송 통합, 실제 화면 캡처 시험), vitest, 화면 비교 하네스(mock IPC), 리눅스 서버 E2E 스크립트(WSL Docker), 실제 앱 두 개를 CDP로 조작하는 통합 하네스(`apps/desktop/e2e`). 언제 무엇을 돌리는지는 "빌드·테스트"의 시험 단계를 따른다. 모두 한 PC에서만 돌렸다. 다른 PC 사이 연결·tailnet·UPnP 실환경·4K·실제 화면·창 캡처·Discord 소리 제외·지연 체감은 사람이 확인한다(`.docs/checklist-0.2.0.md`).
- 남은 일은 `.docs/plan.md` 단계표를 본다. 세부 계획, 결정, 조사 내용은 `.docs/`에 있다.

## 구조

- `crates/core` (dongsi-core): UI·네트워크와 무관한 공통 로직. 서버 주소 파싱, 인증서 지문, 설치 ID·서버 ID, 초대 코드(`dongsi1:` + base64url JSON), 서버 목록, 시그널링 메시지(`proto.rs`), 로컬 감지 패킷(`discovery.rs`), 코덱(H.264·AV1)·화질 설정(`media.rs`: 크기 상한 0×0·`fps` 0 = 원본, D-074·D-075)
- `crates/media` (dongsi-media): WGC 화면·창 캡처와 테스트 패턴, 캡처 대상 미리 보기와 방송 목록 썸네일 JPEG(`thumbnail.rs`, WIC JPEG, `preview_jpeg`), 모니터 주사율(`source.rs` `RefreshRates`, 주사율 원본), 방송 중 캡처 대상 바꾸기(`VideoPipeline::switch_source`, 시험 `tests/switch_source.rs`, 전체 시험에서만), WASAPI 루프백(Discord 제외), libavcodec(ffmpeg 8) 인코딩, QPC 시계. 예제 `record`로 단독 시험
- `crates/net` (dongsi-net): TLS 지문 확인, 시그널링 클라이언트, 로컬 감지 클라이언트, UPnP, WebRTC 송출(str0m)
- `crates/server` (dongsi-server): 서버 코어(시그널링, 방송 목록, 비밀번호 검사, SFU, 감지 응답, 방송 목록 썸네일 보관·배포). 내장 서버와 전용 서버가 같이 쓴다. `tests/e2e/`가 실제 서버로 송출·시청·A/V 동기·비밀번호·종료 경로·감지·썸네일을 확인한다(전체 시험에서만, `full-tests` 기능). 기본은 시험 안에서 띄운 서버이고, `DONGSI_E2E_SERVER`(주소)나 `DONGSI_E2E_INVITE`(초대 코드)를 주면 밖의 서버를 대상으로 돈다(사용법은 `tests/e2e/main.rs` 머리 문서)
- `apps/server` (dongsi-server-app, 실행 파일 `dongsi-server`): 전용 서버. `--data`(기본 `./dongsi-data`)에 인증서와 `dongsi-server.toml`을 두고, 시작할 때 초대 코드를 출력한다. 설정 우선순위는 명령줄 > 환경 변수(`DONGSI_PORT`, `DONGSI_PUBLIC_PORT`, `DONGSI_NAME`, `DONGSI_ADDRESSES`, `DONGSI_AUTO_ADDRESSES` 등) > toml > 기본값. SIGTERM으로 정상 종료하고, `--healthcheck`는 TCP 접속만 확인한다
- `Dockerfile`(저장소 루트)과 `deploy/`: 리눅스 x64 전용 서버 Docker 이미지(빌더 `rust:1-slim-trixie`, 실행 `debian:trixie-slim`, 사용자 uid 10001, 볼륨 `/data`, `/usr/share/doc/dongsi-server/`에 LICENSE·제3자 고지, 릴리스 이미지는 GHCR `ghcr.io/yubinmoon/dongsi-server`), compose(브리지 `docker-compose.yml`, host 네트워크 `docker-compose.host.yml`, 이미지는 GHCR 이름이고 `--build`면 소스로 빌드), 설치·운영 안내 `deploy/README.md`, 통신 시험 안내 `deploy/e2e-test.md`
- `LICENSE`(MIT), `THIRD_PARTY_NOTICES.md`(제3자 고지, **생성물**), `docs/licensing.md`(라이선스 정리, LGPL 의무 대응, 릴리스 절차): 우리 코드는 MIT, ffmpeg는 LGPL 빌드(D-061·D-062)
- `scripts/ffmpeg/`: 최소 LGPL ffmpeg 8을 WSL Docker에서 MinGW-w64로 교차 빌드하는 스크립트(`build.sh`). 결과 폴더에 `bin`(DLL 3개)·`include`·`lib`·`licenses/components.json`·`BUILDINFO.md`·`source/`(소스 묶음)가 생긴다
- `scripts/licenses/`: `THIRD_PARTY_NOTICES.md` 생성기(`generate.mjs`, cargo-about 설정 `about.toml`). `scripts/package-installer.ps1`: NSIS 설치 파일(`dist/dongsi-<버전>-windows-x64-setup.exe`, 사용자별 설치, FFmpeg DLL·라이선스 파일을 exe 옆에, 시작 메뉴 바로 가기 이름은 `동시`: hook `apps/desktop/src-tauri/windows/installer-hooks.nsh`, D-064)을 만든다. `scripts/package-portable.ps1`: 포터블 폴더·zip(`dist/`, git 제외)을 만든다. 두 스크립트의 공통 함수(FFmpeg LGPL 검사, DLL 요구 점검, 라이선스 파일, README)는 `scripts/lib/release.ps1`
- `scripts/`: `test-full.ps1`(전체 시험을 한 번에, "빌드·테스트"), `e2e-linux-server.ps1`(Windows에서 WSL의 리눅스 서버를 네이티브·Docker 브리지·host·다른 번호 매핑으로 띄우고 서버 E2E를 Windows·WSL 클라이언트로 돌린다), `linux-native-server.sh`(그 도우미)
- `apps/desktop`: Tauri 2 앱
  - `src/`: SvelteKit(Svelte 5 + TypeScript) 화면. adapter-static과 해시 라우터(`#/server/<id>`)를 쓴다. 해시 라우터에서는 `ssr` 같은 페이지 옵션을 쓰면 빌드가 실패한다. 홈의 "방송 중"은 저장한 서버(앱이 켜지면 자동으로 연결, 실패하면 5~60초 간격으로 다시 시도)의 방송과 로컬 감지 방송을 한 목록으로 보인다(`components/home/LiveBroadcasts.svelte`, `home.ts` `liveTiles`, 자동 연결은 `stores/connections.svelte.ts`, D-071). 홈에는 서버 카드가 없고, 저장한 서버는 모두 위쪽 탭에 상태(연결 중·실패·지문 다름 등)와 메뉴(연결 끊기/다시 연결·이름 변경·삭제)로 둔다(`tabs.ts`, `shell/TopBar.svelte`, D-073). 서버 화면에는 이 메뉴를 두지 않는다(D-079). 비트레이트 구간 계산 `bitrateZones.ts`(D-074), 방송 타일 썸네일 `stores/thumbnails.svelte.ts`(D-076), 시청 막대 자동 숨김 `overlayIdle.ts`(D-077)
  - `src-tauri/` (dongsi-desktop): 일반 명령(`commands.rs`), 서버 연결 허브(`hub.rs`, 서버마다 시그널링 연결 하나, Tauri 없이 시험할 수 있게 `HubHost`로 분리), 접속·시청·감지 명령(`live.rs`)과 그 로직(`viewing.rs`), 방송 명령(`broadcast.rs`), 시작 창(`splash.rs`, 실행하자마자 뜨는 작은 로딩 창을 Win32·Direct2D로 그린다. 메인 창은 숨겨서 만들고 프런트엔드가 `app_ready`를 부르면 보인다. D-070), 롤 게임 창 따라가기(`follow.rs`, D-069. 가짜 롤 창 시험 `tests/follow_lol.rs`는 `--features full-tests` + `--ignored`), 내장 서버·UPnP(`embedded.rs`), 송출 연결(`session.rs`), 앱 폴더와 시험용 환경 변수(`paths.rs`), 명령 오류(`error.rs`, `message`·`code`·`detail`), 상태 저장(`store.rs`, 앱 데이터 폴더의 `state.json`)
  - `src-tauri/tests/lan_broadcast.rs`: Tauri 없이 앱 코드 그대로 "내장 서버로 방송 → 다른 앱이 감지해 시청"을 확인하는 통합 시험(시청 쪽은 str0m 수신기, 전체 시험에서만)
  - `tests/visual/`: 화면 비교 하네스(mock Tauri IPC + 헤드리스 Edge, 시나리오 34개를 `npm run shots`로 찍음)
  - `e2e/`: 실제 앱 두 개를 띄워 CDP로 조작하는 통합 하네스(로컬 랜 방송, WSL Docker 서버, 화면 25개 스크린샷). 사용법 `apps/desktop/e2e/README.md`
  - 프런트엔드 타입(`src/lib/types.ts`)은 Rust 쪽 DTO(`commands.rs`, `live.rs`, `viewing.rs`, `broadcast.rs`, `error.rs`)와 손으로 맞춘다. 한쪽을 바꾸면 다른 쪽도 바꾼다.

## 프로젝트 메모리: `.docs/`

`.docs/`는 이 프로젝트의 작업 메모리다. `.git/info/exclude`에 등록되어 있어 git에 올라가지 않는다(로컬 전용).

**작업을 시작할 때**

1. `.docs/README.md`(인덱스)를 읽는다.
2. 작업과 관련된 문서를 읽는다. 결정 사항은 `.docs/decisions.md`가 기준이다.

**작업을 마칠 때**

| 바뀐 것 | 기록할 곳 |
|---|---|
| 새 결정, 결정 변경 | `decisions.md` (기존 항목을 지우지 말고 "대체됨"으로 표시한 뒤 새 항목 추가) |
| 계획, 단계 진행 상태 | `plan.md` |
| 확인한 기술 사실, 실험·측정 결과 | `research.md` (출처나 측정 조건을 함께 적음) |
| 세션 요약 | `log.md` 맨 위에 날짜와 함께 추가 |

**규칙**

- `.docs/`는 커밋하지 않는다. `git add -f`로 강제로 추가하지 않는다.
- 날짜는 절대 날짜(YYYY-MM-DD)로 쓴다.
- 확인한 사실과 추측을 구분한다. 확인하지 않은 내용은 "미검증"으로 표시한다.
- 미결 사항은 사용자에게 묻고 `decisions.md`에 기록한 뒤 진행한다. 임의로 확정하지 않는다.

## 핵심 제약

바꾸려면 먼저 `.docs/decisions.md`에 기록한다.

- 대상 OS는 Windows 11이다. 화면 캡처는 Windows.Graphics.Capture, 오디오는 WASAPI 프로세스 루프백을 쓴다.
- 스트림 오디오에서 Discord 프로세스의 소리를 제외한다. 마이크는 넣지 않는다. Discord와 그 밖의 연동(OAuth, Rich Presence, 봇, Activity)은 하지 않는다.
- 공개 서버는 두지 않고, "방" 개념도 없다. 클라이언트는 주소(또는 초대 코드)로 서버를 추가해 목록에 두고, 서버에 들어가 방송 목록을 보고 시청하거나 방송한다.
- 방송을 시작할 때 호스팅 방식을 고른다: 추가한 서버(전용 서버, 누구나 방송) / 로컬 랜 / PC 직접 호스팅(UPnP 자동 포트 열기). 뒤의 두 방식은 앱에 내장된 같은 서버를 쓰며, 호스트만 방송하고, 방송이 끝나면 서버가 닫힌다.
- 서버에는 비밀번호가 없고 누구나 접속한다. 비밀번호는 방송 단위로, 방송을 켤 때 방송자가 직접 건다(선택). 전용 서버와 내장 서버가 똑같다.
- 같은 LAN이나 tailnet에서 시작된 방송은 자동으로 감지해 서버 목록에 보여 준다.
- 기본 포트는 7884다. TCP는 시그널링, UDP는 미디어와 로컬 감지에 쓴다.
- 전송은 WebRTC다. 서버(SFU)와 송출은 `str0m`으로 만들고, 시청은 Tauri 앱의 WebView2에서 한다. 시청도 앱 설치가 필수다.
- 전용 서버에서는 여러 명이 동시에 방송할 수 있다. 방송 하나당 시청자는 2~10명, 최고 화질은 4K다.
- 우리 코드는 MIT, ffmpeg는 LGPL 빌드(`--enable-gpl`·`--enable-nonfree` 없음, DLL로 연결, 소스 묶음을 릴리스에 첨부)로 배포한다(D-061·D-062). 클라이언트는 NSIS 설치 파일, 서버는 GHCR Docker 이미지로 GitHub 릴리스와 함께 배포한다(D-063). 릴리스 절차는 `docs/licensing.md` 5절.
- Discord ToS를 어기는 방식(셀프봇, 클라이언트 주입·수정)은 쓰지 않는다.
- 저지연을 목표로 한다(화면에서 화면까지 500ms 이하).

## 작업 규칙

- 문서와 설명은 한국어로 쓰고, 코드 식별자와 기술 용어는 원문을 쓴다.
- 정식 릴리스 전에는 이전 버전(0.1.0 포함)과 섞여 쓰는 경우를 고려하지 않고 마이그레이션 코드도 만들지 않는다. 0.1.0 릴리스는 내부 시험용이다. 시그널링을 호환되지 않게 바꾸면 `PROTOCOL_VERSION`(`crates/core/src/proto.rs`)만 올린다(D-078).
- Rust stable, cargo workspace(edition 2024)를 쓴다. 새 크레이트는 `.docs/plan.md`의 구조(D-047)를 따라 필요할 때 추가한다.
- 라이선스 의존성(`Cargo.lock`, `apps/desktop/package-lock.json`, 화면에 쓰는 npm 패키지)이나 ffmpeg 빌드가 바뀌면 `node scripts/licenses/generate.mjs --ffmpeg <FFmpeg 폴더>`로 `THIRD_PARTY_NOTICES.md`를 다시 만들어 같이 커밋한다(`--check`로 최신인지 본다). GPL·AGPL 의존성이나 GPL 전용 ffmpeg 부품(libx264 등)을 넣지 않는다(ffmpeg 자신의 LGPL 외 copyleft·MPL은 `docs/licensing.md`에 따라 확인하고 적는다).
- 커밋은 작업 브랜치에 의미 단위로 한다. push는 사용자가 요청할 때만 한다.

## 빌드 환경

`dongsi-media`는 ffmpeg 8 **LGPL** 개발 패키지(include, lib, bin의 DLL)와 bindgen용 libclang이 필요하다. ffmpeg는 `scripts/ffmpeg/build.sh`로 WSL Docker에서 만든 최소 LGPL 빌드이고, `third_party/ffmpeg` 정션의 대상은 그 결과 폴더다(결과 폴더의 위치는 PC마다 다르다). 저장소의 `third_party/`(git 제외)에 정션을 만들면 `.cargo/config.toml`이 `FFMPEG_DIR`, `LIBCLANG_PATH`로 쓴다. 환경 변수를 직접 주면 그것이 우선한다.

```powershell
New-Item -ItemType Directory -Force third_party
New-Item -ItemType Junction -Path third_party\ffmpeg -Target <ffmpeg 8 shared 개발 패키지 폴더>
New-Item -ItemType Junction -Path third_party\libclang -Target <LLVM의 bin 폴더(libclang.dll)>
```

- 실행할 때 ffmpeg DLL(`avcodec-62.dll` 등)이 PATH에 있어야 한다(ffmpeg의 `bin`을 PATH에 넣는다).
- **주의: 정션이 들어 있는 폴더(worktree, 복사본)를 `git worktree remove`나 `rm -rf`, `Remove-Item -Recurse`로 지우지 않는다.** 정션을 따라가 원본(ffmpeg, LLVM 설치 폴더)을 지운다. 먼저 `cmd /c rmdir <정션>`으로 링크만 끊고 지운다. worktree에서 작업할 때도 같은 정션을 만들어야 하므로 특히 조심한다.
- 개발할 때 BtbN `gpl-shared` 같은 GPL 빌드로도 컴파일·시험은 되지만(libx264 후보는 코드에서 뺐으므로 쓰이지 않는다) **배포하지 않는다**. 배포 라이선스는 우리 코드 MIT + ffmpeg LGPL 빌드(`.docs/decisions.md` D-061·D-062, `docs/licensing.md`)다. 설치 파일은 `scripts/package-installer.ps1 -FfmpegDir <LGPL 빌드 폴더>`, 포터블 폴더는 `scripts/package-portable.ps1 -FfmpegDir <LGPL 빌드 폴더>`로 만든다. `tauri.conf.json`은 `bundle.active = false`라서 그냥 `tauri build`로는 FFmpeg DLL이 빠진 설치 파일이 만들어지지 않는다(설치 스크립트가 `--config`로 resources와 함께 켠다).

## 빌드·테스트

### 시험 단계

시험은 빠른 시험, 대상 시험, 전체 시험의 세 단계로 나눈다. 시간은 이 PC(12스레드)에서 2026-11-22에 한 번씩 잰 대략값이다. 다른 에이전트의 빌드나 앱이 같이 돌면 더 걸린다.

- **고치는 동안에는 빠른 시험과 고친 곳의 대상 시험만 돌린다.** 전체 시험을 고칠 때마다 돌리지 않는다.
- **전체 시험은 작업을 마칠 때(커밋·합치기 직전) 한 번만 돌린다.** 거기서 실패한 시험은 원인을 보고 그 시험만 대상 시험으로 다시 돌린다. 전체를 처음부터 다시 돌리지 않는다.
- **같은 시험을 수십 번 되풀이해 재지 않는다.** 간헐 실패인지 볼 때도 몇 번(3번 안쪽)에서 멈추고 결과를 그대로 보고한다. 지연·A/V처럼 되풀이해 재야 하는 측정은 먼저 사용자에게 묻는다.
- **실제 앱이나 창을 띄우는 시험은 되풀이해 재지 않는다.** CDP 하네스(`npm run e2e`), `npm run tauri dev`로 띄운 앱, 작은 창을 띄우는 `--ignored` 시험은 사람이 이 PC를 쓰는 동안 창이 떴다 사라지고, 앱에서 소리를 켜면 테스트 톤이 들린다(CDP 하네스는 기본으로 `--mute-audio`). 이 PC의 실제 화면을 캡처하는 시험(창은 안 뜸)도 전체 시험에만 둔다. 이런 시험이 꼭 필요하면 먼저 사용자(저장소 주인)에게 알리고 돌린다.
- 새 시험을 더할 때: 창·소리·실제 화면·창 캡처·LAN 주소의 포트·외부 서버를 쓰거나 몇 초 넘게 걸리면 전체 시험에 넣는다. 시험 파일(target) 전체면 그 패키지 `Cargo.toml`에 `[[test]] … required-features = ["full-tests"]`를, 단위 시험 하나면 `#[cfg_attr(not(feature = "full-tests"), ignore = "<이유>")]`를 단다(`crates/media/src/thumbnail.rs`의 `thumbnail_of_primary_monitor`가 예). 시험을 지우거나 단언을 약하게 만들어 빠르게 하지 않는다.

#### 1. 빠른 시험 (자주 돌려도 됨)

순수 로직 단위 시험이다. 창·소리·실제 화면 캡처·외부 서버·고정 포트를 쓰지 않는다. 127.0.0.1의 임의 포트(포트 0)만 쓰는 같은 프로세스 안의 짧은 시험(감지 응답·SFU 소켓, `hub`의 시험 서버)은 여기에 든다.

| 명령 | 어디서 | 무엇 | 시간 |
|---|---|---|---|
| `cargo test --workspace` | 루트 | 모든 crate의 단위 시험 194개(그중 무시 3). 전체 시험용 시험 파일은 빌드하지 않는다 | 빌드돼 있으면 약 7초. 한 파일을 고친 뒤 40~50초(대부분 다시 빌드·링크) |
| `cargo test -p <crate> --lib` | 루트 | crate 하나의 단위 시험 | 1~5초. 처음 한 번은 의존성 feature 조합이 `--workspace`와 달라 30~80초 빌드 |
| `npm test` | `apps/desktop` | vitest(`src/**/*.test.ts`, 391개) | 5~30초(첫 실행이 느림) |
| `npm run check` | `apps/desktop` | svelte-check(타입 검사) | 15~25초 |
| `cargo fmt --all`, `cargo clippy --workspace --all-targets` | 루트 | 형식, lint(전체 시험용 시험 파일은 빠진다) | clippy는 새 worktree에서 처음 약 2분 40초, 빌드돼 있으면 약 12초 |

#### 2. 대상 시험 (고친 곳만)

고친 부분을 확인하는 시험만 고른다. 전체 시험용 시험 파일은 `--features full-tests`를 줘야 빌드된다. `-p`로 패키지를 고르면 처음 한 번은 다시 빌드한다(30~80초). 시험 이름 일부를 `--` 뒤에 주면 그 시험만 돈다(목록은 `-- --list`).

| 고친 곳 | 명령(루트에서. 화면은 `apps/desktop`에서) | 시간(빌드 뒤) | 주의 |
|---|---|---|---|
| `crates/core`, `crates/net`, `apps/server` | `cargo test -p dongsi-core` / `-p dongsi-net` / `-p dongsi-server-app` | 1~2초 | |
| 서버 코어 단위(`crates/server/src`) | `cargo test -p dongsi-server --lib` | 1초 | |
| 서버 시그널링·SFU·감지·썸네일의 실제 경로 | `cargo test -p dongsi-server --features full-tests --test e2e -- <이름 일부>`(예: `passwords`, `discovery`, `thumbnails`, `av_sync`) | 시험 하나 몇~15초, 30개 전체 약 23초 | 같은 프로세스 안에 서버를 띄운다(포트 0). 감지·지연 시험은 PC 부하에 민감하다 |
| 데스크톱 백엔드(`hub`·`viewing`·`broadcast`·`store`·`paths` 등) | `cargo test -p dongsi-desktop --lib` | 약 5초 | |
| 내장 서버·로컬 랜 방송·감지 흐름(`embedded`·`session`·`viewing`) | `cargo test -p dongsi-desktop --features full-tests --test lan_broadcast -- <이름 일부>` | 30개 전체 약 15초 | LAN 주소에 내장 서버(8200~8299)를 띄운다. 새 exe면 Windows 방화벽이 물을 수 있다 |
| 미디어 단위(`crates/media/src`) | `cargo test -p dongsi-media --lib` | 1초 | |
| 캡처·인코딩 파이프라인, 주사율, 캡처 대상 바꾸기, 썸네일 캡처 | `cargo test -p dongsi-media --features full-tests --test source_fps` / `--test switch_source` / `--lib thumbnail`, 짧은 녹화 `cargo run -p dongsi-media --example record -- --source test --codec h264 --secs 3` | 시험 파일 하나 1~4초 | 이 PC의 실제 화면을 메모리로 캡처한다(창은 안 뜸) |
| 창 닫힘 기다림·롤 창 따라가기 | `cargo test -p dongsi-media --features full-tests --test switch_source -- --ignored`, `cargo test -p dongsi-desktop --features full-tests --test follow_lol -- --ignored --test-threads 1` | 몇 초 | **작은 창을 띄운다.** 전체 시험에도 들지 않는다. 먼저 사용자에게 알린다 |
| 화면(`apps/desktop/src`) | `npm run check`, `npm test -- <파일 이름 일부>`, `npm run build` 뒤 `npm run shots -- --only <시나리오>`(이름은 `tests/visual/scenarios.mjs`, 예: `b-home`, `b-live`, 쉼표로 여러 개) | build 약 12초, 시나리오 하나 약 7초 | 헤드리스 Edge라 창이 안 뜬다 |
| CDP 하네스 도우미(`apps/desktop/e2e/lib`) | `node --test e2e/lib/<파일>.test.mjs` | 1초 | 앱 불필요 |
| `Dockerfile`, `deploy/`, 리눅스에서 도는 서버 | `scripts/e2e-linux-server.ps1 -Modes bridge -Clients windows -SkipDestructive` | 일반 시험 실행 하나 약 20초 + 빌드(처음 몇 분) | WSL Docker. 커밋한 HEAD만 시험한다 |

#### 3. 전체 시험 (작업을 마칠 때 한 번)

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-full.ps1         # 기본: 앱 창·소리 없음
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-full.ps1 -App    # + 실제 앱 CDP 하네스
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-full.ps1 -All    # + CDP 하네스 + 리눅스 서버 E2E
```

| 단계 | 명령 | 시간 |
|---|---|---|
| Rust 형식·lint | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --features full-tests` | 약 15초(빌드돼 있을 때) |
| Rust 시험 | `cargo test-full`(= `cargo test --workspace --features full-tests --no-fail-fast`, `.cargo/config.toml`의 alias). 261개: 빠른 시험 + 서버 E2E 30, 로컬 랜 방송 30, 실제 화면 캡처 6(`source_fps`·`switch_source`·`thumbnail_of_primary_monitor`) | 빌드 포함 약 2분(시험 실행만 약 50초) |
| 화면 | `npm run check`, `npm test`, `node --test e2e/lib/*.test.mjs`, `npm run build`, `npm run shots`(34개) | 약 1분 30초 |
| 기본 합계 | 위 셋 | 약 4분(224초) |
| `-App` | `cargo build -p dongsi-desktop` 뒤 `npm run e2e -- --scenario all`. **앱 창 두 개가 몇 분 동안 뜬다.** UI·시청·방송 경로를 바꿨을 때 먼저 사용자에게 알리고 돌린다 | 약 4~5분(`apps/desktop/e2e/README.md`) |
| `-Linux` | `scripts/e2e-linux-server.ps1`(기본: 모드 4개 × 클라이언트 2개). 서버·Docker를 바꿨을 때 | 모드 둘(bridge·host)에 약 11분(689초, 2026-11-22 기록). 네 모드는 더 길고, 처음에는 이미지·WSL 빌드로 10분 넘게 더 |

- 단계가 실패해도 끝까지 돌고, 단계별 결과·시간·로그를 `target/test-full/<시각>/summary.md`에 남긴다. 단계를 빼려면 `-Skip shots,build`처럼 준다(이름: fmt, clippy, cargo, check, vitest, e2e-lib, build, shots, app, linux).
- FFmpeg DLL 폴더를 PATH 앞에 붙인다(`DONGSI_FFMPEG_BIN` > `FFMPEG_DIR`의 `bin` > `third_party/ffmpeg/bin`). `apps/desktop`에서 `npm install`을 먼저 해 둔다.
- 기본 `cargo test --workspace`에서 빠져 전체 시험(`full-tests`)에만 드는 것: `crates/server/tests/e2e`(서버 E2E), `apps/desktop/src-tauri/tests/lan_broadcast.rs`(로컬 랜 방송 통합), `crates/media/tests/source_fps.rs`·`switch_source.rs`(실제 화면 캡처·인코딩), `crates/media/src/thumbnail.rs`의 `thumbnail_of_primary_monitor`(주 모니터 캡처), `apps/desktop/src-tauri/tests/follow_lol.rs`(빌드만, 시험은 `--ignored`).
- `#[ignore]` 시험(측정용 `bench_scale`·`thumbnails_of_all_sources`·`publisher_notices_server_loss`, 창을 띄우는 `switch_source`의 창 닫힘·`follow_lol`)은 전체 시험에도 들지 않는다. 각 시험의 머리 문서에 있는 명령으로 따로 돌린다.
- 부하에 민감한 시험(서버 E2E의 `discovery_unicast_under_media_load`·`discovery_rate_limit`, `lan_broadcast`의 A/V 차이)은 다른 빌드와 겹치면 가끔 실패한다. 그 시험만 한 번 다시 돌려 보고, 결과(처음 실패와 다시 돌린 결과)를 그대로 보고한다.

### 그 밖의 명령

`apps/desktop`에서 (처음에는 `npm install`):

```sh
npm run check        # svelte-check (타입 검사)
npm test             # vitest (src/**/*.test.ts, 순수 로직)
npm run build        # 프런트엔드 정적 빌드 → build/
npm run shots        # 화면 비교 하네스(build/ 필요, tests/visual/README.md). --only <시나리오>로 일부만
npm run e2e -- --scenario all   # 실제 앱 CDP 통합 하네스(아래). 앱 창이 뜬다: 전체 시험에서만
npm run tauri dev    # 앱 실행 (vite dev 서버 1420 포트 + Rust 빌드). 창이 뜬다
npm run tauri build  # release exe만(tauri.conf.json의 bundle.active=false). 설치 파일은 scripts/package-installer.ps1
```

전용 서버: `cargo run -p dongsi-server-app -- --data <폴더> --name <이름>`

리눅스 Docker 전용 서버(리눅스 x64 또는 WSL의 Docker Engine, 자세히 `deploy/README.md`):

```sh
docker build -t dongsi-server:latest .                     # 저장소 루트
cd deploy && DONGSI_ADDRESSES=<클라이언트가 접속할 호스트 주소> docker compose up -d --build
docker compose logs dongsi-server | grep -o 'dongsi1:[A-Za-z0-9_-]*' | tail -1   # 초대 코드
```

- 서버는 TCP·UDP 같은 번호(기본 7884, `DONGSI_PORT`)를 쓴다. 브리지 네트워크에서는 `DONGSI_ADDRESSES`를 꼭 주고, LAN 브로드캐스트 감지는 안 된다(host 네트워크는 된다).
- 리눅스 서버 E2E(Windows에서): `powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/e2e-linux-server.ps1`(안내 `deploy/e2e-test.md`).

- 앱 상태는 `%APPDATA%\com.dongsi.desktop\state.json`에 저장된다. 지우면 처음 실행한 상태로 돌아간다(설치 ID도 새로 생김).
- 시험용 환경 변수: `DONGSI_PROFILE=<이름>`(영숫자·`-`·`_` 1~64자)을 주면 앱 데이터가 `%APPDATA%\com.dongsi.desktop\profiles\<이름>\`, WebView2 데이터가 `%LOCALAPPDATA%\com.dongsi.desktop\profiles\<이름>\EBWebView`로 나뉘고 창 제목이 `dongsi [<이름>]`이 된다(한 PC에서 앱 두 개를 띄울 때). `DONGSI_PORT=<포트>`(1024~65535)는 내장 서버 포트와 로컬 감지 질의 포트를 바꾼다(기본 7884). 값이 틀리면 폴더를 만들기 전에 종료 코드 2로 끝난다. 자동 시험은 사람의 앱 데이터와 7884를 쓰지 않는다.
- cargo target은 저장소(worktree는 그 worktree) 안의 기본 `target/`을 쓴다. `CARGO_TARGET_DIR`로 다른 드라이브(D: 등)에 두지 않는다. 빌드 결과·시험 출력도 마찬가지다.
- 내장 서버 인증서는 `%APPDATA%\com.dongsi.desktop\host\`에 있다. 지우면 지문이 바뀌어, 그 방송을 저장해 둔 시청자에게 지문 경고가 뜬다.
- 개발 빌드에서만 보이는 것: 서버 추가 대화 상자의 "예시 초대 코드 넣기", 방송 화면의 "개발용: 테스트 패턴으로 바로 시작"(테스트 패턴 + 테스트 톤), 시청 통계의 목표 지연·stream id(통계 패널은 개발 빌드도 꺼진 채 시작한다, D-084). 시청 화면은 2초마다 통계를 앱 로그(`dongsi_desktop_lib::webview: watch ...`)에 남긴다.
- 화면을 자동으로 조작해 시험할 때는 UIA로 통계를 자주 읽지 않는다(WebView2가 느려져 측정이 틀어진다). `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`를 주고 `npm run tauri dev`로 띄우면 CDP(`http://127.0.0.1:9222/json/list`)로 페이지를 조작할 수 있다. 시험할 때 사용자 화면 대신 테스트 패턴을 쓴다. 창이 가려져도 렌더링이 멈추지 않게 `--disable-features=CalculateNativeWinOcclusion --disable-backgrounding-occluded-windows --disable-renderer-backgrounding`을 같이 준다.
- 메인 창은 숨겨서 만들어지고, 프런트엔드가 첫 화면을 준비해 `app_ready`를 부르면 보인다(그 전에는 작은 시작 창만 보인다, D-070). 프런트엔드가 부르지 못하면 10초 뒤에 보인다. 창을 찾는 자동 시험은 이 시간을 기다린다.
- CDP 통합 하네스(`apps/desktop/e2e`): 저장소 루트에서 `cargo build -p dongsi-desktop`(tauri CLI 없이 만든 디버그 exe는 vite dev 서버 `localhost:1420`을 읽는 개발 모드다) 뒤 `apps/desktop`에서 `npm run e2e -- --scenario all`. 앱 두 개를 `DONGSI_PROFILE`(기본 t4a·t4b)·CDP(9225·9226)로 띄우고, 시나리오 a(로컬 랜 방송), b(WSL Docker 서버, 이미지 `--image` 기본 `dongsi-server:e2e`), shots(화면 스크린샷)를 돌린다. 소스는 테스트 패턴·테스트 톤만 쓰도록 `invoke`를 막아 둔다. 프로필·CDP·포트(`--port-base`)·Docker 이름 접두사는 인자로 바꾼다. vite는 1420 고정이라 하네스 두 개를 동시에 돌리지 않는다. 결과는 `apps/desktop/e2e/out/<시각>/summary.md`.
