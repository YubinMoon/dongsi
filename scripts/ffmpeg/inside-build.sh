#!/usr/bin/env bash
# 빌더 컨테이너 안에서 실행된다(build.sh가 호출). 직접 실행하지 않는다.
# 마운트: /scripts(읽기 전용, scripts/ffmpeg), /sources(읽기 전용, 받아 둔 소스), /work(작업 폴더)
# 네트워크는 꺼져 있다(--network none). 결과는 /work/stage에 모양대로 만들어진다.
set -euo pipefail

: "${JOBS:=6}"
: "${BUILD_ID:?}"
: "${BUILD_DATE:?}"
: "${BUILDER_IMAGE_ID:=unknown}"
: "${BASE_IMAGE:=unknown}"
: "${SOURCE_DATE_EPOCH:?}"

# shellcheck source=versions.sh
. /scripts/versions.sh

W=/work
SOURCES=/sources
SRC=$W/src          # 압축을 푼 소스
BLD=$W/build        # 빌드 폴더
DEPS=$W/deps        # 외부 라이브러리 설치 위치(정적 라이브러리와 헤더)
FFP=$W/ffprefix     # ffmpeg 설치 위치
STAGE=$W/stage      # 최종 결과물 모양
STAMPS=$W/stamps
FACTS=$W/facts
CHECK=$W/check

export HOME=/tmp
export TZ=UTC
export LC_ALL=C.UTF-8
# 우리가 만든 .pc만 보게 한다(빌더 이미지의 리눅스용 .pc가 섞이지 않게)
export PKG_CONFIG_LIBDIR=$DEPS/lib/pkgconfig
export PKG_CONFIG_PATH=$DEPS/lib/pkgconfig

HOST=x86_64-w64-mingw32
export CC=$HOST-gcc CXX=$HOST-g++ AR=$HOST-ar RANLIB=$HOST-ranlib STRIP=$HOST-strip
export NM=$HOST-nm DLLTOOL=$HOST-dlltool WINDRES=$HOST-windres OBJDUMP=$HOST-objdump

log() { printf '\n==== [%s] %s\n' "$(date +%H:%M:%S)" "$*"; }

mkdir -p "$SRC" "$BLD" "$DEPS/lib/pkgconfig" "$DEPS/include" "$STAMPS" "$FACTS"
rm -rf "$STAGE" "$FFP" "$CHECK"
mkdir -p "$STAGE" "$CHECK"

# ---- 1. 받은 소스 확인 --------------------------------------------------------
log "소스 SHA-256 확인"
declare -A FILE_SHA FILE_URL
for line in "${DOWNLOADS[@]}"; do
  IFS='|' read -r f url sha <<<"$line"
  FILE_SHA[$f]=$sha
  FILE_URL[$f]=$url
  [[ -f $SOURCES/$f ]] || { echo "소스가 없다: $SOURCES/$f" >&2; exit 1; }
  got=$(sha256sum "$SOURCES/$f" | cut -d' ' -f1)
  [[ $got == "$sha" ]] || { echo "SHA-256 불일치: $f (기대 $sha, 실제 $got)" >&2; exit 1; }
done
echo "소스 ${#DOWNLOADS[@]}개 확인됨"

# ---- 2. 도구 버전 기록 --------------------------------------------------------
{
  echo "gcc: $($HOST-gcc --version | head -1)  [$($HOST-gcc -v 2>&1 | grep 'Thread model')]"
  echo "g++: $($HOST-g++ --version | head -1)"
  echo "binutils(ld): $($HOST-ld -v)"
  echo "mingw-w64: $(dpkg-query -W -f='${Package} ${Version}' mingw-w64-x86-64-dev)"
  echo "gcc-mingw-w64 패키지: $(dpkg-query -W -f='${Package} ${Version}' gcc-mingw-w64-x86-64-posix)"
  echo "binutils-mingw-w64 패키지: $(dpkg-query -W -f='${Package} ${Version}' binutils-mingw-w64-x86-64)"
  echo "nasm: $(nasm -v)"
  echo "cmake: $(cmake --version | head -1)"
  echo "meson: $(meson --version)"
  echo "ninja: $(ninja --version)"
  echo "make: $(make --version | head -1)"
  echo "pkg-config: $(pkg-config --version)"
  echo "python3: $(python3 --version)"
  echo "빌더 기반 이미지: $BASE_IMAGE"
  echo "빌더 이미지 ID: $BUILDER_IMAGE_ID"
} >"$FACTS/tools.txt"
cat "$FACTS/tools.txt"

