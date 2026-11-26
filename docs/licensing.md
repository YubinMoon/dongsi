# 라이선스와 배포 (licensing)

dongsi의 라이선스, 제3자 소프트웨어 고지, FFmpeg(LGPL) 의무를 지키는 방법, 릴리스할 때 절차를 적는다. 결정 기록은 로컬 `.docs/decisions.md`의 D-061·D-062이다. 이 문서는 법률 자문이 아니다.

## 1. 한눈에

| 대상 | 라이선스 | 어디에 |
|---|---|---|
| dongsi 자체 코드(모든 크레이트, `apps/desktop`) | **MIT**(저작권자 `YubinMoon`, 2026) | 루트 `LICENSE`, 각 `Cargo.toml`(`license.workspace = true`), `apps/desktop/package.json` |
| FFmpeg(avcodec·avutil·swscale) | **LGPL-2.1-or-later**, GPL·nonfree 부품 없는 빌드, DLL로 연결 | `scripts/ffmpeg/`(빌드), 설치 폴더(포터블 폴더)의 DLL 3개 |
| FFmpeg 안에 넣은 라이브러리 | opus(BSD-3-Clause), nv-codec-headers·AMF·libvpl(MIT, 헤더·인터페이스), MinGW-w64 winpthreads, GCC 런타임(예외 조항 포함) | 정확한 구성·버전은 빌드 결과의 `licenses/components.json`이 기준이고, `THIRD_PARTY_NOTICES.md`가 그것으로 만들어진다 |
| Pretendard 글꼴 | OFL-1.1 | `apps/desktop/static/licenses/Pretendard-OFL-1.1.txt` |
| npm 패키지(화면 코드에 들어가는 것) | MIT(@sveltejs/kit, clsx, svelte), Apache-2.0 OR MIT(@tauri-apps/api) | `THIRD_PARTY_NOTICES.md` 3절 |
| Rust 크레이트(앱·서버에 들어가는 것) | 허용 목록 안의 허용형(MIT, Apache-2.0, BSD, ISC, Zlib, Unicode-3.0 등) + **MPL-2.0 6개** + WTFPL 1개 | `THIRD_PARTY_NOTICES.md` 4절 |
| Microsoft WebView2 런타임 | 시스템 구성 요소(Windows 11에 포함). 우리가 배포하지 않는다 | — |

- **Rust의 MPL-2.0 크레이트**: `attohttpc`, `cssparser`, `cssparser-macros`, `dtoa-short`, `option-ext`, `selectors`(2026-11-20 확인, 정확한 목록은 `THIRD_PARTY_NOTICES.md`). 파일 단위 약한 copyleft라서 크레이트를 수정하지 않고 쓰는 한 의무는 라이선스 문구와 소스 위치(crates.io) 고지뿐이다. 이 크레이트를 수정해 쓰게 되면 수정한 파일의 소스를 공개해야 한다.
- **Rust의 WTFPL 크레이트**: `ffmpeg-sys-next`(FFmpeg 바인딩, 허용형).
- 허용 라이선스 목록은 `scripts/licenses/about.toml`의 `accepted`. 거기 없는 라이선스(GPL·LGPL·AGPL 등)가 의존성에 들어오면 고지 생성이 실패하고, 목록에 넣기 전에 copyleft인지 확인해서 여기에 적는다.
- **영상 인코더와 특허**: 영상은 GPU 드라이버가 제공하는 하드웨어 H.264 인코더(NVENC·AMF·QSV)로만 만든다(D-103). 소프트웨어 영상 인코더(openh264)와 AV1 인코더(SVT-AV1·av1_nvenc·av1_amf·av1_qsv)는 FFmpeg 빌드 스크립트에서 뺐다(2026-11-26. 그 뒤 다시 빌드한 DLL부터 해당하고, 그 전 빌드와 지금의 `THIRD_PARTY_NOTICES.md`에는 아직 들어 있다). 그래서 "직접 빌드한 openh264에는 시스코의 H.264 특허 사용료 대납이 적용되지 않는다"는 문제와 SVT-AV1의 AOM 특허 라이선스는 더 이상 우리 이진 파일에 해당하지 않는다. FFmpeg DLL에 남은 H.264 관련 코드(하드웨어 인코더를 부르는 코드, 예제 record 검사용 H.264 디코더)의 특허 사용료 문제는 확인하지 않았다(**미검증**). 지금은 조치하지 않고, 배포 방식(유료·상업 배포 등)이 정해지면 다시 검토한다.

