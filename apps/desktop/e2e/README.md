# e2e: 실제 앱 두 개를 CDP로 조작하는 통합 시험

실제 dongsi 데스크톱 앱(Tauri 2 + WebView2) **두 개**를 띄우고 CDP(Playwright `connectOverCDP`)로 조작해서 화면·통신·재생 지표를 확인한다.
화면 시안 23장의 스크린샷도 같이 찍는다. 화면 소스는 **테스트 패턴**, 소리는 **테스트 톤**만 쓴다(사용자 화면·창 캡처 금지).

## 준비

1. `apps/desktop`에서 `npm install` (`playwright-core`와 vite가 들어 있다).
2. 저장소 루트에서 앱 exe를 만든다. `cargo build`로 만든 디버그 exe는 `localhost:1420`(vite dev 서버)을 불러오므로 개발용 단추(테스트 패턴·테스트 톤)가 보인다. 하네스가 1420이 비어 있으면 vite를 직접 띄우고 끝나면 끈다.
   ```powershell
   $env:FFMPEG_DIR = "D:\dev\ffmpeg"; $env:LIBCLANG_PATH = "D:\dev\clang+llvm\bin"
   $env:CARGO_TARGET_DIR = "D:\dongsi-target\t4-harness"   # 선택: C: 디스크를 아끼려고 target을 D:에 둘 때
   cargo build -p dongsi-desktop        # <target>/debug/dongsi-desktop.exe
   ```
   하네스는 `CARGO_TARGET_DIR`이 있으면 `<CARGO_TARGET_DIR>\debug\dongsi-desktop.exe`, 없으면 `<저장소>\target\debug\dongsi-desktop.exe`를 쓴다(`--exe`로 직접 줄 수도 있다). 하네스를 돌리는 셸에도 같은 `CARGO_TARGET_DIR`를 준다.
3. 실행할 때 ffmpeg DLL 폴더(기본 `D:\dev\ffmpeg\bin`, 환경 변수 `DONGSI_FFMPEG_BIN`로 바꿈)를 하네스가 PATH 앞에 붙여 준다.
4. 시나리오 b는 WSL에 Docker와 이미지 `dongsi-server:e2e`가 있어야 한다(없으면 b는 실패로 기록되고 a·shots는 계속 돈다).

## 실행

```sh
cd apps/desktop
npm run e2e -- --scenario all                 # a, b, shots 모두 (약 4~5분)
node e2e/run.mjs --scenario a                 # 클라-클라(로컬 랜 방송)
node e2e/run.mjs --scenario b --server-addr lan
node e2e/run.mjs --scenario shots --shots C:\경로\ui-shots\final
```

| 옵션 | 뜻 |
|---|---|
| `--scenario a\|b\|shots\|all` | 시나리오(쉼표로 여러 개). 기본 all |
| `--out <폴더>` | 결과 폴더. 기본 `e2e/out/<시각>/`(git 제외) |
| `--shots <폴더>` | 스크린샷 폴더. 기본 `<out>/shots`. 시안 이름 `b-*.png`로 저장 |
| `--image <이름>` | 서버 Docker 이미지. 기본 `dongsi-server:e2e` |
| `--server-addr loopback\|lan` | 시나리오 b의 서버 주소: 127.0.0.1(기본) 또는 이 PC의 LAN IPv4 |
| `--keep` | 끝난 뒤 접두사(기본 `t4-`)로 시작하는 컨테이너·볼륨을 지우지 않음 |
| `--exe <경로>` | 앱 exe 경로. 기본 `<CARGO_TARGET_DIR 또는 저장소 target>/debug/dongsi-desktop.exe` |

| `--profiles <a>,<b>` | 앱 두 개의 `DONGSI_PROFILE`. 기본 `t4a,t4b` (`DONGSI_E2E_PROFILES`) |
| `--cdp <a>,<b>` | 앱 두 개의 CDP 포트. 기본 `9225,9226` (`DONGSI_E2E_CDP`) |
| `--port-base <n>` | 포트 기준값. 기본 `8400` (`DONGSI_E2E_PORT_BASE`). 아래 "포트" 참고 |
| `--prefix <p>` | Docker 컨테이너·볼륨 이름 접두사. 기본 `t4-` (`DONGSI_E2E_PREFIX`) |

