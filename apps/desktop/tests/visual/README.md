# 화면 비교 하네스

Rust 앱(Tauri) 없이 프런트엔드를 브라우저에서 띄워, 디자인 시안(`.docs/design/canvas/b-*.dc.html`)과 **같은 상태·같은 크기**의 화면을 찍고 시안 PNG와 나란히 비교하는 도구다.

- Tauri IPC(`window.__TAURI_INTERNALS__`)를 가짜로 바꿔 서버 3개·연결 실패·지문 다름·방송 3개·방송 중 같은 상태를 재현한다.
- 설치된 Microsoft Edge를 헤드리스로 쓴다(`playwright-core`, `channel: "msedge"`). 브라우저를 따로 내려받지 않는다.
- 시안 PNG와 앱 PNG를 나란히 보여 주는 `compare.html`을 만든다.

## 쓰는 법

`apps/desktop`에서:

```sh
npm run build                                   # build/를 만든다(화면 코드를 고칠 때마다)
npm run shots -- --out <출력 폴더> --design <시안 PNG 폴더>
```

| 인자 | 뜻 |
|---|---|
| `--out <폴더>` | 결과 폴더. 기본 `tests/visual/out`(git 제외). |
| `--design <폴더>` | 시안 PNG 폴더(`<시나리오 이름>.png`). 없으면 `compare.html`에 앱 화면만 나온다. |
| `--only a,b` | 이 이름의 시나리오만 돌린다. |
| `--url <주소>` | build/를 직접 서빙하지 않고 이 주소(예: 떠 있는 vite dev 서버)를 쓴다. |
| `--headed` | 브라우저 창을 보이게 한다(디버깅). |

결과:

- `<out>/<시나리오>.png` — 시안과 같은 창 크기(기기 배율 1)의 화면. 동작이 실패해도 그 시점의 화면을 찍는다.
- `<out>/compare.html` — 시나리오마다 시안 | 앱을 나란히, 크기·성공 여부·콘솔 오류, "겹쳐 보기(차이)" 단추(두 그림을 `difference`로 겹침).
- `<out>/results.json` — 같은 내용을 기계가 읽는 형태로.
- 실행 끝에 시나리오별 성공·실패 표를 찍고, 실패가 하나라도 있으면 종료 코드 1(하네스 자체 오류는 2).

### 서빙과 포트

`--url`이 없으면 `build/`(adapter-static 결과, Tauri가 읽는 것과 같은 파일)를 작은 정적 서버로 **8150~8159 중 비어 있는 포트**에서 서빙하고 끝나면 닫는다. 1420(`tauri dev`)은 쓰지 않는다. 브라우저도 시나리오마다 새 임시 컨텍스트라 사용자 프로필을 건드리지 않는다.

### 성공·실패의 뜻

- **실패**: 동작을 못 했다(대상 요소를 5초 안에 못 찾음 — 보통 화면이 하네스 계약을 아직 안 지킨 것), 시나리오 표에 없는 명령을 앱이 불렀다, 처리 안 된 예외가 났다.
- **참고**(실패 아님): `console.error` 출력, 문서 높이가 창보다 긴 경우, 글꼴 로드 실패.
- 화면이 옛 모양이어도 하네스는 돈다. 계약 요소가 없는 시나리오만 실패하고, 그 시점의 화면은 찍힌다.

## 파일

| 파일 | 역할 |
|---|---|
| `shots.mjs` | 실행기(서버·브라우저·시나리오 실행·비교 HTML). |
| `scenarios.mjs` | 시나리오(시안 하나당 하나, 이름은 시안 파일 이름과 같다). |
| `fixtures.mjs` | 고정 데이터. 시안의 이름·주소·숫자, Rust DTO(`src/lib/types.ts`) 모양. |
| `mock-ipc.js` | 브라우저에 `addInitScript`로 넣는 가짜 Tauri IPC. |

## 하네스 계약(화면이 지켜야 하는 이름)

시나리오는 화면 구현 방식이 아니라 아래 이름에만 기댄다.

- 홈 서버 카드 요소: `data-server-id="<id>"`. 카드 안의 본문 단추·링크(누르면 연결)의 접근 이름에 서버 이름이 든다.
- 카드 ⋯ 단추: aria-label `'<서버 이름>' 서버 메뉴`. 메뉴 항목은 role `menuitem`, 글자 "이름 변경"·"삭제"(서버 화면 ⋯ 메뉴는 aria-label `서버 메뉴`, 항목 "이름 변경"·"연결 끊기"·"삭제").
- 방송 타일: `BroadcastTile`이 만든 링크(접근 이름에 방송자 이름).
- 시청 비밀번호 입력란: label `'<이름>' 방송의 비밀번호`, 확인 단추 "확인".
- 위쪽 바 + 단추 aria-label "서버 추가".
- 방송 시작 화면 최종 단추 "방송 시작", 개발용 단추 "개발용: 테스트 패턴으로 바로 시작".

## 시나리오 더하는 법

`scenarios.mjs`의 `createScenarios` 배열에 `s("이름", { ... })`를 더한다. 이름은 시안 파일 이름(`b-xxx`)과 같게 하면 `compare.html`이 시안 PNG(`<design>/<이름>.png`)와 짝지어 준다.