## 2. LGPL 의무를 지키는 방법

ffmpeg.org/legal.html의 "LGPL 준수 체크리스트"를 **2026-11-20에 다시 읽고** 항목마다 대응을 적었다. 이 체크리스트는 LGPL을 지키는 유일한 방법이 아니라 가장 쉬운 방법이다. 근거 조항은 LGPL 2.1의 4절(이진 파일에 소스 동반), 6절(라이브러리를 쓰는 프로그램, 6b 공유 라이브러리 방식, 6d 같은 곳에서 소스 제공)이다.

| # | 체크리스트 항목 | 우리의 대응 |
|---|---|---|
| 1 | `--enable-gpl`, `--enable-nonfree` 없이 빌드 | `scripts/ffmpeg/build.sh`가 LGPL 최소 구성으로만 빌드한다. 고지 생성기와 포터블 스크립트가 `components.json`의 configure 줄을 검사해 `--enable-gpl`·`--enable-nonfree`(와 `--enable-libx264` 등)가 있으면 멈춘다 |
| 2 | FFmpeg 라이브러리를 동적으로 링크(Windows는 DLL) | `avcodec-62.dll`, `avutil-60.dll`, `swscale-9.dll`을 `dongsi.exe` 옆에서 불러 쓴다(LGPL 6b). 이름을 바꾸지 않아 사용자가 호환 DLL로 바꿀 수 있다 |
| 3 | 수정 여부와 관계없이 FFmpeg 소스를 배포 | FFmpeg 소스 묶음(`dongsi-ffmpeg-lgpl-<버전>-<빌드 id>-src.tar`)을 GitHub 릴리스에 첨부한다 |
| 4 | 배포한 소스가 라이브러리 이진 파일과 정확히 일치 | 소스 묶음은 DLL을 만든 바로 그 빌드의 입력(원본 소스 tarball, 패치, 빌드 스크립트·Dockerfile, 도구 버전 고정)이다. 묶음의 SHA-256을 `components.json`·릴리스에 적고, 포터블 zip과 소스 묶음은 같은 빌드 결과에서 만든 짝만 같은 릴리스에 올린다 |
| 5 | FFmpeg 소스 루트에서 git으로 변경분 diff 생성 | 우리는 FFmpeg 소스를 고치지 않는다(원본 tarball + configure 옵션). 패치를 쓰게 되면 묶음에 diff를 넣는다 |
| 6 | 소스 루트에 FFmpeg를 어떻게 빌드했는지 설명하는 텍스트 파일 | 소스 묶음의 `BUILDINFO.md`(configure 줄, 도구 버전)와 `components.json`. configure 줄은 `THIRD_PARTY_NOTICES.md`에도 있다 |
| 7 | 소스를 tarball 또는 zip으로 묶음 | `.tar` 하나(+ `.sha256`) |
| 8 | 소스를 이진 파일 내려받기와 같은 웹 서버에 둠 | 같은 GitHub 릴리스에 설치 파일(포터블 zip을 올리면 그것도)과 소스 묶음을 같이 첨부한다(LGPL 4절·6d의 "같은 곳") |
| 9 | 내려받기 페이지마다 FFmpeg 크레딧, LGPL 2.1 전문 링크, 소스 링크 고지 | 릴리스 노트에 고정 문구를 넣는다(5절 예시). 저장소 README가 생기면 거기에도 |
| 10 | 프로그램의 "정보" 대화 상자에 FFmpeg 크레딧 | 설정 > 이 앱의 "FFmpeg" 행("이 프로그램은 LGPL 2.1 이상으로 배포되는 FFmpeg를 사용합니다." + 소스 위치)과 "제3자 고지" 화면 |
| 11 | EULA에 FFmpeg를 LGPL 2.1로 쓴다고 명시 | EULA가 없다. 같은 내용이 설치 폴더(포터블 폴더)의 `README.txt`, `THIRD_PARTY_NOTICES.md`, 앱 설정 화면에 있다 |
| 12 | EULA가 코드 소유를 주장하면 FFmpeg는 소유하지 않는다고 명시 | 소유를 주장하는 EULA가 없다. `LICENSE`(MIT)는 dongsi 코드에만 해당하고, `README.txt`에 "FFmpeg는 FFmpeg 개발자들의 소프트웨어이며 dongsi가 소유한 코드가 아니다"를 적었다 |
| 13 | EULA에서 리버스 엔지니어링 금지 삭제 | 금지 조항이 없다(MIT는 수정·재배포를 허용). LGPL 6절의 "수정과 디버깅을 위한 리버스 엔지니어링 허용" 조건도 MIT로 충족한다 |
| 14 | 번역된 EULA에도 같은 변경 | 한국어 문구뿐이다. 고지는 한국어 본문 + 영어 라이선스 전문이다 |
| 15 | 이름은 "FFmpeg"(F 대문자 둘, mpeg 소문자) | 문서·앱 문구에서 "FFmpeg"로 쓴다. `rg -n -i ffmpeg`로 확인할 수 있다(식별자·파일 이름 제외) |
| 16 | DLL 이름을 관련 없는 이름으로 바꾸지 않음(앞·뒤 접두사는 허용) | 원래 이름 그대로 둔다 |
| 17 | FFmpeg에 컴파일해 넣은 다른 LGPL 외부 라이브러리에도 같은 점검 | 넣은 라이브러리는 BSD·MIT 계열이다(1절 표). LGPL 외부 라이브러리는 넣지 않는다. `components.json`·`THIRD_PARTY_NOTICES.md`가 목록과 라이선스 전문을 담는다 |
| 18 | GPL 라이브러리(libx264 등)를 쓰지 않음 | `crates/media`의 libx264 후보를 뺐고, 지금은 소프트웨어 영상 인코더 자체가 없다(하드웨어 H.264만, D-103). 저장소 전체를 `rg -i`로 x264·x265·xvid·postproc·frei0r·rubberband·vidstab·gpl을 검색해 GPL 전용 인코더·필터를 쓰는 곳이 없음을 확인했다(2026-11-20) |