값을 정하는 순서는 인자 > 환경 변수 > 기본값이다. 잘못된 값(아래 "값 규칙")은 앱·vite·Docker를 건드리기 전에 오류(종료 코드 2)로 끝난다. 시작할 때 실제로 쓰는 값(프로필, CDP, 포트, 접두사, exe, vite)을 출력하고 `summary.md`·`results.json` 머리에도 적는다.

종료 코드는 실패한 검사가 하나라도 있으면 1이다. 검사가 실패해도 다음 단계로 넘어가고, 앞 단계에 기대는 단계만 "건너뜀"으로 적는다.

## 결과 파일

- `summary.md`: 검사마다 이름·통과/실패/경고/건너뜀·값, 시청 지표 표, 스크린샷 표(못 찍은 것 포함)
- `results.json`: 같은 내용(기계가 읽는 용)
- `logs/<시나리오>-<A|B>.{stdout,stderr}.log`: 앱 로그, `logs/vite.*.log`
- `failures/`: 실패한 단계의 화면(모든 창)
- `shots/`: 스크린샷(1100×720, 방송 시작 1100×860, 방송 중 1100×900, 포트 안내 1100×1000, 분리 창 1280×760. CSS 픽셀)

## 시나리오

- **a** (클라-클라): 로컬 랜 방송 → 감지 타일 → 시청 → 통계 → 시청자 수 → 분리 창 → 소리 막힘(needsUnmute) 재현 시도 → 방송 끝 → 비밀번호 방송(오답·정답·기억) → 저장 서버·"비밀번호 기억됨" → 내장 서버 닫힘 → 포트 대체 안내
- **b** (클라-서버): WSL Docker 서버 `<접두사>srv`(기본 `t4-srv`, 포트 8460) → 초대 코드로 서버 추가 → 연결 → 서버로 방송 → 시청 → 방송 중 화질·코덱 변경(1080p30 VP8, 720p30 VP9) → 방송 끝 → 서버 정지(연결 끊김 화면) → 재시작·다시 연결 → 다른 인증서의 `<접두사>srv2`로 지문 경고
- **shots**: 홈 카드·메뉴·이름 변경·삭제 대화 상자·연결 중(응답 없는 서버, 기본 8470)·연결 실패(기본 8471)·시청 연결 중·시청 오류·설정

## 시나리오 av: A/V 차이·화면 전환 실측

`--scenario av`로만 돈다(`all`에 들어 있지 않다). 앱 두 개(방송 A, 시청 B)에서 **화면과 소리가 실제로 나온 시각**을 재서 코덱별 A/V 차이와 화질·코덱 전환 때의 회복을 표로 낸다. 고치는 도구가 아니라 측정 도구다. 소스는 테스트 패턴·테스트 톤뿐이다.

```sh
node e2e/run.mjs --scenario av --av-targets lan,server --profiles t6a,t6b --cdp 9229,9230 --port-base 8600 --prefix t6-
```