```js
s("b-server-empty", {
  note: "회사 테일넷 서버 화면: 방송 없음",
  viewport: { width: 1100, height: 720 },        // 시안과 같은 창 크기(기본 1100×720)
  hash: "#/server/" + corp.id,                    // 시작 해시
  connected: [home, minsu, corp],                 // connected_servers가 돌려줄 서버(= 탭)
  mock: { connect_server: value(...) },           // 기본 표(baseTable) 위에 덮어쓸 명령
  state: { ... },                                 // fn 항목이 쓰는 가변 상태
  steps: [ { waitFor: { text: "진행 중인 방송이 없습니다" } } ],
})
```

### mock 표(명령 이름 → 항목)

| 항목 | 만드는 함수 | 뜻 |
|---|---|---|
| 태그 없는 값 | — | 그 값으로 성공 |
| `value(v, delay?)` | | `v`로 성공(delay ms 뒤) |
| `reject(v, delay?)` | | `v`로 거절. Rust `CmdError` 모양 `{ message, code?, detail? }` |
| `pending()` | | 영원히 대기(연결하는 중 상태) |
| `fn((args, ctx, data) => 값, data?)` | | 함수. `throw`하면 거절. `ctx.state`(가변), `ctx.emit(이벤트, 페이로드)`, `ctx.calls`. **함수는 소스로 직렬화되어 브라우저에서 돌므로 바깥 변수를 쓸 수 없다** — 필요한 값은 `data`로 넘긴다. |

표에 없는 명령을 앱이 부르면 콘솔 경고(`[mock] 처리하지 않은 명령`) + 거절하고, 그 시나리오는 실패로 표시된다(표에 명령을 더하라는 뜻). 기본 표(`baseTable`)에는 `get_settings`, `list_servers`, `scan_local`, `connected_servers`, `disconnect_server`, `connect_server`, `broadcast_status`, `list_capture_sources`, `capture_source_thumbnails`, `available_encoders`, `quality_presets`, `stop_watch`, `request_keyframe`가 있고, `plugin:app|version`(0.1.0)·`dev_log`는 내장이다. `add_server`·`rename_server`·`watch_broadcast` 등은 시나리오가 직접 준다.

### 동작(steps)

`{ waitFor: 대상 }`, `{ click: 대상 }`, `{ fill: 대상, value }`, `{ hover: 대상 }`, `{ press: "Enter" }`, `{ wait: ms }`, `{ emit: [이벤트, 페이로드] }`(= `window.__mockEmit`), `{ hash: "#/..." }`, `{ eval: "js 소스" }`.

대상: `{ role, name, exact? }`, `{ label }`, `{ text, exact? }`, `{ css }`, `{ card: id }`, `{ cardButton: id, name }`, `{ cardMenu: 이름 }`, `{ menuitem: 글자 }`, `{ tile: 방송자 이름 }`, `{ or: [대상, 대상] }`. 자세한 뜻은 `scenarios.mjs` 머리말.

맨 앞에 `waitFor`를 두어 화면이 뜬 것을 확인하고, 끝에 찍는다. 찍기 직전에 글꼴(`document.fonts.ready`)과 두 프레임을 기다리고, CSS 애니메이션은 끈다(스피너는 처음 모양).

## 하네스가 고정하는 것

- **시각**: `Date`를 실행 시각으로 멈춘다(타이머는 그대로). 방송 시작 시각을 "지금 − N초"로 만들어 방송 시간이 항상 "30분", "1시간 30분", "1분", "12분", 위쪽 바 "12:34"로 찍힌다.
- **재생 가능 코덱**: `RTCRtpReceiver.getCapabilities("video")`를 H.264·VP8·VP9·AV1만 돌려주게 바꾼다(시안의 "H.265 재생 불가").
- **창 레이블**: `getCurrentWindow().label` = `main`. `isTauri` = true. `import.meta.env.DEV`는 build/에서 false라 개발용 단추는 안 보인다.
- 지문 `short`는 Rust(`Fingerprint::short`)처럼 앞 8바이트(`3A:9F:12:C4:7B:E0:55:D1`)다. 시안의 카드는 "3A:9F:12:C4…"로 줄여 보이므로 화면에서 자른다.
- 서연 방송자 이름은 시안대로 "서연 · 엘든 링 보스전"(이름이 긴 경우를 보여 주는 시안이라 `broadcasterName`에 그대로 넣었다).

## 한계

- **재생 화면은 없다.** 실제 WebRTC 미디어가 필요하므로 `b-watch`, `b-watch-popout`은 하네스에서 뺐다. **실제 앱에서 찍는다**(`npm run tauri dev` + CDP, 테스트 패턴). 시청의 상태 화면(비밀번호·연결하는 중·끝남·오류)은 `watch_broadcast`를 가짜로 만들어 재현한다. 연결하는 중 상태는 `RTCPeerConnection`은 진짜(offer만 만들고 끝)이고, 연결 성공·통계 패널은 재현하지 않는다.
- Rust 쪽 동작(실제 연결·캡처·인코딩·UPnP)은 확인하지 않는다. 가짜 IPC가 DTO 모양대로 값을 줄 뿐이라, DTO가 바뀌면 `fixtures.mjs`를 `src/lib/types.ts`와 손으로 맞춰야 한다.
- 이벤트는 `window.__mockEmit`으로 시나리오가 직접 쏜 것만 온다(`broadcast-stopped`, `server-changed` 같은 주기 이벤트는 없다). 찍는 순간 `scan_local`·`broadcast_status` 같은 폴링은 항상 같은 값을 돌려준다.
- 시안은 헤드리스 Edge로 찍은 PNG다. 다른 글꼴·OS의 Edge로 돌리면 글자 가장자리가 조금 달라질 수 있다. 완전히 같은 픽셀이 아니라 배치·수치를 보는 용도다.