LGPL 2.1 6절은 이 밖에 "작업물과 함께 라이브러리가 쓰였다는 눈에 띄는 고지를 줄 것", "라이선스 사본을 제공할 것", "실행 중 저작권 고지를 표시한다면 라이브러리의 저작권 고지를 포함할 것"을 요구한다. 앱 설정 화면·`README.txt`·`THIRD_PARTY_NOTICES.md`(LGPL 2.1 전문 포함)가 이를 충족한다. 사용자가 다른 DLL로 바꿀 수 있어야 한다는 조건은 설치 폴더(포터블 폴더)의 `README.txt`에 안내가 있다.

## 3. 고지 파일과 앱 안 표시

- 루트 `THIRD_PARTY_NOTICES.md`는 **생성물**이다. 직접 고치지 않는다. `node scripts/licenses/generate.mjs --ffmpeg <FFmpeg 폴더>`로 만든다(FFmpeg 폴더는 환경 변수 `FFMPEG_DIR`로도 줄 수 있다). 최신인지만 보려면 `--check`.
  - 필요한 것: Node.js, `apps/desktop`에서 `npm ci`, cargo-about(`cargo install cargo-about --locked`).
  - FFmpeg 절: `<FFmpeg 폴더>/licenses/components.json`(없거나 GPL·nonfree면 오류). npm 절: 프런트엔드를 빌드하면서 `apps/desktop/vite-bundle-report.js`(Vite 플러그인)가 번들에 실제로 들어간 패키지를 모은다. Rust 절: `cargo about generate --workspace --format json --offline`(설정 `scripts/licenses/about.toml`, 대상 `x86_64-pc-windows-msvc`·`x86_64-unknown-linux-gnu`, build·dev 의존성과 우리 크레이트 제외).
  - 결과는 실행할 때마다 같다(정렬, 날짜·절대 경로 없음).
