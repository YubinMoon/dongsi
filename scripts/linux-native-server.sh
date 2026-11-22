#!/usr/bin/env bash
# 리눅스(WSL 포함)에서 dongsi-server를 백그라운드로 띄우고 끄는 도우미.
# E2E 시험 스크립트(scripts/e2e-linux-server.ps1)와 손으로 하는 시험이 같이 쓴다.
#
# 사용법:
#   linux-native-server.sh start|stop|restart|status [옵션]
#
# 옵션:
#   --name <이름>   서버 이름(기본 e2e-native)
#   --port <번호>   서버가 듣는 TCP·UDP 포트(기본 7884)
#   --data <폴더>   인증서·설정·pid·로그를 둘 폴더(기본 ./dongsi-data)
#   --bin <경로>    dongsi-server 실행 파일(기본 <저장소>/target/release/dongsi-server)
#
# 동작:
#   - pid 파일(<데이터>/dongsi-server.pid)과 로그(<데이터>/dongsi-server.log)는 데이터 폴더 안에 둔다.
#   - start는 로그에 초대 코드가 나올 때까지(30초) 기다리고, 초대 코드 한 줄만 표준 출력에 쓴다.
#     나머지 안내는 표준 오류로 간다. 이미 떠 있으면 새로 띄우지 않고 초대 코드만 쓴다.
#   - stop은 SIGTERM을 보내고 15초까지 종료를 기다린다. 안 죽으면 SIGKILL하고 경고한다.
#   - pid 파일의 프로세스가 이 실행 파일이 아니면 건드리지 않는다(pid 파일만 지운다).
#   - start 때 쓴 --name/--port/--bin은 <데이터>/native-server.args에 적어 두어,
#     stop·restart·status에서 다시 안 줘도 된다.
#   - 종료 코드: 0 정상, 1 실패, 2 사용법 오류. status는 떠 있으면 0, 아니면 3.

set -u

SELF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SELF_DIR/.." && pwd)"

NAME=""
PORT=""
DATA=""
BIN=""
INVITE_WAIT=30
STOP_WAIT=15

die() { echo "오류: $*" >&2; exit 1; }
note() { echo "[native] $*" >&2; }

usage() {
  sed -n '2,/^$/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
  exit 2
}

[ $# -ge 1 ] || usage
CMD="$1"; shift
case "$CMD" in start|stop|restart|status) ;; -h|--help) usage ;; *) echo "알 수 없는 명령: $CMD" >&2; usage ;; esac

while [ $# -gt 0 ]; do
  case "$1" in
    --name) [ $# -ge 2 ] || usage; NAME="$2"; shift 2 ;;
    --port) [ $# -ge 2 ] || usage; PORT="$2"; shift 2 ;;
    --data) [ $# -ge 2 ] || usage; DATA="$2"; shift 2 ;;
    --bin)  [ $# -ge 2 ] || usage; BIN="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "알 수 없는 옵션: $1" >&2; usage ;;
  esac
done

[ -n "$DATA" ] || DATA="./dongsi-data"
mkdir -p "$DATA" || die "데이터 폴더를 만들 수 없습니다: $DATA"
DATA="$(cd "$DATA" && pwd)"
PID_FILE="$DATA/dongsi-server.pid"
LOG_FILE="$DATA/dongsi-server.log"
ARGS_FILE="$DATA/native-server.args"

# 이전 start가 적어 둔 값을 채운다(명령줄이 우선). 한 줄에 KEY=값, 첫 '='로만 나눈다.
if [ -f "$ARGS_FILE" ]; then
  while IFS= read -r line; do
    key="${line%%=*}"; val="${line#*=}"
    case "$key" in
      name) [ -n "$NAME" ] || NAME="$val" ;;
      port) [ -n "$PORT" ] || PORT="$val" ;;
      bin)  [ -n "$BIN" ] || BIN="$val" ;;
    esac
  done < "$ARGS_FILE"
fi
[ -n "$NAME" ] || NAME="e2e-native"
[ -n "$PORT" ] || PORT="7884"
[ -n "$BIN" ] || BIN="$REPO_DIR/target/release/dongsi-server"

case "$PORT" in ''|*[!0-9]*) die "포트가 숫자가 아닙니다: $PORT" ;; esac

BIN_REAL="$(readlink -f -- "$BIN" 2>/dev/null || echo "$BIN")"

# pid 파일의 pid가 살아 있고 이 실행 파일이면 pid를 출력하고 0을 돌려준다.
# 살아 있지만 다른 프로세스면 2, 없으면 1.
running_pid() {
  [ -f "$PID_FILE" ] || return 1
  local pid exe
  pid="$(tr -dc '0-9' < "$PID_FILE")"
  [ -n "$pid" ] || return 1
  kill -0 "$pid" 2>/dev/null || return 1
  exe="$(readlink -f "/proc/$pid/exe" 2>/dev/null || true)"
  exe="${exe% (deleted)}"
  if [ "$exe" = "$BIN_REAL" ] || [ "${exe##*/}" = "${BIN_REAL##*/}" ]; then
    echo "$pid"; return 0
  fi
  return 2
}

