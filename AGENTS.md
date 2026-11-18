# AGENTS.md

이 저장소(dongsi)는 Rust로 만드는 Windows 화면 공유 앱이다. Discord의 화면 공유(Go Live)만 떼어 낸 형태로, 한 사람이 화면이나 앱을 송출하고 다른 사람들이 방송처럼 시청한다. 대화는 Discord 음성 채널에서 하므로 **스트림 오디오에 Discord 소리가 섞이면 안 된다.**

## 현재 상태

- 단계: 클라이언트 골격 완료(UI + 핵심 로직). 서버 연결 전이라 방송 목록과 로컬 네트워크 목록은 예시 데이터다. 다음 작업은 Phase 0 기술 검증이다.
- 세부 계획, 결정, 조사 내용은 `.docs/`에 있다.

## 구조

- `crates/core` (dongsi-core): UI·네트워크와 무관한 공통 로직. 서버 주소 파싱, 인증서 지문, 설치 ID·서버 ID, 초대 코드(`dongsi1:` + base64url JSON), 서버 목록
- `apps/desktop`: Tauri 2 앱
  - `src/`: SvelteKit(Svelte 5 + TypeScript) 화면. adapter-static과 해시 라우터(`#/server/<id>`)를 쓴다. 해시 라우터에서는 `ssr` 같은 페이지 옵션을 쓰면 빌드가 실패한다.
  - `src-tauri/` (dongsi-desktop): Tauri 명령(`commands.rs`), 상태 저장(`store.rs`, 앱 데이터 폴더의 `state.json`), 예시 데이터(`sample.rs`)
  - 프런트엔드 타입(`src/lib/types.ts`)은 `commands.rs`의 DTO와 손으로 맞춘다. 한쪽을 바꾸면 다른 쪽도 바꾼다.

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
- Rust stable, cargo workspace(edition 2024)를 쓴다. 새 크레이트는 `.docs/plan.md`의 구조 안을 따라 필요할 때 추가한다.
- 커밋은 작업 브랜치에 의미 단위로 한다. push는 사용자가 요청할 때만 한다.

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
npm run tauri build  # 배포용 빌드
```

- 앱 상태는 `%APPDATA%\com.dongsi.desktop\state.json`에 저장된다. 지우면 처음 실행한 상태로 돌아간다(설치 ID도 새로 생김).
- 개발 빌드에서만 서버 추가 대화 상자에 "예시 초대 코드 넣기" 단추가 보인다.