- **다시 만들어야 할 때**: `Cargo.lock`·`package-lock.json`이 바뀌어 의존성이 달라졌을 때, 화면에 쓰는 npm 패키지가 바뀌었을 때, FFmpeg 빌드가 바뀌었을 때. 커밋 전에 `--check`로 확인한다.
- 앱 설정 화면의 "이 앱" 패널에 라이선스(MIT), FFmpeg 고지(소스 위치는 복사 단추), 제3자 고지 "보기"(`#/settings/licenses`)가 있다. 제3자 고지 화면은 루트 `THIRD_PARTY_NOTICES.md`를 Vite의 `?raw` import로 화면 번들에 넣고 그 화면을 열 때만 불러온다.
- 설치 폴더와 포터블 폴더에는 `LICENSE`, `THIRD_PARTY_NOTICES.md`, `licenses/`(FFmpeg 구성 요소 라이선스, `FFmpeg-BUILDINFO.md`, Pretendard), `README.txt`가 들어간다(`scripts/lib/release.ps1`의 `Copy-LicenseFiles`, `Write-ReleaseReadme`).
- 서버 Docker 이미지에는 `/usr/share/doc/dongsi-server/`에 `LICENSE`와 `THIRD_PARTY_NOTICES.md`가 들어간다. 고지 파일은 앱과 서버를 합친 하나이고(Rust 절은 Windows·Linux 대상을 모두 담는다), 서버는 FFmpeg를 쓰지 않는다.

## 4. FFmpeg 빌드와 소스 묶음

`scripts/ffmpeg/build.sh`(WSL의 Docker, 자세한 옵션은 `scripts/ffmpeg/` 안의 안내)가 최소 LGPL FFmpeg를 MinGW-w64로 교차 빌드한다. 결과 폴더(`--out`으로 준 곳)는 다음 모양이다.

```text
bin/        avcodec-62.dll, avutil-60.dll, swscale-9.dll
include/    lib/        (MSVC 가져오기 라이브러리, Rust 빌드가 링크)
licenses/   components.json, <구성 요소 id>/<라이선스 파일>
source/     <소스 묶음>.tar, <소스 묶음>.tar.sha256
BUILDINFO.md
```

Rust 빌드는 이 폴더를 `FFMPEG_DIR`(또는 `third_party/ffmpeg` 정션, AGENTS.md "빌드 환경")로 가리킨다.

## 5. 릴리스할 때 절차

릴리스·태그는 사용자가 요청할 때만 만든다. 0.1.0부터 클라이언트는 **설치 파일**(NSIS), 서버는 **Docker 이미지**(GHCR)로 배포한다(D-063). 포터블 zip은 지금 릴리스에 올리지 않는다(만들 수는 있다).