| 옵션(환경 변수 `DONGSI_E2E_AV_*`) | 기본 | 설명 |
|---|---|---|
| `--av-targets lan,server` | lan | 로컬 랜 방송 / WSL Docker 전용 서버(시나리오 b와 같이 띄움, `--image`·`--server-addr` 적용) |
| `--av-codecs h264,h264sw,vp8,vp9,av1` | 전부 | 못 고르는 코덱은 "건너뜀". `h264sw`는 A를 `DONGSI_MEDIA_ENCODER=libopenh264`(없으면 libx264)로 다시 띄운다 |
| `--av-preset "720p 30fps"` | 720p 30fps | 코덱별 측정의 방송 화질 |
| `--av-secs 25` | 25 | 코덱마다 기록하는 시간(시청 첫 프레임 기준, 8~600) |
| `--av-switches 8` | 8 | 대상마다 화질·코덱 전환 횟수(7초 간격, 0이면 건너뜀) |
| `--av-trace` | 끔 | A를 `DONGSI_MEDIA_TRACE=<out>/trace/<대상>-<코덱>-<팔>-r<반복>`으로 띄운다(송신 쪽 프레임 기록, 실행마다 폴더가 달라 A를 매번 다시 띄운다. 요약은 `node e2e/lib/trace-summary.mjs <폴더>`). 끊김 목록의 "송신" 칸도 이 기록으로 채운다 |
| `--av-arms on,off,stress` (`DONGSI_E2E_AV_ARMS`) | on | 비교 팔. `on` = 립싱크 보정 켬(`localStorage "dongsi.lipsync"` 지움), `off` = 끔(`"off"`), `stress` = 끔 + 시청 비디오 수신기 `jitterBufferTarget`을 주기적으로 바꿈. 쉼표로 여럿 |
| `--av-repeat <n>` (`DONGSI_E2E_AV_REPEAT`) | 1 | 1~20. (대상 × 코덱 × 팔) 묶음을 되풀이하는 횟수. 실행마다 새 방송·새 시청 |
| `--av-stress-period <ms>` (`DONGSI_E2E_AV_STRESS_PERIOD`) | 2000 | stress 팔에서 `jitterBufferTarget`을 바꾸는 주기(500~10000). 설치 2초 뒤부터 |
| `--av-stress-ms <ms>` (`DONGSI_E2E_AV_STRESS_MS`) | 150 | stress 팔에서 올리는 값(20~1000). 이 값과 0을 번갈아 준다 |
| 환경 변수 `DONGSI_E2E_AUDIBLE=1` | 끔 | 하네스가 띄운 앱은 기본으로 `--mute-audio`라 스피커로 소리가 나가지 않는다(테스트 톤 삐가 안 들림). 이때 루프백(L) 측정은 건너뛰고 WebAudio(W)·EPT(E)만 잰다. 실제 출력을 재려면 이 값을 준다(삐가 들린다) |
| 환경 변수 `DONGSI_E2E_AV_LIPSYNC=off` | 켬 | 호환용. `--av-arms`·`DONGSI_E2E_AV_ARMS`가 없을 때 기본 팔을 `off`로 바꾼다(시청 페이지의 립싱크 보정을 끄고 잰다) |

**측정 방법**(`lib/sync-probe.mjs` 머리말, 앱 쪽 `src-tauri/src/dev_probe.rs`·`crates/media/src/probe.rs`)
- 테스트 패턴은 매초 정초에 흰 프레임, 테스트 톤은 같은 정초에 1kHz 100ms 삐를 낸다. 캡처 시각이 정초이므로 *표시/출력 시각 − 정초*가 곧 그 경로의 지연이다.
- 화면: 시청 페이지의 `requestVideoFrameCallback`에서 흰 프레임을 찾고 `expectedDisplayTime`을 쓴다.
- 소리 세 가지: (L) WASAPI 프로세스 루프백으로 **실제 출력**의 삐 시작을 QPC로, (W) WebAudio AudioWorklet로 삐 시작을 잡고 `getOutputTimestamp`로 출력 시각 추정, (E) 앱 통계와 같은 `estimatedPlayoutTimestamp`(EPT) 차이.
- 시계: 페이지 `performance.now`와 QPC를 `dev_clock` 17번 중 왕복이 가장 짧은 것으로 맞춘다(시작·끝 두 번으로 표류도 확인).
- **부호**: A/V = 소리 시각 − 화면 시각. 양수 = 비디오가 앞섬(앱 통계 패널과 같다).
- 구간: 0~3, 3~6, 6~10, 10~20, 20초~(시청 첫 프레임 기준) 각각 평균과 최대|값|.
- 루프백 대상: 설정 단계에서 B 가 440Hz 를 내 보며 "앱 프로세스 트리 → 각 자식 프로세스 → 기본 출력 장치 전체" 순서로 소리가 잡히는 대상을 고른다(결과가 summary.md 에 남는다). 개발 빌드 전용 명령(`dev_clock`, `dev_tone_probe_*`)을 쓴다.

**전환 시험**: 같은 시청 세션에서 A 의 화질·코덱을 7초 간격으로 바꾸고(720p H.264 → 1080p VP8 → 720p H.264 → 1080p VP9 → 1080p60 H.264 …) 클릭→상태 반영, 클릭→새 화면, 최대 화면 공백, 초당 프레임 수, 키프레임·PLI·freeze·손실 증가, 회복 시간(표시 프레임 간격이 1초 동안 계속 2×프레임 간격 이하가 된 처음 순간)을 잰다.

**합격선**(`scenarios/av.mjs` 맨 위 `LIMITS` 한곳): 측정이 실제로 잡혔는지(구간마다 흰 프레임·삐 개수)는 통과/실패, `|A/V| ≤ 45ms`(3초부터)와 회복 ≤ 2초는 **경고만** 한다.

