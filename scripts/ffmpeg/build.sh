#!/usr/bin/env bash
# dongsi용 최소 LGPL ffmpeg(Windows x64 DLL)를 Docker 안에서 교차 빌드한다.
#
# 사용법(WSL이나 리눅스에서):
#   bash scripts/ffmpeg/build.sh [--out <결과 폴더>] [--work <작업 폴더>] [--jobs N] [--sources <미리 받은 소스 폴더>]
#
#   --out      결과물(bin/ lib/ include/ licenses/ source/ BUILDINFO.md)을 둘 폴더.
#              기본 <저장소>/third_party/ffmpeg-lgpl (third_party/는 git에서 제외됨)
#   --work     작업 폴더(받은 소스, 빌드 중간물). 기본 ${XDG_CACHE_HOME:-$HOME/.cache}/dongsi-ffmpeg-build
#   --jobs     병렬 수. 기본 6
#   --sources  미리 받은 소스 폴더. 주면 인터넷으로 내려받지 않는다(versions.sh의 파일 이름·SHA-256과 같아야 함).
#   --build-id 소스 묶음 이름에 쓰는 빌드 id(기본: 날짜-스크립트 해시)
#   --clean    작업 폴더의 중간물을 지우고 처음부터 빌드한다(받은 소스는 남김)
#   --no-check 빌드 뒤 ffcheck(DLL 확인 프로그램) 실행을 건너뛴다
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/../.." && pwd)

OUT=""
WORK=""
JOBS=6
SOURCES_ARG=""
BUILD_ID=""
CLEAN=0
RUN_CHECK=1

usage() { sed -n '2,15p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
die() { echo "오류: $*" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out) OUT=$2; shift 2 ;;
    --work) WORK=$2; shift 2 ;;
    --jobs) JOBS=$2; shift 2 ;;
    --sources) SOURCES_ARG=$2; shift 2 ;;
    --build-id) BUILD_ID=$2; shift 2 ;;
    --clean) CLEAN=1; shift ;;
    --no-check) RUN_CHECK=0; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "알 수 없는 인자: $1 (--help)" ;;
  esac
done

[[ $JOBS =~ ^[0-9]+$ && $JOBS -ge 1 ]] || die "--jobs는 1 이상의 숫자여야 한다: $JOBS"
command -v docker >/dev/null || die "docker가 없다"
docker info >/dev/null 2>&1 || die "docker를 쓸 수 없다(데몬이 꺼졌거나 권한이 없음. sudo 없이 docker 명령이 돌아야 한다)"

abspath() { # 없는 폴더도 절대 경로로
  mkdir -p "$1"; (cd "$1" && pwd)
}
[[ -n $OUT ]] || OUT="$REPO_ROOT/third_party/ffmpeg-lgpl"
[[ -n $WORK ]] || WORK="${XDG_CACHE_HOME:-$HOME/.cache}/dongsi-ffmpeg-build"
OUT=$(abspath "$OUT")
WORK=$(abspath "$WORK")
[[ $OUT != / && $WORK != / ]] || die "--out/--work가 루트다"
INNER=$WORK/inner   # 컨테이너의 /work

# shellcheck source=versions.sh
. "$SCRIPT_DIR/versions.sh"

# ---- 소스 준비 ----------------------------------------------------------------
if [[ -n $SOURCES_ARG ]]; then
  [[ -d $SOURCES_ARG ]] || die "--sources 폴더가 없다: $SOURCES_ARG"
  SOURCES=$(cd "$SOURCES_ARG" && pwd)
  OFFLINE=1
else
  SOURCES=$(abspath "$WORK/dl")
  OFFLINE=0
fi

echo "== 소스 확인($([[ $OFFLINE == 1 ]] && echo '내려받지 않음' || echo '없으면 내려받음')): $SOURCES"
for line in "${DOWNLOADS[@]}"; do
  IFS='|' read -r f url sha <<<"$line"
  if [[ ! -f $SOURCES/$f ]]; then
    [[ $OFFLINE == 0 ]] || die "--sources에 파일이 없다: $f"
    echo "내려받는 중: $f"
    curl -fL --retry 3 -sS -o "$SOURCES/$f.part" "$url"
    mv "$SOURCES/$f.part" "$SOURCES/$f"
  fi
  got=$(sha256sum "$SOURCES/$f" | cut -d' ' -f1)
  if [[ $got != "$sha" ]]; then
    die "SHA-256 불일치: $f (기대 $sha, 실제 $got). 빌드를 멈춘다."
  fi
done
echo "소스 ${#DOWNLOADS[@]}개 SHA-256 확인됨"

