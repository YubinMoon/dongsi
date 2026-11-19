# 최소 LGPL ffmpeg 교차 빌드 (scripts/ffmpeg)

dongsi 앱이 DLL로 동적 링크하는 ffmpeg를 **우리가 쓰는 것만 넣은 LGPL 빌드**로 만든다. 결과는 앱 옆에 놓는
DLL 세 개(avcodec-62.dll, avutil-60.dll, swscale-9.dll)와 링크용 가져오기 라이브러리·헤더·라이선스 모음이다.

## 무엇을 왜 이렇게 빌드하는가

- 우리 코드는 MIT, ffmpeg는 LGPL로 둔다(결정 D-061·D-062). 그러려면 GPL·version3·nonfree 부품이 없어야 한다.
  그래서 `--enable-gpl`, `--enable-version3`, `--enable-nonfree` 없이 빌드한다(LGPL 2.1 이상).
- 기성 BtbN lgpl-shared 대신 **쓰는 것만 넣은 최소 빌드**를 스크립트로 만든다. 기성 빌드는 DLL 안에 fribidi·libass·
  fontconfig·harfbuzz·libxml2 같은 많은 라이브러리가 정적으로 들어 있어서 "배포한 바이너리와 정확히 같은 소스 제공"과
  고지의 범위가 커지고 크기도 크다. 최소 빌드는 소스 묶음·고지가 작고, 무엇이 들었는지 정확히 안다.
  (수치는 아래 "기성 빌드와의 차이" 절.)
- 우리가 쓰는 라이브러리는 avcodec, avutil, swscale뿐이다. avformat·avfilter·avdevice·swresample·네트워크는 끈다.
- 외부 인코더는 정적 라이브러리로 DLL 안에 넣는다: OpenH264(H.264), libvpx(VP8·VP9), SVT-AV1(AV1), Opus.
  하드웨어 인코더: NVENC(nv-codec-headers), AMF(AMD 헤더), QSV(libvpl 디스패처). 하드웨어 쪽은 실행할 때
  각 드라이버의 DLL(nvEncodeAPI64, amfrt64, Intel 런타임)을 불러 쓴다.
- 인코더 14개: h264_nvenc hevc_nvenc av1_nvenc h264_amf hevc_amf av1_amf h264_qsv hevc_qsv av1_qsv libopenh264
  libvpx(VP8) libvpx-vp9 libsvtav1 libopus. 디코더 5개(예제 record의 `--decode` 검사용): h264 hevc vp8 vp9 libopus. 네이티브 opus 디코더는 swresample이 필요해서
  (swresample은 끔) libopus 디코더로 대신한다(record는 "opus", "libopus" 순으로 찾는다).
- AV1 디코더(libdav1d)는 넣지 않았다. 앱은 디코딩을 WebView2가 하므로 필요 없고, 예제 record의 AV1 `--decode`
  검사만 못 한다.
- mingw 런타임(libgcc, libstdc++, libwinpthread)은 DLL 안에 정적으로 넣어서, 결과 DLL은 **윈도우 시스템 DLL과
  ffmpeg DLL 말고는 아무것도 가져오지 않는다**(빌드가 이를 검사해서 어기면 실패한다).
- 재현성: Docker 빌더 이미지(우분투 24.04를 다이제스트로 고정 + mingw-w64 posix)에서 교차 빌드하고, 소스는
  versions.sh의 URL·SHA-256으로 고정한다. 받은 파일의 해시가 다르면 빌드를 멈춘다. 빌드 컨테이너는 네트워크를 끈 채
  (--network none) 돈다. apt 패키지 버전은 고정하지 않으므로(그 시점의 우분투 24.04 저장소) 쓴 도구 버전은
  BUILDINFO.md에 남는다. 같은 소스 묶음과 같은 빌더 이미지(같은 우분투 패키지 버전)면 DLL, 가져오기 라이브러리, 소스 묶음
  tar가 바이트까지 같게 나온다(PE 타임스탬프는 `-Wl,--no-insert-timestamp`, 시각은 `SOURCE_DATE_EPOCH`로 고정). 소스 묶음에서 다시 빌드한
  결과와 비교해 확인했다. 우분투 패키지가 바뀌면 컴파일러 버전이 달라져 결과가 달라질 수 있다.

## 기성 빌드와의 차이

비교 대상: BtbN FFmpeg-Builds의 `ffmpeg-n8.1-latest-win64-lgpl-shared-8.1.zip`(2026-11-19 빌드, n8.1.3-36-ge22a4510c9). 내려받아
확인만 했고 쓰지 않았다.