# ---- 도우미 -------------------------------------------------------------------
fileof() { # 구성 요소 id → 소스 파일 이름
  local line id f
  for line in "${COMPONENTS[@]}"; do
    IFS='|' read -r id _ _ _ _ f _ <<<"$line"
    [[ $id == "$1" ]] && { echo "$f"; return; }
  done
  return 1
}
extract() { # extract <구성 요소 id> [폴더 이름]: 최상위 폴더 한 단계를 벗겨 $SRC/<이름>에 푼다
  local id=$1 name=${2:-$1} f
  f=$(fileof "$id")
  rm -rf "${SRC:?}/$name"
  mkdir -p "$SRC/$name"
  tar -xf "$SOURCES/$f" -C "$SRC/$name" --strip-components=1
}
# 단계 도장: 같은 소스(SHA)·같은 스크립트로 이미 끝낸 외부 라이브러리 단계는 건너뛴다
stamp_name() { # <id> <함수>: 소스 SHA-256과 그 단계 함수의 본문이 같으면 같은 도장
  echo "$STAMPS/$1.$( { echo "${FILE_SHA[$(fileof "$1")]}"; declare -f "$2"; cat "$W/mingw-toolchain.cmake"; cat /scripts/Dockerfile; } | sha256sum | cut -c1-12)"
}
step() { # step <id> <함수>
  local st; st=$(stamp_name "$1" "$2")
  if [[ -f $st ]]; then log "$1: 이미 끝남(도장 있음), 건너뜀"; return; fi
  log "$1: 시작"
  local t0=$SECONDS
  "$2"
  touch "$st"
  log "$1: 끝($((SECONDS - t0))초)"
}

# CMake 교차 컴파일 도구 체인 파일
cat >"$W/mingw-toolchain.cmake" <<'EOF'
set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR x86_64)
set(CMAKE_C_COMPILER x86_64-w64-mingw32-gcc)
set(CMAKE_CXX_COMPILER x86_64-w64-mingw32-g++)
set(CMAKE_RC_COMPILER x86_64-w64-mingw32-windres)
set(CMAKE_AR x86_64-w64-mingw32-ar)
set(CMAKE_RANLIB x86_64-w64-mingw32-ranlib)
set(CMAKE_FIND_ROOT_PATH /usr/x86_64-w64-mingw32)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
EOF

# ---- 3. 외부 라이브러리 -------------------------------------------------------
build_nv_codec_headers() {
  extract nv-codec-headers
  make -C "$SRC/nv-codec-headers" PREFIX="$DEPS" install
}

build_amf() {
  extract amf
  rm -rf "$DEPS/include/AMF"
  cp -r "$SRC/amf/AMF" "$DEPS/include/AMF"
}

build_opus() {
  extract opus
  rm -rf "$BLD/opus"; mkdir -p "$BLD/opus"; cd "$BLD/opus"
  "$SRC/opus/configure" --host=$HOST --prefix="$DEPS" \
    --disable-shared --enable-static --disable-doc --disable-extra-programs \
    --disable-hardening CFLAGS="-O2"
  make -j"$JOBS"
  make install
}

build_libvpx() {
  extract libvpx
  rm -rf "$BLD/libvpx"; mkdir -p "$BLD/libvpx"; cd "$BLD/libvpx"
  CROSS=$HOST- "$SRC/libvpx/configure" --target=x86_64-win64-gcc --prefix="$DEPS" \
    --as=nasm --disable-shared --enable-static \
    --enable-vp8 --enable-vp9 --disable-vp8-decoder --disable-vp9-decoder \
    --enable-runtime-cpu-detect --enable-multithread \
    --disable-examples --disable-tools --disable-docs --disable-unit-tests \
    --disable-install-bins --disable-install-docs
  make -j"$JOBS"
  make install
}

build_svt_av1() {
  extract svt-av1
  rm -rf "$BLD/svt-av1"
  cmake -S "$SRC/svt-av1" -B "$BLD/svt-av1" -G Ninja \
    -DCMAKE_TOOLCHAIN_FILE="$W/mingw-toolchain.cmake" \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$DEPS" -DCMAKE_INSTALL_LIBDIR=lib \
    -DBUILD_SHARED_LIBS=OFF -DBUILD_DEC=OFF -DBUILD_APPS=OFF -DBUILD_TESTING=OFF \
    -DSVT_AV1_LTO=OFF -DENABLE_AVX512=OFF -DEXCLUDE_HASH=ON -DREPRODUCIBLE_BUILDS=ON
  cmake --build "$BLD/svt-av1" -j"$JOBS"
  cmake --install "$BLD/svt-av1"
}