1. **FFmpeg 빌드**: `bash scripts/ffmpeg/build.sh …`로 결과 폴더(`<FFmpeg 폴더>`)를 만든다. `licenses/components.json`과 `source/`의 소스 묶음·`.sha256`이 있는지 본다.
2. **버전 올리기**: 워크스페이스 `Cargo.toml`의 `[workspace.package] version`, `apps/desktop/src-tauri/tauri.conf.json`의 `version`, `apps/desktop/package.json`의 `version`을 같이 올린다(`Cargo.lock`도 바뀐다).
3. **시험**: `FFMPEG_DIR=<FFmpeg 폴더>`로 전체 시험 `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\test-full.ps1`(`cargo fmt --all --check`, `cargo clippy --workspace --all-targets --features full-tests`, `cargo test-full`, `apps/desktop`의 `npm run check`, `npm test`, `npm run build`, `npm run shots`. AGENTS.md "빌드·테스트"). 서버 이미지는 4번의 E2E로 시험한다.
4. **고지 다시 생성**: `node scripts/licenses/generate.mjs --ffmpeg <FFmpeg 폴더>` → `THIRD_PARTY_NOTICES.md`를 커밋한다(`--check`로 최신인지 본다). 이미지와 설치 파일이 이 파일을 담으므로 둘을 만들기 전에 한다.
5. **설치 파일**: `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\package-installer.ps1 -FfmpegDir <FFmpeg 폴더>` → `dist\dongsi-<버전>-windows-x64-setup.exe`와 `.sha256`. `LIBCLANG_PATH`에 LLVM bin을 주면 exe·DLL의 DLL 요구도 점검한다.
   - `tauri.conf.json`은 `bundle.active = false`라서 `npm run tauri build`만으로는 설치 파일이 만들어지지 않는다(FFmpeg DLL이 빠진 설치 파일을 막는다). 스크립트가 `--config`로 `bundle.resources`(DLL·라이선스 파일)와 `active = true`를 덧붙인다.
   - 설치는 사용자별(`installMode: currentUser`, 관리자 권한 없음, 기본 `%LOCALAPPDATA%\dongsi`)이다. 코드 서명이 없어 SmartScreen 경고가 뜬다.
   - 시작 메뉴 바로 가기 이름은 `동시`다(윈도우 검색에 "동시"로 나오게). Tauri는 `dongsi.lnk`를 만들므로 설치 프로그램 hook(`apps/desktop/src-tauri/windows/installer-hooks.nsh`, UTF-8 BOM)이 설치 뒤 이름을 바꾸고 제거 때 지운다. 바탕 화면 바로 가기는 `dongsi.lnk` 그대로다.