**실행 순서**: 대상마다 `for 반복 1..n { for 코덱(h264sw 제외) { for 팔 in rotate(팔 목록, 반복−1) { 실행 } } }`, 그 뒤 전환 시험(대상마다 한 번, 팔은 목록의 첫 값), 마지막에 h264sw를 같은 반복·팔 규칙으로. 팔 순서를 반복마다 한 칸씩 돌린다(반복 1: on,off,stress / 반복 2: off,stress,on …). 켬/끔을 번갈아 돌려 같은 조건(같은 PC 부하·시각)에서 비교하려는 것이다. 한 실행 = 새 방송 + 새 시청 + `--av-secs`초 기록.

```sh
# 로컬 랜·Docker 서버에서 켬/끔/stress 를 두 번씩 번갈아(전환 시험은 건너뜀)
node e2e/run.mjs --scenario av --av-targets lan,server --av-arms on,off,stress --av-repeat 2 --av-switches 0 --profiles t7a,t7b --cdp 9231,9232 --port-base 8700 --prefix t7-
```

결과: `summary.md`의 표와 원자료 `av.json`.

- **코덱별 A/V 표**: 위 표에 `팔`, `반복` 칸이 붙는다.
- **끊김 비교**(대상 × 코덱 × 팔): 실행 수, 동시 끊김 사건 수, 동시 끊김이 한 번 이상인 실행 수, 사건 길이(비디오 공백ms/오디오 은닉ms), 비디오만·오디오만 사건 수, CPU 평균(첫 프레임 뒤 시청 구간), 실행 시작 때 빌드 프로세스(rustc·cargo·link·cl·clang)가 있던 실행 수. 아래에 팔별 합계와 **Fisher 정확 검정**(동시 끊김이 있는 실행 수 기준, on 대 off와 stress 대 off, 양측 p와 단측 p(앞 팔이 더 많음)). 동시 끊김은 `info`로만 남기고 실패·경고로 세지 않는다(|A/V| 경고도 지금처럼 경고뿐).
- **끊김 목록**: 사건마다 실행, 종류, t(첫 프레임 뒤 초), 비디오 공백·오디오 은닉 ms, 그 시각 립싱크 보정값, 마지막 바닥 변경(`applied`) 뒤 ms, 마지막 jbt 변경 뒤 ms, 사건 끝 뒤 3초 NetEq 목표/지연 최대 ms, CPU %, (`--av-trace`) 송신 큐 지연 최대·캡처 간격 최대 ms.
- **보정 지나침**: 실행마다 EPT 차이 E의 (a) 3초 뒤 최대 |E|, (b) 끊김(동시·오디오) 사건 앞 0.5초~뒤 12초를 뺀 3초 뒤 최대 |E|, (c) 끊김 사건 뒤 12초 안 E의 최소·최대.
- **첫 프레임 분해**: 클릭 → 처음 만든 `RTCPeerConnection`(생성자 시간 포함) → `setLocalDescription` → `setRemoteDescription`(answer 적용; 서버 연결·시청 요청·왕복이 들어 있다) → `connected` → 첫 프레임, 합계(ms). 시각은 `GUARD_SCRIPT`가 `window.__t7pc`에 남긴다(동작은 바꾸지 않는다).
- `av.json`의 `runs[]`에는 `arm`, `rep`, `dropouts`, `cpu`(1초 표본 `samples`과 평균·최대), `heavy`, `firstFrame`, `lipsync`(`window.__dongsiLipsync` 복사, 모르는 필드 포함), `jbt`, `overshoot`, 원자료 `raw`가 들어 있다.

**끊김 검출 기준**(`lib/sync-probe.mjs`의 `detectDropouts`, 순수 함수. 시각 축은 페이지 `performance.now()`)
- 비디오 공백: rVFC `expectedDisplayTime` 간격 ≥ 150ms(`gapMs`), 첫 프레임 뒤 1.0초(`skipSec`) 이후 시작한 것.
- 오디오 은닉: 500ms 통계 사이 `concealedSamples` 증가 ≥ 960(`concealMinSamples`, 48kHz에서 20ms). ms 환산은 `totalSamplesReceived`÷`totalSamplesDuration`(없으면 48000Hz). 구간 가운데가 `skipSec` 이후인 것.
- 분류: 비디오 공백 구간과 은닉 통계 구간을 각각 ±250ms(`overlapMs`) 넓혀 겹치면 동시 끊김(`av`), 아니면 `video`/`audio`. 은닉 하나는 한 번만 짝짓고, 겹치는 공백이 여럿이면 가운데가 가장 가까운 하나만 `av`, 나머지는 `video`.
- 통계 간격은 500ms 고정(바꾸면 측정 부하가 달라져 팔 비교가 깨진다). 통계에는 오디오 `totalSamplesDuration`·`concealmentEvents`·`silentConcealedSamples`·`lastPacketReceivedTimestamp`, 비디오 `lastPacketReceivedTimestamp`, `media-playout` 통계(`stats[].p`)도 들어 있다.
- CPU: 실행 동안 `os.cpus()` 시간 합으로 1초마다 전체 사용률, 실행 시작 때 `tasklist`로 빌드 프로세스 개수.
- 순수 함수 시험: `node --test e2e/lib/sync-probe.test.mjs e2e/lib/av-stats.test.mjs e2e/lib/cpu-load.test.mjs`(앱 불필요).