build_openh264() {
  rm -rf "$BLD/openh264"; mkdir -p "$BLD"
  extract openh264
  cp -a "$SRC/openh264" "$BLD/openh264"   # Makefile이 소스 안에서 빌드한다
  cd "$BLD/openh264"
  make -j"$JOBS" OS=mingw_nt ARCH=x86_64 CC=$CC CXX=$CXX AR=$AR PREFIX="$DEPS" \
    ASM=nasm USE_ASM=Yes BUILDTYPE=Release install-static
}

build_libvpl() {
  extract libvpl
  rm -rf "$BLD/libvpl"
  cmake -S "$SRC/libvpl" -B "$BLD/libvpl" -G Ninja \
    -DCMAKE_TOOLCHAIN_FILE="$W/mingw-toolchain.cmake" \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$DEPS" -DCMAKE_INSTALL_LIBDIR=lib \
    -DBUILD_SHARED_LIBS=OFF -DBUILD_TESTS=OFF -DBUILD_EXAMPLES=OFF -DBUILD_PREVIEW=OFF \
    -DBUILD_EXPERIMENTAL=OFF -DINSTALL_EXAMPLE_CODE=OFF -DBUILD_TOOLS=OFF
  cmake --build "$BLD/libvpl" -j"$JOBS"
  cmake --install "$BLD/libvpl"
}

step nv-codec-headers build_nv_codec_headers
step amf build_amf
step opus build_opus
step libvpx build_libvpx
step svt-av1 build_svt_av1
step openh264 build_openh264
step libvpl build_libvpl

log "외부 라이브러리 설치 결과"
ls -la "$DEPS/lib" "$DEPS/lib/pkgconfig"
for p in openh264 vpx SvtAv1Enc opus vpl ffnvcodec; do
  echo "--- $p.pc"; pkg-config --static --libs --cflags "$p" || true
done

# ---- 4. FFmpeg ----------------------------------------------------------------
build_ffmpeg() {
  extract ffmpeg
  rm -rf "$BLD/ffmpeg"; mkdir -p "$BLD/ffmpeg"; cd "$BLD/ffmpeg"

  local enc_args=() dec_args=() e d
  for e in "${ENCODERS_CONFIGURE[@]}"; do enc_args+=("--enable-encoder=$e"); done
  for d in "${DECODERS_CONFIGURE[@]}"; do dec_args+=("--enable-decoder=$d"); done

  # GPL·version3·nonfree는 일부러 넣지 않는다(LGPL 2.1 이상).
  # --disable-autodetect는 재현성을 위해 켠다. 그러면 스레드·d3d11va·dxva2도 꺼지니 아래에서 명시한다.
  "$SRC/ffmpeg/configure" \
    --prefix=/ffmpeg-lgpl \
    --pkg-config=pkg-config --pkg-config-flags=--static \
    --target-os=mingw32 --arch=x86_64 --cross-prefix=$HOST- \
    --enable-shared --disable-static --disable-debug \
    --disable-programs --disable-doc --disable-avformat --disable-avdevice --disable-avfilter \
    --disable-swresample --disable-network \
    --disable-autodetect \
    --enable-w32threads --enable-d3d11va --enable-dxva2 \
    --enable-ffnvcodec --enable-nvenc --enable-amf --enable-libvpl \
    --enable-libopenh264 --enable-libvpx --enable-libsvtav1 --enable-libopus \
    --disable-everything "${enc_args[@]}" "${dec_args[@]}" \
    --extra-ldflags="-static -static-libgcc -static-libstdc++ -Wl,--no-insert-timestamp" \
    --extra-libs="-lstdc++ -lwinpthread" \
    || { echo "configure 실패. ffbuild/config.log 끝:"; tail -60 ffbuild/config.log; exit 1; }

  # autodetect를 끄면 스레드가 꺼진다. 켜졌는지 확인한다.
  grep -E '^#define HAVE_THREADS 1' config.h
  grep -E '^#define HAVE_W32THREADS 1' config.h
  grep -E '^#define CONFIG_(GPL|VERSION3|NONFREE) ' config.h
  [[ $(grep -cE '^#define CONFIG_(GPL|VERSION3|NONFREE) 0' config.h) == 3 ]] \
    || { echo "GPL/VERSION3/NONFREE가 꺼져 있지 않다" >&2; exit 1; }
  grep -E '^#define FFMPEG_CONFIGURATION ' config.h | sed -E 's/^#define FFMPEG_CONFIGURATION "(.*)"$/\1/' >"$FACTS/configure.txt"
  cp config.h "$FACTS/config.h"

  make -j"$JOBS"
  make install DESTDIR="$FFP"
}

