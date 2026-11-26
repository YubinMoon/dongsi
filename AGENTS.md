# AGENTS.md

dongsi는 Rust + Tauri 2로 만드는 Windows 11 화면 공유 앱이다. 한 사람이 화면이나 앱을 송출하고 여럿이 시청한다. 대화는 Discord 음성 채널에서 하므로 **스트림 오디오에 Discord 소리가 섞이지 않게 빼는 것**이 핵심이다(마이크 미포함, Discord 연동·ToS 위반 방식 금지).

## 구조

| 경로 | 내용 |
|---|---|
| `crates/core` | 공통 순수 로직: 서버 주소, 인증서 지문, 설치·서버 ID, 초대 코드(`dongsi1:` + base64url JSON), 시그널링(`proto.rs`), 로컬 감지 패킷, 화질 설정(`media.rs`, fps 30·60), 스레드 우선순위·타이머(`priority.rs`) |
| `crates/media` | WGC 화면·창 캡처, 테스트 패턴, 미리 보기·썸네일 JPEG, WASAPI 루프백(Discord 제외), Opus, libavcodec(FFmpeg 8) 하드웨어 H.264 인코딩(NVENC → AMF → QSV, `encoder.rs`), GPU 변환(D3D11 Video Processor `convert.rs`), NVENC zero-copy(`ff.rs`), GPU 테스트 패턴(`gpu_pattern.rs`). 단독 시험 예제 `record` |
| `crates/net` | TLS 지문 확인, 시그널링·감지 클라이언트, UPnP, WebRTC 송출(`str0m`, BWE), 목표 비트레이트(`rate.rs`, TWCC 추정) |
| `crates/server` | 전용·내장 공통 서버 코어: 시그널링, 방송 목록, 비밀번호, SFU(`str0m`, 방송당 단일 비디오 트랙 전달), 로컬 감지 응답, 썸네일, 지표. `tests/e2e/`가 서버 E2E |
| `apps/server` | 전용 서버 실행 파일 `dongsi-server` (설정 우선순위: CLI > `DONGSI_*` > `dongsi-server.toml` > 기본값, `--metrics-addr` 기본 꺼짐) |
| `apps/desktop` | Tauri 2 앱. `src/`는 SvelteKit(Svelte 5 + TypeScript, adapter-static, 해시 라우터), `src-tauri/`는 Rust 백엔드(`hub.rs`, `live.rs`, `viewing.rs`, `broadcast.rs`, `embedded.rs`, `store.rs`). `tests/visual/` 화면 비교 하네스, `e2e/` 실제 앱 CDP 하네스 |
| `Dockerfile`, `deploy/` | 리눅스 x64 전용 서버 이미지와 compose (`deploy/README.md`) |
| `scripts/` | `test-full.ps1` 전체 시험, `e2e-linux-server.ps1` 리눅스 서버 E2E, `ffmpeg/` LGPL FFmpeg 빌드, `licenses/` 제3자 고지 생성기, `package-installer.ps1`·`package-portable.ps1` 배포물(`lib/release.ps1`) |
| `LICENSE`, `THIRD_PARTY_NOTICES.md`, `docs/licensing.md` | 라이선스와 릴리스 절차 (`docs/licensing.md` 5절) |

- 프런트엔드 타입(`apps/desktop/src/lib/types.ts`)은 Rust DTO(`commands.rs`, `live.rs`, `viewing.rs`, `broadcast.rs`, `error.rs`)와 수동으로 맞춘다.
- 해시 라우터이므로 SvelteKit 페이지에 `ssr` 같은 옵션을 쓰면 빌드가 실패한다.

## 작업 규칙

- 설명과 문서는 한국어로 쓰고, 코드 식별자와 기술 용어는 원문을 쓴다.
- 아직 정식 릴리스 전이다. 이전 버전 호환이나 마이그레이션은 신경 쓰지 않는다. 시그널링을 호환되지 않게 바꾸면 `PROTOCOL_VERSION`(`crates/core/src/proto.rs`)만 올린다.
- Rust stable, cargo workspace(edition 2024).
- 라이선스: 우리 코드는 MIT, FFmpeg는 LGPL 빌드(`--enable-gpl`·`--enable-nonfree` 없음, DLL 링크). GPL·AGPL 의존성과 GPL 전용 FFmpeg 부품(`libx264` 등)은 넣지 않는다.
- `Cargo.lock`, `apps/desktop/package-lock.json`, 화면에 쓰는 npm 패키지, FFmpeg 빌드가 바뀌면 `node scripts/licenses/generate.mjs --ffmpeg <FFmpeg 폴더>`로 `THIRD_PARTY_NOTICES.md`를 갱신해 같이 커밋한다(`--check`로 검증).
- 커밋은 작업 브랜치에 의미 단위로 한다. `push`는 사용자가 요청할 때만 한다.

## 빌드 환경

`dongsi-media`는 FFmpeg 8 **LGPL** 개발 패키지(`include`, `lib`, `bin` DLL)와 bindgen용 `libclang`이 필요하다. `third_party/`(git 제외)에 정션을 두면 `.cargo/config.toml`이 `FFMPEG_DIR`·`LIBCLANG_PATH`로 사용한다(환경 변수를 직접 주면 우선).

```powershell
New-Item -ItemType Directory -Force third_party
New-Item -ItemType Junction -Path third_party\ffmpeg -Target <ffmpeg LGPL 빌드 결과 폴더>
New-Item -ItemType Junction -Path third_party\libclang -Target <LLVM의 bin 폴더(libclang.dll)>
```

