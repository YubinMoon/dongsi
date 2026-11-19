# e2e: 실제 앱 두 개를 CDP로 조작하는 통합 시험

실제 dongsi 데스크톱 앱(Tauri 2 + WebView2) **두 개**를 띄우고 CDP(Playwright `connectOverCDP`)로 조작해서 화면·통신·재생 지표를 확인한다.
화면 시안 23장의 스크린샷도 같이 찍는다. 화면 소스는 **테스트 패턴**, 소리는 **테스트 톤**만 쓴다(사용자 화면·창 캡처 금지).

## 준비

1. `apps/desktop`에서 `npm install` (`playwright-core`와 vite가 들어 있다).
2. 저장소 루트에서 앱 exe를 만든다. `cargo build`로 만든 디버그 exe는 `localhost:1420`(vite dev 서버)을 불러오므로 개발용 단추(테스트 패턴·테스트 톤)가 보인다. 하네스가 1420이 비어 있으면 vite를 직접 띄우고 끝나면 끈다.
   ```powershell
   $env:FFMPEG_DIR = "D:\dev\ffmpeg"; $env:LIBCLANG_PATH = "D:\dev\clang+llvm\bin"
   cargo build -p dongsi-desktop        # target/debug/dongsi-desktop.exe
   ```
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
| `--keep` | 끝난 뒤 `t4-` 컨테이너·볼륨을 지우지 않음 |
| `--exe <경로>` | 앱 exe 경로. 기본 `target/debug/dongsi-desktop.exe` |

종료 코드는 실패한 검사가 하나라도 있으면 1이다. 검사가 실패해도 다음 단계로 넘어가고, 앞 단계에 기대는 단계만 "건너뜀"으로 적는다.

## 결과 파일

- `summary.md`: 검사마다 이름·통과/실패/경고/건너뜀·값, 시청 지표 표, 스크린샷 표(못 찍은 것 포함)
- `results.json`: 같은 내용(기계가 읽는 용)
- `logs/<시나리오>-<A|B>.{stdout,stderr}.log`: 앱 로그, `logs/vite.*.log`
- `failures/`: 실패한 단계의 화면(모든 창)
- `shots/`: 스크린샷(1100×720, 방송 시작 1100×860, 방송 중 1100×900, 포트 안내 1100×1000, 분리 창 1280×760. CSS 픽셀)

## 시나리오

- **a** (클라-클라): 로컬 랜 방송 → 감지 타일 → 시청 → 통계 → 시청자 수 → 분리 창 → 소리 막힘(needsUnmute) 재현 시도 → 방송 끝 → 비밀번호 방송(오답·정답·기억) → 저장 서버·"비밀번호 기억됨" → 내장 서버 닫힘 → 포트 대체 안내
- **b** (클라-서버): WSL Docker 서버 `t4-srv`(포트 8460) → 초대 코드로 서버 추가 → 연결 → 서버로 방송 → 시청 → 방송 중 화질·코덱 변경(1080p30 VP8, 720p30 VP9) → 방송 끝 → 서버 정지(연결 끊김 화면) → 재시작·다시 연결 → 다른 인증서의 `t4-srv2`로 지문 경고
- **shots**: 홈 카드·메뉴·이름 변경·삭제 대화 상자·연결 중(8470, 응답 없는 서버)·연결 실패(8471)·시청 연결 중·시청 오류·설정

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
- 앱 데이터: `DONGSI_PROFILE=t4a|t4b`로 `%APPDATA%\com.dongsi.desktop\profiles\t4a|t4b\`만 쓰고, 시작할 때 그 안의 `state.json` 파일 하나만 지운다(경로 모양을 검사한다). 사용자 앱 데이터는 건드리지 않는다.
- 프로세스: 하네스가 띄운 PID만 `taskkill /T /F /PID`로 끈다. 비정상 종료(Ctrl+C 포함)에도 정리한다.
- Docker: 이름이 `t4-`로 시작하는 컨테이너·볼륨만 만들고 지운다.

## 포트

| 용도 | 포트 |
|---|---|
| 앱 두 개(내장 서버·감지 질의, `DONGSI_PORT`) | 8450 |
| Docker 서버(`t4-srv`, `t4-srv2`는 정지 후 같은 포트) | 8460 |
| 응답 없는 TCP 서버(shots) | 8470 |
| 아무도 안 듣는 포트(shots) | 8471 (8472는 서버 카드 하나용) |
| CDP | 앱 A 9225, 앱 B 9226 |
| vite dev 서버 | 1420 |

7884(기본 포트)는 쓰지 않는다. 시나리오 a의 마지막 단계는 8450 TCP·UDP를 일부러 점유해서 포트 대체를 시험한다.

## 알려진 한계

- 소리 막힘 알약(`needsUnmute`)은 `--autoplay-policy` 인자로도 재현되지 않았다(시나리오 a의 a6b 참고).
- 시청 연결 중 화면(b-watch-connecting)은 응답 없는 서버(8470)로 간 시청 주소로 만든다. 실제 방송이 연결되는 짧은 순간을 잡는 것이 아니다.
- 모든 시험은 한 PC 안이다. 다른 PC 사이 연결·UPnP·4K는 확인하지 않는다.