6. **설치 파일 확인**: 설치 파일을 `/S /D=<빈 폴더>`로 조용히 설치하고, 설치 폴더에 `dongsi.exe`·DLL 3개·라이선스 파일이 있는지, 시작 메뉴(`%APPDATA%\Microsoft\Windows\Start Menu\Programs`)에 `동시.lnk`가 있고 `dongsi.lnk`는 없는지, FFmpeg가 PATH에 없는 상태에서 `dongsi.exe`가 뜨고 로그에 "인코더 탐색 완료"가 나오는지 본다(시험용 `DONGSI_PROFILE`을 준다). 끝나면 설치 폴더의 `uninstall.exe /S`로 지우고, `reg delete HKCU\Software\YubinMoon\dongsi /f`로 설치 위치 기록도 지운다. 조용한 제거는 이 키(기본값 = 설치 폴더)를 남기고, 다음 설치 프로그램이 그 폴더를 기본 위치로 다시 띄운다. 앱을 실제로 설치해 쓰는 PC라면 시험 설치가 그 설치 기록을 덮으므로, 시험 뒤 설치 파일을 다시 실행해 원래 위치(`%LOCALAPPDATA%\dongsi`)에 설치한다.
7. **서버 이미지**: `powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/e2e-linux-server.ps1 -Modes bridge,host -ImageTag ghcr.io/yubinmoon/dongsi-server:<버전>`로 커밋한 상태(HEAD)에서 이미지를 만들고 E2E로 시험한다(준비물·인자는 스크립트 머리 도움말). 통과하면 WSL에서 `latest` 태그를 더하고 push한다. push에는 `write:packages` 권한이 있는 토큰이 필요하다(`gh auth refresh -h github.com -s write:packages` 뒤 `gh auth token | docker login ghcr.io -u <사용자> --password-stdin`).
   ```sh
   docker tag ghcr.io/yubinmoon/dongsi-server:<버전> ghcr.io/yubinmoon/dongsi-server:latest
   docker push ghcr.io/yubinmoon/dongsi-server:<버전>
   docker push ghcr.io/yubinmoon/dongsi-server:latest
   ```
   이미지는 OCI index(이미지 + attestation)로 올라가서 GHCR가 Dockerfile 라벨(`org.opencontainers.image.source`)을 읽지 못하고 패키지가 저장소에 연결되지 않는다(0.1.0에서 확인). push 뒤 index에 annotation을 붙여 다시 태그한다(이미지 manifest는 그대로, index digest만 바뀐다):
   ```sh
   docker buildx imagetools create \
     --annotation "index:org.opencontainers.image.source=https://github.com/YubinMoon/dongsi" \
     --annotation "index:org.opencontainers.image.licenses=MIT" \
     --annotation "index:org.opencontainers.image.title=dongsi-server" \
     --annotation "index:org.opencontainers.image.version=<버전>" \
     --annotation "index:org.opencontainers.image.description=dongsi dedicated server (screen sharing signaling + SFU), linux/amd64" \
     --tag ghcr.io/yubinmoon/dongsi-server:<버전> --tag ghcr.io/yubinmoon/dongsi-server:latest \
     ghcr.io/yubinmoon/dongsi-server:<버전>
   docker logout ghcr.io
   ```
   `gh api users/<사용자>/packages/container/dongsi-server`의 `repository`가 저장소 이름이면 연결된 것이다.

   **패키지는 공개여야 한다.** 저장소가 공개이고, compose 파일과 릴리스 노트는 로그인 없이 `docker pull`한다고 가정한다. GHCR은 처음 게시한 패키지를 비공개로 만들고, 공개로 바꾸는 API는 없다(2026-11-20, PATCH 404). 처음 게시했다면 패키지 설정(`https://github.com/users/<사용자>/packages/container/dongsi-server/settings` → Danger Zone → Change visibility → Public, 패키지 이름 입력)에서 바꾼다(`dongsi-server`는 2026-11-21에 공개로 바꿨다). 릴리스를 알리기 전에 매번 확인한다:
   ```sh
   gh api users/<사용자>/packages/container/dongsi-server --jq .visibility      # public
   DOCKER_CONFIG=$(mktemp -d) docker pull ghcr.io/yubinmoon/dongsi-server:<버전>   # 로그인 정보 없이 받아진다
   ```
8. **GitHub 릴리스**(`gh`): 태그를 만들고 릴리스에 다음을 첨부한다 — 설치 파일과 `.sha256`, **FFmpeg 소스 묶음**(`<FFmpeg 폴더>\source\dongsi-ffmpeg-lgpl-<버전>-<빌드 id>-src.tar`)과 그 `.sha256`. 예:
   ```text
   gh release create v<버전> --title "dongsi <버전>" --notes-file <릴리스 노트> ^
     dist\dongsi-<버전>-windows-x64-setup.exe ^
     dist\dongsi-<버전>-windows-x64-setup.exe.sha256 ^
     <FFmpeg 폴더>\source\dongsi-ffmpeg-lgpl-<버전>-<빌드 id>-src.tar ^
     <FFmpeg 폴더>\source\dongsi-ffmpeg-lgpl-<버전>-<빌드 id>-src.tar.sha256
   ```
   소스 묶음 파일 이름과 SHA-256은 `THIRD_PARTY_NOTICES.md`, `README.txt`, `components.json`에 적힌 것과 같아야 한다(같은 빌드 결과로 만든 것).
9. 릴리스 페이지에서 첨부 파일이 모두 내려받아지는지, 해시가 맞는지, 로그인하지 않은 상태(다른 PC나 빈 `DOCKER_CONFIG`)에서 `docker pull`이 되는지 확인한다.

