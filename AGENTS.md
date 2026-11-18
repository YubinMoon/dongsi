# AGENTS.md

이 저장소(dongsi)는 Rust로 만드는 Windows 화면 공유 앱이다. Discord의 화면 공유(Go Live)만 떼어 낸 형태로, 한 사람이 화면이나 앱을 송출하고 다른 사람들이 방송처럼 시청한다. 대화는 Discord 음성 채널에서 하므로 **스트림 오디오에 Discord 소리가 섞이면 안 된다.**

## 현재 상태

- 단계: 0.1.0 구현 완료. 전용 서버·내장 서버, 방송(캡처·인코딩·송출), 시청(WebView2), 로컬 감지, 방송 중 화질·코덱 변경이 동작한다. 한 PC에서만 확인했고, 다른 PC 사이 연결·UPnP 실환경·4K는 미검증이다. 남은 일은 `.docs/plan.md` 단계표를 본다.
- 세부 계획, 결정, 조사 내용은 `.docs/`에 있다.

## 구조

- `crates/core` (dongsi-core): UI·네트워크와 무관한 공통 로직. 서버 주소 파싱, 인증서 지문, 설치 ID·서버 ID, 초대 코드(`dongsi1:` + base64url JSON), 서버 목록, 시그널링 메시지(`proto.rs`), 로컬 감지 패킷(`discovery.rs`), 화질 프리셋·코덱(`media.rs`)
- `crates/media` (dongsi-media): WGC 화면·창 캡처와 테스트 패턴, WASAPI 루프백(Discord 제외), libavcodec(ffmpeg 8) 인코딩, QPC 시계. 예제 `record`로 단독 시험
- `crates/net` (dongsi-net): TLS 지문 확인, 시그널링 클라이언트, 로컬 감지 클라이언트, UPnP, WebRTC 송출(str0m)
- `crates/server` (dongsi-server): 서버 코어(시그널링, 방송 목록, 비밀번호 검사, SFU, 감지 응답). 내장 서버와 전용 서버가 같이 쓴다. `tests/e2e.rs`가 실제 서버로 송출·시청·A/V 동기를 확인한다
- `apps/server` (dongsi-server-app, 실행 파일 `dongsi-server`): 전용 서버. `--data`(기본 `./dongsi-data`)에 인증서와 `dongsi-server.toml`을 두고, 시작할 때 초대 코드를 출력한다
- `apps/desktop`: Tauri 2 앱
  - `src/`: SvelteKit(Svelte 5 + TypeScript) 화면. adapter-static과 해시 라우터(`#/server/<id>`)를 쓴다. 해시 라우터에서는 `ssr` 같은 페이지 옵션을 쓰면 빌드가 실패한다.
  - `src-tauri/` (dongsi-desktop): 일반 명령(`commands.rs`), 서버 연결 허브(`hub.rs`, 서버마다 시그널링 연결 하나), 접속·시청·감지(`live.rs`), 방송·내장 서버(`broadcast.rs`), 상태 저장(`store.rs`, 앱 데이터 폴더의 `state.json`)
  - 프런트엔드 타입(`src/lib/types.ts`)은 Rust 쪽 DTO(`commands.rs`, `live.rs`, `broadcast.rs`)와 손으로 맞춘다. 한쪽을 바꾸면 다른 쪽도 바꾼다.

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
- Discord ToS를 어기는 방식(셀프봇, 클라이언트 주입·수정)은 쓰지 않는다.
- 저지연을 목표로 한다(화면에서 화면까지 500ms 이하).

## 작업 규칙

- 문서와 설명은 한국어로 쓰고, 코드 식별자와 기술 용어는 원문을 쓴다.
- Rust stable, cargo workspace(edition 2024)를 쓴다. 새 크레이트는 `.docs/plan.md`의 구조(D-047)를 따라 필요할 때 추가한다.
- 커밋은 작업 브랜치에 의미 단위로 한다. push는 사용자가 요청할 때만 한다.

## 빌드 환경

`dongsi-media`는 ffmpeg 8 개발 패키지(include, lib, bin의 DLL)와 bindgen용 libclang이 필요하다. 저장소의 `third_party/`(git 제외)에 정션을 만들면 `.cargo/config.toml`이 `FFMPEG_DIR`, `LIBCLANG_PATH`로 쓴다. 환경 변수를 직접 주면 그것이 우선한다.

```powershell
New-Item -ItemType Directory -Force third_party
New-Item -ItemType Junction -Path third_party\ffmpeg -Target <ffmpeg 8 shared 개발 패키지 폴더>
New-Item -ItemType Junction -Path third_party\libclang -Target <LLVM의 bin 폴더(libclang.dll)>
```

- 실행할 때 ffmpeg DLL(`avcodec-62.dll` 등)이 PATH에 있어야 한다(ffmpeg의 `bin`을 PATH에 넣는다).
- **주의: 정션이 들어 있는 폴더(worktree, 복사본)를 `git worktree remove`나 `rm -rf`, `Remove-Item -Recurse`로 지우지 않는다.** 정션을 따라가 원본(ffmpeg, LLVM 설치 폴더)을 지운다. 먼저 `cmd /c rmdir <정션>`으로 링크만 끊고 지운다. worktree에서 작업할 때도 같은 정션을 만들어야 하므로 특히 조심한다.
- 지금 쓰는 ffmpeg 빌드(BtbN `gpl-shared`)는 GPL 구성이다. 배포 라이선스는 미결이다(`.docs/decisions.md`).

## 빌드·테스트

저장소 루트에서:

```sh
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets
```

`apps/desktop`에서 (처음에는 `npm install`):

```sh
npm run check        # svelte-check (타입 검사)
npm run build        # 프런트엔드 정적 빌드 → build/
npm run tauri dev    # 앱 실행 (vite dev 서버 1420 포트 + Rust 빌드)
npm run tauri build  # 배포용 빌드 (설치 파일 없이: npx tauri build --no-bundle)
```

전용 서버: `cargo run -p dongsi-server-app -- --data <폴더> --name <이름>`

- 앱 상태는 `%APPDATA%\com.dongsi.desktop\state.json`에 저장된다. 지우면 처음 실행한 상태로 돌아간다(설치 ID도 새로 생김).
- 내장 서버 인증서는 `%APPDATA%\com.dongsi.desktop\host\`에 있다. 지우면 지문이 바뀌어, 그 방송을 저장해 둔 시청자에게 지문 경고가 뜬다.
- 개발 빌드에서만 보이는 것: 서버 추가 대화 상자의 "예시 초대 코드 넣기", 방송 화면의 "개발용: 테스트 패턴으로 바로 시작"(테스트 패턴 + 테스트 톤), 시청 통계의 목표 지연·stream id. 시청 화면은 2초마다 통계를 앱 로그(`dongsi_desktop_lib::webview: watch ...`)에 남긴다.
- 화면을 자동으로 조작해 시험할 때는 UIA로 통계를 자주 읽지 않는다(WebView2가 느려져 측정이 틀어진다). `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`를 주고 `npm run tauri dev`로 띄우면 CDP(`http://127.0.0.1:9222/json/list`)로 페이지를 조작할 수 있다. 시험할 때 사용자 화면 대신 테스트 패턴을 쓴다.