## 시청 지표 읽는 법

시청 화면의 `RTCPeerConnection.getStats()`를 1초에 한 번 읽어(10초) 합친다.

- 합격 기준(실패 판정): 3초에 `framesDecoded` 30 이상 증가, 해상도가 방송자(`broadcast_status`)와 같음, 오디오 `packetsReceived` 증가, `totalAudioEnergy` > 0(테스트 톤).
- **A/V 차이 = 비디오 `estimatedPlayoutTimestamp` − 오디오 `estimatedPlayoutTimestamp`**(앱 통계 패널·`watchStats.ts`와 같은 부호. 양수면 비디오가 앞섬). 지시서의 "오디오 − 비디오"와 부호가 반대다.
- 지연 = 통계 시각 − 비디오 `estimatedPlayoutTimestamp`(NTP→유닉스 보정). 두 앱이 같은 PC라 시계가 같다.
- |A/V 평균| > 100ms, 지연 평균 > 500ms는 "경고"(실패로 세지 않음). 값은 매번 조금씩 다르다(특히 방송을 처음 보는 직후).

## 안전장치

- 모든 페이지에서 Tauri `invoke`를 거치는 길을 감싼다. `invoke`는 쓰기 불가라, vite가 내려 주는 `@tauri-apps/api` 모듈의 `invoke()`를 `context.route`로 패치해 `window.__t4invoke`를 거치게 한다.
  `list_capture_sources`는 테스트 패턴만 남기고, `start_broadcast`는 소스가 테스트 패턴·소리가 테스트 톤이 아니면 Rust까지 보내지 않고 오류로 막는다.
- 앱을 띄울 때마다 자체 시험을 한다(막히는지, 목록에 테스트 패턴만 남는지). 통과하지 못하면 그 앱으로 아무것도 하지 않는다.
- 방송 시작 화면의 선택(소스·호스팅·화질·코덱·소리·비밀번호)을 화면에서 읽어 맞는지 확인한 뒤에만 "방송 시작"을 누른다. "PC 직접 호스팅(UPnP)"은 쓰지 않는다.
- 앱 데이터: `DONGSI_PROFILE=<프로필 이름>`(기본 `t4a`·`t4b`)으로 `%APPDATA%\com.dongsi.desktop\profiles\<이름>\`만 쓰고, 시작할 때 그 안의 `state.json` 파일 하나만 지운다. 경로가 정확히 `...\profiles\<이름>\state.json` 꼴일 때만 지우며(이름은 정규식 이스케이프해서 검사), 프로필 없이 실제 사용자 데이터를 쓰는 길은 없다(빈 이름은 오류). 사용자 앱 데이터는 건드리지 않는다.
- 프로세스: 하네스가 띄운 PID만 `taskkill /T /F /PID`로 끈다. 비정상 종료(Ctrl+C 포함)에도 정리한다.
- Docker: 이름이 접두사(기본 `t4-`)로 시작하는 컨테이너·볼륨만 만들고 지운다. 그 밖의 이름은 `dockerCmd`가 거부한다.

## 포트

포트는 `--port-base <n>`(기본 8400)에서 계산한다. 기본값의 배치는 다음과 같다.

| 용도 | 계산 | 기본값 |
|---|---|---|
| 앱 두 개(내장 서버·감지 질의, `DONGSI_PORT`) | n+50 | 8450 |
| Docker 서버(`<접두사>srv`, `srv2`는 정지 후 같은 포트) | n+60 | 8460 |
| 응답 없는 TCP 서버(shots) | n+70 | 8470 |
| 아무도 안 듣는 포트(shots) | n+71 | 8471 |
| 서버 카드 하나용(shots) | n+72 | 8472 |
| CDP | `--cdp` | 앱 A 9225, 앱 B 9226 |
| vite dev 서버 | 고정 | 1420 |

7884(기본 포트)는 쓰지 않는다. 시나리오 a의 마지막 단계는 앱 포트(기본 8450) TCP·UDP를 일부러 점유해서 포트 대체를 시험한다.

## 값 규칙

- 프로필: `^[A-Za-z0-9_-]{1,64}$`, 둘이 달라야 한다.
- CDP: 1024~65535, 둘이 달라야 하고 계산한 포트·1420과 겹치면 안 된다.
- 포트 기준값: 계산한 다섯 포트가 모두 1024~65535 안이어야 하고 7884가 되면 안 된다(예: `--port-base 7834` → 7884라 오류).
- 접두사: `^[a-z0-9][a-z0-9_.-]*-$`(예: `t4-`, `t5-`). 소문자로 시작하고 `-`로 끝난다.

## vite dev 서버(1420 고정) 주의

앱은 `tauri.conf.json`의 `devUrl` 때문에 항상 `http://localhost:1420`만 불러온다. 그래서 다른 값으로 바꿀 수 없고, 두 가지를 조심해야 한다.