# ---- 빌더 이미지 --------------------------------------------------------------
DOCKERFILE_HASH=$(sha256sum "$SCRIPT_DIR/Dockerfile" | cut -c1-12)
IMAGE=dongsi-ffmpeg-builder:$DOCKERFILE_HASH
BASE_IMAGE=$(grep -m1 '^FROM ' "$SCRIPT_DIR/Dockerfile" | cut -d' ' -f2)
echo "== 빌더 이미지: $IMAGE (기반 $BASE_IMAGE)"
if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  docker build -t "$IMAGE" -f "$SCRIPT_DIR/Dockerfile" "$SCRIPT_DIR"
fi
BUILDER_IMAGE_ID=$(docker image inspect -f '{{.Id}}' "$IMAGE")

# ---- 빌드 id ------------------------------------------------------------------
if [[ -z $BUILD_ID ]]; then
  SCRIPT_HASH=$(cd "$SCRIPT_DIR" && find . -type f ! -name '*.pyc' ! -path './__pycache__/*' -print0 | sort -z | xargs -0 sha256sum | sha256sum | cut -c1-8)
  BUILD_ID=$(date -u +%Y%m%d)-$SCRIPT_HASH
fi
BUILD_DATE=$(date -u +%Y-%m-%dT%H:%M:%SZ)
: "${SOURCE_DATE_EPOCH:=$(date -u +%s)}"
echo "== 빌드 id: $BUILD_ID, 병렬 수: $JOBS, 작업 폴더: $WORK, 결과 폴더: $OUT"

# ---- 컨테이너 안 빌드 -----------------------------------------------------------
if [[ $CLEAN == 1 ]]; then
  echo "== --clean: 작업 폴더의 중간물을 지운다: $INNER"
  rm -rf "${INNER:?}"
fi
mkdir -p "$INNER"
T_START=$SECONDS
docker run --rm --network none \
  --user "$(id -u):$(id -g)" \
  -e JOBS="$JOBS" -e BUILD_ID="$BUILD_ID" -e BUILD_DATE="$BUILD_DATE" \
  -e BUILDER_IMAGE_ID="$BUILDER_IMAGE_ID" -e BASE_IMAGE="$BASE_IMAGE" \
  -e SOURCE_DATE_EPOCH="$SOURCE_DATE_EPOCH" \
  -v "$SCRIPT_DIR:/scripts:ro" -v "$SOURCES:/sources:ro" -v "$INNER:/work" \
  "$IMAGE" bash /scripts/inside-build.sh
echo "== 컨테이너 빌드 끝($((SECONDS - T_START))초)"

STAGE=$INNER/stage
[[ -f $STAGE/BUILDINFO.md && -d $STAGE/bin ]] || die "결과물이 없다: $STAGE"

# ---- 결과 폴더로 옮기기 ---------------------------------------------------------
echo "== 결과물 복사: $OUT"
for d in bin include lib licenses source; do
  [[ ! -L $OUT/$d ]] || die "$OUT/$d가 링크라 지우지 않는다. 직접 확인한다."
done
for d in bin include lib licenses source BUILDINFO.md; do rm -rf "${OUT:?}/$d"; done
cp -a "$STAGE/." "$OUT/"

# ---- ffcheck --------------------------------------------------------------------
CHECK_LOG=$WORK/ffcheck.txt
CHECK_RC=skipped
if [[ $RUN_CHECK == 1 ]]; then
  echo "== ffcheck 실행"
  CHK=$(mktemp -d "$WORK/ffcheck.XXXXXX")
  cp "$INNER/check/"* "$CHK/"
  if [[ -r /proc/sys/fs/binfmt_misc/WSLInterop || -r /proc/sys/fs/binfmt_misc/WSLInterop-late ]]; then
    set +e
    ( cd "$CHK" && ./ffcheck.exe ) | tr -d '\r' >"$CHECK_LOG"
    CHECK_RC=${PIPESTATUS[0]}
    set -e
  elif command -v wine >/dev/null; then
    set +e
    ( cd "$CHK" && wine ffcheck.exe ) | tr -d '\r' >"$CHECK_LOG"
    CHECK_RC=${PIPESTATUS[0]}
    set -e
  else
    echo "ffcheck.exe를 돌릴 수단(WSL interop, wine)이 없어 건너뛴다. Windows에서 $INNER/check/ffcheck.exe를 직접 돌린다." | tee "$CHECK_LOG"
  fi
  rm -rf "${CHK:?}"
  cat "$CHECK_LOG"
  {
    echo
    echo "## 실행 확인 (ffcheck.exe, 종료 코드 $CHECK_RC)"
    echo
    echo "결과 DLL을 같은 폴더에서 불러 avcodec_configuration()·*_license()·인코더 존재를 확인한 출력이다. ffcheck.exe는 결과물에 넣지 않는다(scripts/ffmpeg/ffcheck.c)."
    echo
    echo '```'
    cat "$CHECK_LOG"
    echo '```'
  } >>"$OUT/BUILDINFO.md"
fi

echo
echo "== 끝. 결과 폴더: $OUT"
( cd "$OUT" && du -sh bin include lib licenses source | cat && ls -l bin source )
[[ $CHECK_RC == 0 || $CHECK_RC == skipped ]] || die "ffcheck 실패(종료 코드 $CHECK_RC). $CHECK_LOG를 본다."