| 항목 | 기성 BtbN lgpl-shared | 이 빌드 |
|---|---|---|
| FFmpeg 라이선스 | `--enable-version3` → LGPL 3.0 이상 | LGPL 2.1 이상 |
| 외부 라이브러리 | 약 60개(fontconfig, harfbuzz, freetype, fribidi, libass, libxml2, vulkan, libplacebo, libssh, libsrt, libzmq, libbluray, libjxl 등) | 5개 정적(OpenH264, libvpx, SVT-AV1, Opus, libvpl) + 헤더 2개(nv-codec-headers, AMF) |
| avcodec-62.dll | 93,854,720바이트 | 17,333,760바이트 |
| avutil-60.dll | 3,025,920 | 2,370,560 |
| swscale-9.dll | 12,954,112 | 2,042,880 |
| 앱이 읽는 DLL 합계 | 110,584,320(swresample-6.dll 749,568 포함, avcodec가 가져옴) | 21,747,200 |
| avcodec의 가져오는 DLL | 40여 개(d2d1, DWrite, USP10, DNSAPI, WS2_32 등) | 5개(ADVAPI32, KERNEL32, msvcrt, ole32, avutil-60) |
| C 런타임 | UCRT | msvcrt.dll |

기성 빌드는 안 쓰는 라이브러리가 DLL 안에 정적으로 들어 있고, "배포한 바이너리와 정확히 같은 소스 제공" 의무가 그 전부에
걸린다. 이 빌드는 소스 묶음에 7개 소스만 들어간다.

## 요구 사항

- WSL(우분투) 또는 리눅스, Docker(sudo 없이 `docker` 명령이 돌아야 함), bash, curl, sha256sum, tar
- 빈 디스크 약 20GB(작업 폴더 + Docker 이미지), 메모리 8GB 이상 권장
- 결과물 확인(ffcheck)은 WSL의 Windows 실행 파일 연동(interop)이나 wine이 있어야 돈다. 없으면 건너뛰고 알려 준다.

## 실행 방법

```sh
# WSL에서, 저장소 루트
bash scripts/ffmpeg/build.sh --work ~/dongsi-ffmpeg-build --out /mnt/d/dev/ffmpeg-lgpl --jobs 6
```

| 인자 | 뜻 |
|---|---|
| `--out <폴더>` | 결과물 폴더. 기본 `<저장소>/third_party/ffmpeg-lgpl`(`third_party/`는 git에서 제외됨) |
| `--work <폴더>` | 작업 폴더(받은 소스, 중간물). 기본 `~/.cache/dongsi-ffmpeg-build` |
| `--jobs N` | 병렬 수. 기본 6 |
| `--sources <폴더>` | 미리 받은 소스 폴더. 주면 인터넷으로 내려받지 않는다 |
| `--clean` | 작업 폴더의 중간물을 지우고 처음부터(받은 소스는 남김) |
| `--build-id <id>` | 소스 묶음 이름에 쓰는 빌드 id. 기본 날짜-스크립트 해시 |
| `--no-check` | ffcheck 실행을 건너뜀 |

작업 폴더는 외부 라이브러리 빌드 결과를 단계별로 기억한다(같은 소스·같은 inside-build.sh면 건너뜀). ffmpeg 본체는
매번 다시 빌드한다. 빌드 중 처음 한 번은 빌더 이미지를 만들며 apt 패키지를 내려받는다.

앱을 이 DLL로 빌드·실행할 때는 저장소 루트 AGENTS.md의 환경 변수를 이 폴더로 가리킨다:
`FFMPEG_DIR=<결과 폴더>`, PATH 앞에 `<결과 폴더>\bin`.

## 결과물 모양

```
<out>/
  bin/            avcodec-62.dll, avutil-60.dll, swscale-9.dll (strip됨, exe 없음)
  include/        libavcodec, libavutil, libswscale 헤더
  lib/            avcodec.lib avutil.lib swscale.lib (MSVC 링커용), lib*.dll.a, *.def, pkgconfig/
  licenses/       components.json + <구성 요소 id>/<라이선스 파일>
  BUILDINFO.md    버전·configure 줄·구성 요소·도구·DLL 크기·가져오기 목록·라이선스 함수 출력·인코더 목록
  source/         dongsi-ffmpeg-lgpl-<ffmpeg 버전>-<빌드 id>-src.tar (+ .sha256)
```

### 소스 묶음

LGPL은 배포한 DLL과 정확히 같은 소스를 줘야 한다. `source/`의 tar가 그것이다: 빌드에 쓴 소스 원본 파일 그대로
(FFmpeg, OpenH264, libvpx, SVT-AV1, Opus, libvpl, 헤더 묶음 2개와 FFmpeg 서명 파일), 이 폴더(scripts/ffmpeg)의 사본,
SHA256SUMS, 다시 빌드하는 방법(REBUILD.md)이 들어 있다. GitHub 릴리스에 DLL과 같이 그대로 첨부한다. 이 파일들은
git에 넣지 않는다. 패치를 쓰게 되면 scripts/ffmpeg/patches/에 두면 묶음에 같이 들어간다(지금은 패치 없음).
mingw-w64·GCC 런타임은 빌더 이미지(우분투 패키지)에서 오며 묶음에 소스가 없다. GCC 런타임은 GCC Runtime Library
Exception, mingw-w64는 허용적 라이선스를 따르고, 라이선스 문서는 licenses/에 들어 있다.

