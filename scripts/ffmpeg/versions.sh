# shellcheck shell=bash
# 최소 LGPL ffmpeg 빌드에 쓰는 소스의 버전·URL·SHA-256을 한곳에 모은다.
# build.sh와 inside-build.sh가 읽는다(source). 받은 파일의 SHA-256이 다르면 빌드가 멈춘다.
#
# 버전을 올리는 방법은 README.md의 "버전 올리기"를 본다.

# FFmpeg 8.x 중 최신 릴리스. 우리 exe는 avcodec-62·avutil-60·swscale-9를 읽는다(8.x 계열 공통).
# 9.x(avcodec-63)로 올리면 DLL 이름이 바뀌어 앱 쪽 링크가 깨지므로 안 된다.
FFMPEG_VERSION="8.1.3"
FFMPEG_DLLS=(avcodec-62.dll avutil-60.dll swscale-9.dll)

# 소스 내려받기 목록: 파일 이름|URL|SHA-256
DOWNLOADS=(
"ffmpeg-8.1.3.tar.xz|https://ffmpeg.org/releases/ffmpeg-8.1.3.tar.xz|7138d28c96d9d3e3af4ee3d8cad72741f8ffb40da90c1112235dea3ecd3178a3"
"ffmpeg-8.1.3.tar.xz.asc|https://ffmpeg.org/releases/ffmpeg-8.1.3.tar.xz.asc|5f2880c7fea01f8694d30cb9d06362f77241ee67ce5926b157a226b18f08a538"
"openh264-2.6.0.tar.gz|https://github.com/cisco/openh264/archive/refs/tags/v2.6.0.tar.gz|558544ad358283a7ab2930d69a9ceddf913f4a51ee9bf1bfb9e377322af81a69"
"libvpx-1.15.2.tar.gz|https://github.com/webmproject/libvpx/archive/refs/tags/v1.15.2.tar.gz|26fcd3db88045dee380e581862a6ef106f49b74b6396ee95c2993a260b4636aa"
"SVT-AV1-3.1.2.tar.gz|https://github.com/AOMediaCodec/SVT-AV1/archive/refs/tags/v3.1.2.tar.gz|f1ec3d800156d10356bdc10a7a38a81dd427c76edc3ef15dcaa7e37edfc4774f"
"opus-1.5.2.tar.gz|https://downloads.xiph.org/releases/opus/opus-1.5.2.tar.gz|65c1d2f78b9f2fb20082c38cbe47c951ad5839345876e46941612ee87f9a7ce1"
"nv-codec-headers-12.2.72.0.tar.gz|https://github.com/FFmpeg/nv-codec-headers/archive/refs/tags/n12.2.72.0.tar.gz|dbeaec433d93b850714760282f1d0992b1254fc3b5a6cb7d76fc1340a1e47563"
"AMF-headers-v1.4.36.tar.gz|https://github.com/GPUOpen-LibrariesAndSDKs/AMF/releases/download/v1.4.36/AMF-headers-v1.4.36.tar.gz|ec07ee21b820a73f5bae224e92fd3c492f52732d4885f0637f48189afdd87564"
"AMF-LICENSE-v1.4.36.txt|https://raw.githubusercontent.com/GPUOpen-LibrariesAndSDKs/AMF/v1.4.36/LICENSE.txt|eb297397aaa455b5668ab67d216b83828466152dab123fa92384c6ec16b74170"
"libvpl-2.15.0.tar.gz|https://github.com/intel/libvpl/archive/refs/tags/v2.15.0.tar.gz|7218c3b8206b123204c3827ce0cf7c008d5c693c1f58ab461958d05fe6f847b3"
)