log "ffmpeg: 시작"
T0=$SECONDS
build_ffmpeg
log "ffmpeg: 끝($((SECONDS - T0))초)"

# ---- 5. 결과물 모양 -----------------------------------------------------------
log "결과물 정리"
FFROOT=$FFP/ffmpeg-lgpl
mkdir -p "$STAGE/bin" "$STAGE/lib" "$STAGE/include"
for dll in "${FFMPEG_DLLS[@]}"; do
  [[ -f $FFROOT/bin/$dll ]] || { echo "DLL이 없다: $dll" >&2; ls -la "$FFROOT/bin"; exit 1; }
  cp "$FFROOT/bin/$dll" "$STAGE/bin/$dll"
  $STRIP --strip-unneeded "$STAGE/bin/$dll"
done
cp -a "$FFROOT/include/." "$STAGE/include/"
cp -a "$FFROOT/lib/." "$STAGE/lib/"
# .def가 설치되지 않았으면 DLL에서 뽑는다. MSVC 링커용 가져오기 라이브러리(.lib)는 .def로 만든다.
for dll in "${FFMPEG_DLLS[@]}"; do
  base=${dll%%-*}
  def=$STAGE/lib/${dll%.dll}.def
  if [[ ! -f $def ]]; then
    ( cd "$W" && rm -f "$(basename "$def")" && gendef "$FFROOT/bin/$dll" && mv "${dll%.dll}.def" "$def" )
  fi
  $DLLTOOL -m i386:x86-64 -d "$def" -D "$dll" -l "$STAGE/lib/$base.lib"
done
find "$STAGE/lib" -maxdepth 1 -type f | sort | sed "s|$STAGE/||"

# DLL 가져오기 목록(시스템 DLL과 ffmpeg DLL만 있어야 한다)
: >"$FACTS/imports.txt"
for dll in "${FFMPEG_DLLS[@]}"; do
  {
    echo "## $dll"
    $OBJDUMP -p "$STAGE/bin/$dll" | awk '/DLL Name:/ {print $3}' | sort -u | tr '\n' ' '
    echo
  } >>"$FACTS/imports.txt"
done
cat "$FACTS/imports.txt"
# msvcrt.dll은 윈도우 시스템 DLL이라 허용한다(우분투 mingw-w64는 msvcrt 기본 빌드. BtbN은 UCRT)
bad=$(grep -v '^##' "$FACTS/imports.txt" | tr ' ' '\n' | grep -iE '^(libgcc|libstdc|libwinpthread|libssp)' || true)
[[ -z $bad ]] || { echo "허용하지 않는 DLL을 가져온다: $bad" >&2; exit 1; }
for dll in "${FFMPEG_DLLS[@]}"; do
  stat -c '%n %s' "$STAGE/bin/$dll"
done >"$FACTS/sizes.txt"
cat "$FACTS/sizes.txt"

# ---- 6. ffcheck(확인 프로그램, 결과물에는 넣지 않는다) ------------------------
log "ffcheck 빌드"
$CC -O1 -static -I"$STAGE/include" /scripts/ffcheck.c -o "$CHECK/ffcheck.exe" \
  -L"$STAGE/lib" -lavcodec -lavutil -lswscale
cp "$STAGE/bin/"*.dll "$CHECK/"