# 로그에서 가장 마지막 초대 코드(색 코드를 지운 뒤)를 쓴다. 없으면 1.
find_invite() {
  [ -f "$LOG_FILE" ] || return 1
  local code
  code="$(sed 's/\x1b\[[0-9;]*m//g' "$LOG_FILE" | grep -o 'dongsi1:[A-Za-z0-9_-]\+' | tail -n 1)"
  [ -n "$code" ] || return 1
  echo "$code"
}

do_start() {
  local pid rc i code
  pid="$(running_pid)"; rc=$?
  if [ $rc -eq 0 ]; then
    note "이미 떠 있습니다(pid $pid)"
    find_invite || die "로그에서 초대 코드를 찾지 못했습니다: $LOG_FILE"
    return 0
  elif [ $rc -eq 2 ]; then
    note "경고: pid 파일의 프로세스가 dongsi-server가 아니어서 건드리지 않습니다(pid 파일만 지움)"
  fi
  rm -f "$PID_FILE"

  [ -x "$BIN" ] || die "실행 파일이 없습니다: $BIN (cargo build -p dongsi-server-app --release)"

  printf 'name=%s\nport=%s\nbin=%s\n' "$NAME" "$PORT" "$BIN" > "$ARGS_FILE"
  [ -f "$LOG_FILE" ] && mv -f "$LOG_FILE" "$LOG_FILE.1"
  : > "$LOG_FILE"

  # setsid로 새 세션에서 띄운다(이 셸이 끝나도, wsl.exe가 끝나도 살아남는다).
  # 안쪽 셸이 자기 pid를 적고 exec하므로 pid 파일은 서버 프로세스의 pid다.
  setsid bash -c 'echo $$ > "$1"; shift; exec "$@"' _ "$PID_FILE" \
    "$BIN" --data "$DATA" --port "$PORT" --name "$NAME" \
    >> "$LOG_FILE" 2>&1 < /dev/null &

  for ((i = 0; i < INVITE_WAIT * 10; i++)); do
    if code="$(find_invite)"; then
      note "시작함(pid $(cat "$PID_FILE" 2>/dev/null), 포트 $PORT, 데이터 $DATA)"
      echo "$code"
      return 0
    fi
    running_pid >/dev/null; rc=$?
    if [ $i -gt 20 ] && [ $rc -ne 0 ]; then
      note "서버가 시작하자마자 끝났습니다. 로그 끝부분:"
      tail -n 20 "$LOG_FILE" >&2
      rm -f "$PID_FILE"
      return 1
    fi
    sleep 0.1
  done
  note "${INVITE_WAIT}초 안에 초대 코드가 나오지 않았습니다. 로그 끝부분:"
  tail -n 20 "$LOG_FILE" >&2
  return 1
}

do_stop() {
  local pid rc sig=TERM i
  pid="$(running_pid)"; rc=$?
  if [ $rc -eq 1 ]; then
    note "떠 있지 않습니다"
    rm -f "$PID_FILE"
    return 0
  elif [ $rc -eq 2 ]; then
    note "경고: pid 파일의 프로세스가 dongsi-server가 아니어서 건드리지 않습니다(pid 파일만 지움)"
    rm -f "$PID_FILE"
    return 0
  fi
  note "SIG$sig를 보냅니다(pid $pid)"
  kill -s "$sig" "$pid" 2>/dev/null || true
  for ((i = 0; i < STOP_WAIT * 10; i++)); do
    if ! kill -0 "$pid" 2>/dev/null; then
      rm -f "$PID_FILE"
      note "종료함(약 $((i / 10)).$((i % 10))초)"
      return 0
    fi
    sleep 0.1
  done
  note "경고: ${STOP_WAIT}초 안에 끝나지 않아 SIGKILL합니다(pid $pid). 정상 종료가 안 되는 것입니다"
  kill -s KILL "$pid" 2>/dev/null || true
  sleep 0.5
  rm -f "$PID_FILE"
  return 0
}

do_status() {
  local pid rc code
  pid="$(running_pid)"; rc=$?
  if [ $rc -eq 0 ]; then
    echo "떠 있음 pid=$pid 포트=$PORT 데이터=$DATA"
    if code="$(find_invite)"; then echo "초대 코드: $code"; fi
    return 0
  fi
  echo "떠 있지 않음 데이터=$DATA"
  return 3
}

case "$CMD" in
  start) do_start ;;
  stop) do_stop ;;
  restart) do_stop && do_start ;;
  status) do_status ;;
esac