포터블 폴더가 필요하면 `scripts\package-portable.ps1 -FfmpegDir <FFmpeg 폴더>` → `dist\dongsi-<버전>-windows-x64-portable\`와 `.zip`(테스트 패턴 방송 확인용 `-Debug`는 배포하지 않는다). 올린다면 같은 빌드의 소스 묶음과 같은 릴리스에 둔다.

### 릴리스 노트 문구 예시

```text
## dongsi <버전>

### 클라이언트(Windows 11 x64)
dongsi-<버전>-windows-x64-setup.exe를 실행해 설치합니다(사용자별 설치, 관리자 권한 필요 없음).
코드 서명이 없어 "Windows의 PC 보호" 창이 뜨면 "추가 정보 → 실행"을 누릅니다.

### 서버(Linux x64, Docker)
docker pull ghcr.io/yubinmoon/dongsi-server:<버전>
실행 방법: deploy/README.md

### 라이선스
- dongsi는 MIT 라이선스입니다(LICENSE).
- 이 프로그램은 LGPL 2.1 이상으로 배포되는 FFmpeg(https://ffmpeg.org/)를 사용합니다.
  FFmpeg는 FFmpeg 개발자들의 소프트웨어이며 dongsi가 소유한 코드가 아닙니다.
  LGPL 2.1 전문: https://www.gnu.org/licenses/old-licenses/lgpl-2.1.html (설치 폴더의 THIRD_PARTY_NOTICES.md에도 있습니다)
- FFmpeg 소스: 이 릴리스에 첨부한 dongsi-ffmpeg-lgpl-<버전>-<빌드 id>-src.tar
  (SHA-256: <해시>, .sha256 파일도 첨부). 같은 DLL을 다시 만드는 방법: docs/licensing.md
- 다른 제3자 소프트웨어의 고지: 설치 폴더의 THIRD_PARTY_NOTICES.md (앱: 설정 > 이 앱 > 제3자 고지). 서버 이미지: /usr/share/doc/dongsi-server/
```

## 6. FFmpeg 소스 묶음을 얼마나 오래 두는가

- LGPL 2.1은 4절(이진 파일에 소스를 동반)과 6d절(이진 파일을 "지정한 곳"에서 내려받게 하면 소스도 **같은 곳**에서 받게 하면 된다)에서 소스 제공 방식을 정할 뿐 **보관 기간을 따로 정하지 않는다**. "최소 3년"은 6c절의 **서면 제안**(소스를 우편 등으로 준다는 약속)에만 나오는 기간이고, 우리는 서면 제안 방식을 쓰지 않는다.
- 그러므로 법이 요구하는 최소 기준은 **그 DLL이 든 이진 파일을 내려받을 수 있게 두는 동안, 같은 곳(같은 GitHub 릴리스)에서 일치하는 소스 묶음을 내려받을 수 있게 하는 것**이다.
- 우리 방침(법적 요구가 아니라 안전 여유): ① 릴리스를 삭제하지 않고 소스 묶음을 릴리스에 계속 둔다. ② 어쩔 수 없이 릴리스(이진 파일)를 내리더라도 **마지막 배포일부터 3년** 동안은 소스 묶음을 보관하고 요청하면 제공한다(6c절의 3년을 따른 것).
- 소스 묶음은 해당 빌드에서 만든 이진 파일마다 짝으로 보관한다(다른 빌드의 소스로 대신하지 않는다).

## 7. 보류·미결

- 코드 서명, 자동 업데이트는 아직 없다(plan.md 6단계).
- H.264 특허 사용료(1절, FFmpeg DLL의 H.264 관련 코드)는 상업 배포를 하게 되면 다시 본다(미검증).
- 서버 이미지의 기반 `debian:trixie-slim`에는 GPL 등 Debian 패키지가 들어 있다. 패키지별 라이선스는 이미지 안의 `/usr/share/doc/*/copyright`에 있고 소스는 Debian 저장소(snapshot.debian.org)에 있다. 기반 이미지를 그대로 재배포할 때 따로 소스를 첨부해야 하는지는 **미검증**이다(지금은 하지 않는다).