## 버전 올리기

1. `versions.sh`의 FFMPEG_VERSION과 DOWNLOADS(파일 이름|URL|SHA-256), COMPONENTS(버전·파일 이름)를 같이 고친다.
   FFmpeg는 8.x(avcodec-62)에 머문다. 9.x(avcodec-63)로 가면 DLL 이름이 바뀌어 앱 링크가 깨진다(FFMPEG_DLLS도 같이).
2. SHA-256은 소스를 받아 `sha256sum`으로 구해 적는다. FFmpeg는 `.asc` 서명을 ffmpeg.org의 릴리스 키로 확인한다
   (`gpg --verify ffmpeg-<버전>.tar.xz.asc ffmpeg-<버전>.tar.xz`, 키 지문 FCF9 86EA 15E6 E293 A564 4F10 B432 2F04 D676 58D8).
3. 외부 라이브러리를 올릴 때: ffmpeg가 요구하는 최소 버전(`configure`의 `require_pkg_config`·`check_pkg_config`)을 확인한다.
   AMF·nv-codec-headers는 **새 헤더일수록 필요한 드라이버 버전이 올라가므로** 일부러 낮게(AMF 1.4.36 = ffmpeg 최소,
   ffnvcodec 12.2.72.0 = 윈도우 드라이버 551.76 이상) 둔다.
4. `bash scripts/ffmpeg/build.sh --clean`으로 다시 빌드하고 BUILDINFO.md의 실행 확인(ffcheck)이 OK인지 본다.
5. 앱 쪽 확인(아래)을 다시 한다. 빌드 id는 스크립트가 바뀌면 같이 바뀐다.

## 확인 방법

1. build.sh가 끝에 ffcheck.exe를 돌려 BUILDINFO.md에 붙인다: 세 라이브러리의 `*_license()`가 모두
   "LGPL version 2.1 or later"인지, 인코더 14개·디코더 5개가 있는지, 등록된 인코더·디코더 전체 목록.
2. DLL 가져오기 목록(BUILDINFO.md)에 시스템 DLL과 ffmpeg DLL만 있는지(libgcc·libstdc++·libwinpthread 없음).
3. 앱 쪽(Windows, PowerShell): `FFMPEG_DIR`을 결과 폴더로 하고 PATH에서 다른 ffmpeg의 bin을 뺀 채
   `cargo run -p dongsi-media --example record -- --list`(실제로 열어 보는 인코더 탐색),
   `cargo test -p dongsi-media`, 코덱별 짧은 녹화(`--source test --codec h264|h265|vp8|vp9|av1 --secs 3`).
   AMD·Intel 하드웨어가 없는 PC에서는 amf·qsv가 "없음"으로 빠지는 것이 정상이다(드라이버 DLL이 없어서).

## 알아 둘 사실

- **C 런타임**: 우분투의 mingw-w64는 msvcrt.dll(윈도우 시스템 DLL)을 기본으로 써서, DLL이 UCRT가 아니라 msvcrt.dll을
  가져온다(기성 BtbN·GPL 빌드는 UCRT). 앱(MSVC, UCRT)과 CRT가 다르므로 FILE*이나 malloc/free 포인터를 DLL 경계 너머로
  넘기는 코드는 안 된다. crates/media에는 그런 사용이 없다(`rg`로 확인). UCRT가 필요해지면 llvm-mingw 같은 도구 체인으로
  바꿔야 한다(우분투 24.04의 gcc 13에는 `-mcrtdll` 옵션이 없다).

- **OpenH264**: 시스코는 자신이 배포하는 OpenH264 *바이너리*에 대해서만 H.264 특허 사용료를 대신 낸다(AVC 특허
  라이선스). 이 빌드는 OpenH264를 소스에서 빌드해 DLL에 정적으로 넣었으므로 그 특허 라이선스가 적용되지 않는다.
  (사실만 적는다. 조치는 하지 않았다.)
- 하드웨어 인코더(NVENC, AMF, QSV)는 각 제조사 드라이버가 있어야 동작한다. 드라이버가 없으면 인코더를 열 때
  실패하고 앱은 다음 후보로 넘어간다(encoder.rs의 탐색).
- libvpx는 VP8·VP9 *인코더*만, SVT-AV1은 인코더만 넣었다(SVT-AV1 AVX-512 코드는 끔).
- 구성 요소별 라이선스·버전은 BUILDINFO.md, 기계가 읽는 목록은 licenses/components.json.
