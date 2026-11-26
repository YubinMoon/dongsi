# 다시 빌드하는 방법

이 묶음은 dongsi 앱과 함께 배포한 ffmpeg DLL(avcodec-62.dll, avutil-60.dll, swscale-9.dll)을 만들 때 쓴
소스 원본과 빌드 스크립트다. 같은 DLL을 다시 만들거나, 고친 ffmpeg로 DLL을 바꿔 끼울 수 있다.

## 들어 있는 것

- sources/: 소스 원본 파일(내려받은 그대로). FFmpeg 본체와 정적으로 들어간 라이브러리(Opus, libvpl), 헤더 묶음(nv-codec-headers, AMF)
- scripts/ffmpeg/: 빌드 스크립트와 빌더 Docker 이미지 정의(build.sh, Dockerfile, versions.sh, inside-build.sh, gen-metadata.py, ffcheck.c, README.md)
- SHA256SUMS: 위 모든 파일의 SHA-256
- BUILD_ID: 이 묶음의 빌드 id

mingw-w64 런타임과 GCC 런타임(libgcc·libstdc++)은 빌더 이미지(우분투 24.04의 mingw-w64 패키지)에서 오며 이 묶음에는 소스가 들어 있지 않다.

## 필요한 것

- Windows의 WSL(우분투)이나 리눅스, 그리고 Docker(sudo 없이 docker 명령을 쓸 수 있어야 함)
- 빈 디스크 약 20GB, 메모리 8GB 이상

## 방법

```sh
tar xf dongsi-ffmpeg-lgpl-<버전>-<빌드 id>-src.tar
cd dongsi-ffmpeg-lgpl-<버전>-<빌드 id>-src
sha256sum -c SHA256SUMS                 # 파일 무결성 확인
bash scripts/ffmpeg/build.sh --sources sources --work <작업 폴더> --out <결과 폴더>
```

- 빌드 id는 묶음의 BUILD_ID 파일에서 자동으로 읽는다(같은 이름의 소스 묶음이 다시 만들어진다).
- --sources를 주면 인터넷 없이 빌드한다(빌더 Docker 이미지를 만들 때의 apt 내려받기는 처음 한 번 필요).
- 결과 폴더에 bin/(DLL), lib/(가져오기 라이브러리), include/, licenses/, BUILDINFO.md가 만들어진다.
- ffmpeg 소스를 고쳐 다시 빌드하려면 sources/ffmpeg-*.tar.xz를 고친 것으로 바꾸고 scripts/ffmpeg/versions.sh의 SHA-256을 맞춘 뒤 같은 명령을 돌린다.

자세한 설명은 scripts/ffmpeg/README.md를 본다.