- **하네스를 동시에 두 개 돌리지 마라.** 1420이 비어 있으면 하네스가 vite를 띄우고 **끝날 때 끈다**. 먼저 띄운 쪽이 끝나면 다른 쪽이 화면을 못 불러와 깨진다. 같이 돌려야 하면 먼저 `apps/desktop`에서 `npm run dev`로 vite를 따로 띄워 두라. 이미 떠 있는 vite는 하네스가 끄지 않는다.
- **1420에 떠 있는 vite가 다른 worktree의 화면 코드를 내줄 수 있다.** 하네스는 시작할 때 1420을 듣는 프로세스의 명령줄을 읽어 이 폴더(`apps/desktop`)의 vite가 아니면 "경고" 한 줄(`setup` 단계)을 남기고 콘솔에도 출력한다. 경고가 보이면 화면 코드가 시험하려던 브랜치의 것인지 확인하라.

## 다른 작업이 돌릴 때

여러 작업이 같은 PC에서 하네스를 쓸 때는 자기 몫의 프로필·CDP 포트·포트 대역·Docker 접두사를 따로 준다. 예(설명용 — 이 값은 해당 작업이 쓰는 대역이므로 다른 작업이 함부로 돌리지 않는다):

```sh
node e2e/run.mjs --scenario all --profiles t5a,t5b --cdp 9227,9228 --port-base 8500 --prefix t5-
```

- 앱 `DONGSI_PORT` 8550, Docker 서버 8560, 응답 없는 TCP 8570, 닫힌 포트 8571, 서버 카드용 8572를 쓰고 Docker 컨테이너·볼륨은 `t5-`로 시작하는 것만 만들고 지운다.
- 대역이 겹치지 않게 서로 정한다(포트 기준값은 100 단위, CDP는 두 개씩, 프로필 이름·접두사는 작업마다 다르게).
- 환경 변수(`DONGSI_E2E_PROFILES`, `DONGSI_E2E_CDP`, `DONGSI_E2E_PORT_BASE`, `DONGSI_E2E_PREFIX`)로 줘도 된다. 인자가 우선한다.
- vite(1420)와 앱 exe의 `CARGO_TARGET_DIR`는 위 주의와 "준비"를 따른다. 지금 하네스는 같은 시각에 한 번에 하나만 돌리는 것이 안전하다(vite 때문에).
- 프로필 폴더(`%APPDATA%\com.dongsi.desktop\profiles\<이름>\`)는 시험이 끝나도 남는다. 지워도 되는지는 그 작업이 정한다. 하네스는 `state.json` 하나만 지운다.

## 알려진 한계

- 소리 막힘 알약(`needsUnmute`)은 `--autoplay-policy` 인자로도 재현되지 않았다(시나리오 a의 a6b 참고).
- 시청 연결 중 화면(b-watch-connecting)은 응답 없는 서버(8470)로 간 시청 주소로 만든다. 실제 방송이 연결되는 짧은 순간을 잡는 것이 아니다.
- 모든 시험은 한 PC 안이다. 다른 PC 사이 연결·UPnP·4K는 확인하지 않는다.