# 구성 요소 목록(components.json·BUILDINFO.md·licenses/의 기준):
#   id|이름|버전|SPDX 라이선스|홈페이지|소스 파일(DOWNLOADS의 파일 이름)|링크 방식|라이선스 파일(소스 최상위 폴더 기준 경로, 공백 구분)|비고
# 라이선스 파일이 .h면 머리의 첫 주석 블록만 따로 뽑아 \`<파일 이름>.license.txt\`로 둔다.
# \`@이름\`은 inside-build.sh의 license_from_special()이 정하는 특수 위치(받은 파일 또는 빌더 이미지)다.
# 소스 파일이 "-"인 것은 빌더 이미지(우분투 패키지)에서 오는 도구 런타임이다(inside-build.sh가 라이선스 파일을 이미지에서 복사).
COMPONENTS=(
"ffmpeg|FFmpeg|8.1.3|LGPL-2.1-or-later|https://ffmpeg.org/|ffmpeg-8.1.3.tar.xz|공유 라이브러리(avcodec-62.dll, avutil-60.dll, swscale-9.dll)|COPYING.LGPLv2.1 LICENSE.md|--enable-gpl·--enable-version3·--enable-nonfree 없이 빌드(LGPL 2.1 이상). 소스 서명(.asc)은 소스 묶음에 들어 있다."
"openh264|OpenH264|2.6.0|BSD-2-Clause|https://www.openh264.org/|openh264-2.6.0.tar.gz|정적 링크(avcodec-62.dll 안, 인코더만)|LICENSE|시스코의 H.264 특허 사용료 대납(AVC 특허 라이선스)은 시스코가 배포한 바이너리에만 적용되며, 소스에서 직접 빌드해 정적으로 넣은 이 빌드에는 적용되지 않는다."
"libvpx|libvpx|1.15.2|BSD-3-Clause|https://www.webmproject.org/code/|libvpx-1.15.2.tar.gz|정적 링크(avcodec-62.dll 안, VP8·VP9 인코더)|LICENSE PATENTS|PATENTS 파일은 소스에서 그대로 복사."
"svt-av1|SVT-AV1|3.1.2|BSD-3-Clause-Clear AND BSD-2-Clause|https://gitlab.com/AOMediaCodec/SVT-AV1|SVT-AV1-3.1.2.tar.gz|정적 링크(avcodec-62.dll 안, 인코더만)|LICENSE.md LICENSE-BSD2.md PATENTS.md|AOMedia 특허 라이선스(PATENTS.md)가 따라온다."
"opus|Opus|1.5.2|BSD-3-Clause|https://opus-codec.org/|opus-1.5.2.tar.gz|정적 링크(avcodec-62.dll 안)|COPYING|"
"nv-codec-headers|nv-codec-headers (ffnvcodec)|12.2.72.0|MIT|https://github.com/FFmpeg/nv-codec-headers|nv-codec-headers-12.2.72.0.tar.gz|헤더만(빌드할 때 포함, NVIDIA 드라이버의 nvcuda·nvEncodeAPI는 실행 때 동적 로드)|include/ffnvcodec/dynlink_loader.h include/ffnvcodec/nvEncodeAPI.h README|별도 LICENSE 파일이 없어 헤더 머리의 MIT 고지(처음 주석 블록)를 licenses/에 따로 뽑아 두었다. 윈도우 드라이버 551.76 이상이 필요하다."
"amf|AMD AMF 헤더|1.4.36|MIT|https://github.com/GPUOpen-LibrariesAndSDKs/AMF|AMF-headers-v1.4.36.tar.gz|헤더만(빌드할 때 포함, AMD 드라이버의 amfrt64.dll은 실행 때 동적 로드)|@AMF-LICENSE-v1.4.36.txt|"
"libvpl|Intel oneVPL (libvpl) 디스패처|2.15.0|MIT|https://github.com/intel/libvpl|libvpl-2.15.0.tar.gz|정적 링크(avcodec-62.dll 안, 디스패처만. 실제 인코더는 Intel 드라이버의 런타임이 실행 때 로드됨)|LICENSE|"
"mingw-w64|mingw-w64 런타임·winpthreads|11.0.1|ZPL-2.1 AND MIT AND BSD-3-Clause|https://www.mingw-w64.org/|-|정적 링크(CRT 시작 코드·libmingwex, libwinpthread)|@mingw-w64-common-copyright|빌더 이미지(우분투 mingw-w64 11.0.1)의 copyright 파일을 그대로 복사했다. 여러 부품의 라이선스가 한 파일에 있다."
"gcc-runtime|GCC 런타임 라이브러리(libgcc, libstdc++)|13.2.0|GPL-3.0-or-later WITH GCC-exception-3.1|https://gcc.gnu.org/|-|정적 링크(avcodec-62.dll 안)|@gcc-mingw-w64-base-copyright @gpl-3|GCC Runtime Library Exception에 따라 이 런타임을 포함한 결과물은 GPL이 아니라 우리 라이선스를 따른다. 예외 문장은 copyright 파일 안에 있다."
)

# 결과물 모양에 들어가는 인코더·디코더(configure의 구성 요소 이름)와 ffcheck가 찾는 코덱 이름
ENCODERS_CONFIGURE=(h264_nvenc hevc_nvenc av1_nvenc h264_amf hevc_amf av1_amf h264_qsv hevc_qsv av1_qsv libopenh264 libvpx_vp8 libvpx_vp9 libsvtav1 libopus)
DECODERS_CONFIGURE=(h264 hevc vp8 vp9 opus)