# ---- 7. 라이선스 파일 ---------------------------------------------------------
log "라이선스 파일 모으기"
license_from_special() { # @이름 → 원본 파일 경로
  case "$1" in
    @AMF-LICENSE-v1.4.36.txt) echo "$SOURCES/AMF-LICENSE-v1.4.36.txt" ;;
    @mingw-w64-common-copyright) echo /usr/share/doc/mingw-w64-common/copyright ;;
    @gcc-mingw-w64-base-copyright) echo /usr/share/doc/gcc-mingw-w64-base/copyright ;;
    @gpl-3) echo /usr/share/common-licenses/GPL-3 ;;
    *) echo "알 수 없는 특수 라이선스 위치: $1" >&2; return 1 ;;
  esac
}
rm -rf "$STAGE/licenses"; mkdir -p "$STAGE/licenses"
: >"$W/license-files.txt"   # id|<licenses/ 아래 경로 공백 구분>
for line in "${COMPONENTS[@]}"; do
  IFS='|' read -r id _ _ _ _ f _ files _ <<<"$line"
  mkdir -p "$STAGE/licenses/$id"
  out=()
  for lf in $files; do
    if [[ $lf == @* ]]; then
      src=$(license_from_special "$lf"); dst=${lf#@}
      cp "$src" "$STAGE/licenses/$id/$dst"
    else
      [[ -d $SRC/$id ]] || extract "$id"
      src=$SRC/$id/$lf; dst=$(basename "$lf")
      [[ -f $src ]] || { echo "라이선스 파일이 없다: $src" >&2; exit 1; }
      if [[ $lf == *.h ]]; then
        dst=$dst.license.txt
        # 헤더 머리의 첫 주석 블록(/* ... */)만 뽑는다
        awk '{print} /\*\//{exit}' "$src" >"$STAGE/licenses/$id/$dst"
      else
        cp "$src" "$STAGE/licenses/$id/$dst"
      fi
    fi
    out+=("$id/$dst")
  done
  echo "$id|${out[*]}" >>"$W/license-files.txt"
done
find "$STAGE/licenses" -type f | sort | sed "s|$STAGE/||"

# ---- 8. 소스 묶음 -------------------------------------------------------------
log "소스 묶음"
BUNDLE_NAME=dongsi-ffmpeg-lgpl-$FFMPEG_VERSION-$BUILD_ID-src
BUNDLE_DIR=$W/bundle/$BUNDLE_NAME
rm -rf "$W/bundle"; mkdir -p "$BUNDLE_DIR/sources" "$BUNDLE_DIR/scripts/ffmpeg"
for line in "${DOWNLOADS[@]}"; do
  IFS='|' read -r f _ _ <<<"$line"
  cp "$SOURCES/$f" "$BUNDLE_DIR/sources/$f"
done
cp -a /scripts/. "$BUNDLE_DIR/scripts/ffmpeg/"
echo "$BUILD_ID" >"$BUNDLE_DIR/BUILD_ID"
cp /scripts/REBUILD.md "$BUNDLE_DIR/REBUILD.md"
( cd "$BUNDLE_DIR" && find sources scripts -type f -print0 | sort -z | xargs -0 sha256sum >SHA256SUMS )
# 파일 권한을 고정한다(윈도우 드라이브 마운트는 전부 rwx로 보이므로 어디서 빌드해도 같은 tar가 나오게)
find "$BUNDLE_DIR" -type d -exec chmod 755 {} +
find "$BUNDLE_DIR" -type f -exec chmod 644 {} +
find "$BUNDLE_DIR/scripts" -type f \( -name '*.sh' -o -name '*.py' \) -exec chmod 755 {} +
mkdir -p "$STAGE/source"
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$SOURCE_DATE_EPOCH" \
  -cf "$STAGE/source/$BUNDLE_NAME.tar" -C "$W/bundle" "$BUNDLE_NAME"
( cd "$STAGE/source" && sha256sum "$BUNDLE_NAME.tar" >"$BUNDLE_NAME.tar.sha256" )
ls -la "$STAGE/source"
echo "$BUNDLE_NAME.tar" >"$FACTS/bundle-name.txt"
cut -d' ' -f1 "$STAGE/source/$BUNDLE_NAME.tar.sha256" >"$FACTS/bundle-sha256.txt"

# ---- 9. components.json, BUILDINFO.md ----------------------------------------
log "메타데이터"
printf '%s\n' "${COMPONENTS[@]}" >"$FACTS/components.txt"
printf '%s\n' "${DOWNLOADS[@]}" >"$FACTS/downloads.txt"
printf '%s\n' "${ENCODERS_CONFIGURE[@]}" >"$FACTS/encoders.txt"
printf '%s\n' "${DECODERS_CONFIGURE[@]}" >"$FACTS/decoders.txt"
python3 -I /scripts/gen-metadata.py \
  --facts "$FACTS" --work "$W" --stage "$STAGE" \
  --ffmpeg-version "$FFMPEG_VERSION" --build-id "$BUILD_ID" --build-date "$BUILD_DATE"

log "완료"