- 실행 시 FFmpeg `bin`(`avcodec-62.dll` 등)이 `PATH`에 있어야 한다.
- **정션이 든 폴더(worktree, 복사본)를 `git worktree remove`·`rm -rf`·`Remove-Item -Recurse`로 지우지 않는다.** 정션을 따라가 원본(FFmpeg, LLVM)을 지운다. 먼저 `cmd /c rmdir <정션>`으로 링크만 끊는다.
- `target/`은 저장소(worktree는 그 worktree)의 기본 위치를 쓴다. `CARGO_TARGET_DIR`로 다른 드라이브에 두지 않는다.
- 배포물은 반드시 LGPL FFmpeg로 만든다: `scripts/package-installer.ps1 -FfmpegDir <LGPL 폴더>` 또는 `scripts/package-portable.ps1 -FfmpegDir <LGPL 폴더>`. `tauri.conf.json`은 `bundle.active = false`라 `tauri build`만으로는 설치 파일이 생기지 않는다.

## 시험

- 수정 중에는 빠른 시험과 고친 영역의 대상 시험을 돌리고, 커밋·마무리 직전에 전체 시험(`scripts/test-full.ps1`)을 돌린다.
- **창이나 소리가 나는 시험**(`npm run e2e`, `npm run tauri dev`, `test-full.ps1 -App`, 창 띄우는 `--ignored` 시험)은 실행 전 사용자에게 먼저 알린다.
- 실제 화면 캡처, LAN 포트, 외부 서버, 창·소리 등을 쓰거나 오래 걸리는 새 시험은 `full-tests` feature 뒤에 둔다(`Cargo.toml`의 `required-features = ["full-tests"]` 또는 `#[cfg_attr(not(feature = "full-tests"), ignore = "...")]`).

### 빠른 시험

| 명령 | 위치 |
|---|---|
| `cargo test --workspace` / `cargo test -p <crate> --lib` | 루트 |
| `cargo fmt --all` / `cargo clippy --workspace --all-targets` | 루트 |
| `npm test` (vitest) / `npm run check` (svelte-check) | `apps/desktop` |

### 대상 시험 (`--features full-tests`)

| 고친 곳 | 명령 | 주의 |
|---|---|---|
| 서버 E2E | `cargo test -p dongsi-server --features full-tests --test e2e -- <이름>` | |
| 내장 서버·LAN 방송·감지 | `cargo test -p dongsi-desktop --features full-tests --test lan_broadcast -- <이름>` | LAN 8200~8299 |
| 미디어 캡처·인코딩·변환·썸네일 | `cargo test -p dongsi-media --features full-tests --test source_fps` / `switch_source` / `bitrate_cap` / `zero_copy` / `--lib thumbnail` | 실제 화면 캡처(창 없음) |
| 단독 녹화 시험 | `cargo run -p dongsi-media --example record -- --source test --secs 3 --out target/out` | |
| 창 닫힘·롤 창 따라가기 | `cargo test -p dongsi-media --features full-tests --test switch_source -- --ignored`, `cargo test -p dongsi-desktop --features full-tests --test follow_lol -- --ignored --test-threads 1` | **창이 뜸 (사전 알림)** |
| 화면 비교 | `apps/desktop`에서 `npm run build` 뒤 `npm run shots -- --only <시나리오>` | 헤드리스 Edge |
| CDP 하네스 도우미 | `apps/desktop`에서 `node --test e2e/lib/<파일>.test.mjs` | |
| 리눅스 서버·Docker | `scripts/e2e-linux-server.ps1 -Modes bridge -Clients windows -SkipDestructive` | WSL Docker, 커밋된 HEAD |

### 전체 시험

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-full.ps1        # fmt·clippy·cargo test-full·화면
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-full.ps1 -App   # + 실제 앱 CDP 하네스 (창 뜸, 사전 알림)
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-full.ps1 -All   # + CDP 하네스 + 리눅스 서버 E2E
```

- `cargo test-full`은 `cargo test --workspace --features full-tests --no-fail-fast` alias다(`.cargo/config.toml`).
- 결과 요약은 `target/test-full/<시각>/summary.md`에 남는다. `-Skip shots,build`로 단계를 뺄 수 있다.

## 실행과 환경 변수

- **앱 개발 실행**: `apps/desktop`에서 `npm run tauri dev` (창 뜸, vite 1420 고정).
- **전용 서버**: `cargo run -p dongsi-server-app -- --data <폴더> --name <이름>` (Docker는 `deploy/README.md`).
- **CDP 통합 E2E**: 루트에서 `cargo build -p dongsi-desktop` 뒤 `apps/desktop`에서 `npm run e2e -- --scenario all` (vite 1420 고정이라 동시 실행 금지, 자세한 내용은 `apps/desktop/e2e/README.md`).
- **앱 상태 경로**: `%APPDATA%\com.dongsi.desktop\state.json` (내장 서버 인증서는 같은 폴더 `host\`).
- **주요 환경 변수**:
  - `DONGSI_PROFILE=<이름>`: 앱·WebView2 데이터를 `profiles\<이름>\`로 분리(한 PC에서 앱 여러 개 실행).
  - `DONGSI_PORT=<포트>`: 내장 서버·로컬 감지 포트 변경(기본 7884).
  - `DONGSI_MEDIA_NO_ZEROCOPY=1`: NVENC에서도 zero-copy 대신 GPU 변환 + 읽어 오기 경로 사용.
