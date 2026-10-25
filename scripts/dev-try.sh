#!/bin/sh
# Start shardraild and the browser UI for one local try.
# Prints a URL and an enrollment code. Does not open a browser.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
common=$(git -C "$ROOT" rev-parse --git-common-dir) || {
  echo "cannot resolve the Shardrail checkout" >&2
  exit 1
}
case "$common" in
  /*) ;;
  *) common=$ROOT/$common ;;
esac
SHARDRAIL=$(CDPATH= cd -- "$(dirname -- "$common")" && pwd -P) || {
  echo "cannot resolve the Shardrail checkout" >&2
  exit 1
}
case "$SHARDRAIL" in
  /System/Volumes/Data/*)
    logical=/${SHARDRAIL#/System/Volumes/Data/}
    if [ -d "$logical" ]; then
      SHARDRAIL=$logical
    fi
    ;;
esac
PORT=${SHARDRAIL_TRY_PORT:-9731}
WORKDIR=${SHARDRAIL_SESSION_WORKDIR:-$SHARDRAIL/spikes/s1-testrepo}
CODEX=${SHARDRAIL_TRY_CODEX:-}
SKIP_BUILD=0
DRY_RUN=0
if [ "${SHARDRAIL_TRY_SKIP_BUILD:-}" = 1 ]; then
  SKIP_BUILD=1
fi

while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY_RUN=1 ;;
    --skip-build) SKIP_BUILD=1 ;;
    --codex)
      if [ $# -lt 2 ]; then
        echo "--codex needs a path" >&2
        exit 2
      fi
      CODEX=$2
      shift
      ;;
    --port)
      if [ $# -lt 2 ]; then
        echo "--port needs a number" >&2
        exit 2
      fi
      PORT=$2
      shift
      ;;
    --workdir)
      if [ $# -lt 2 ]; then
        echo "--workdir needs a directory" >&2
        exit 2
      fi
      WORKDIR=$2
      shift
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 2
      ;;
  esac
  shift
done

case "$PORT" in
  ''|*[!0-9]*)
    echo "port must be a number" >&2
    exit 2
    ;;
esac

if [ -z "$CODEX" ]; then
  CODEX=$(command -v codex || true)
fi
if [ -n "$CODEX" ]; then
  case "$CODEX" in
    /*) ;;
    *)
      CODEX=$(CDPATH= cd -- "$(dirname -- "$CODEX")" && pwd)/$(basename -- "$CODEX")
      ;;
  esac
fi
if [ -z "$CODEX" ] || [ ! -x "$CODEX" ]; then
  echo "codex is missing. Install the Codex CLI or pass --codex PATH." >&2
  exit 1
fi

port_busy() {
  python3 -c '
import socket, sys
port = int(sys.argv[1])
sock = socket.socket()
sock.settimeout(0.2)
try:
    busy = sock.connect_ex(("127.0.0.1", port)) == 0
finally:
    sock.close()
sys.exit(0 if busy else 1)
' "$1"
}

if port_busy "$PORT"; then
  echo "127.0.0.1:$PORT is already in use." >&2
  exit 1
fi

BIN="$ROOT/target/debug/shardraild"
ASSETS="$ROOT/apps/s5-prototype/dist"
DATA=
DAEMON_PID=

cleanup() {
  # Closing the write end makes --exit-with-stdin stop the daemon. TERM is the
  # backup when the signal is one this script can still catch.
  exec 3>&- 2>/dev/null || true
  if [ -n "$DAEMON_PID" ]; then
    kill -TERM "$DAEMON_PID" 2>/dev/null || true
    wait "$DAEMON_PID" 2>/dev/null || true
    DAEMON_PID=
  fi
  if [ -n "$DATA" ]; then
    rm -rf "$DATA"
    DATA=
  fi
}

on_signal() {
  cleanup
  exit 130
}

trap on_signal INT TERM HUP
trap cleanup EXIT

if [ "$SKIP_BUILD" -eq 0 ]; then
  if [ "$DRY_RUN" -eq 1 ]; then
    echo "nice cargo build -p shardraild"
    if [ ! -d "$ROOT/apps/s5-prototype/node_modules" ]; then
      echo "npm --prefix \"$ROOT/apps/s5-prototype\" ci"
    fi
    echo "npm --prefix \"$ROOT/apps/s5-prototype\" run build"
  else
    (cd "$ROOT" && nice cargo build -p shardraild)
    if [ ! -d "$ROOT/apps/s5-prototype/node_modules" ]; then
      npm --prefix "$ROOT/apps/s5-prototype" ci
    fi
    npm --prefix "$ROOT/apps/s5-prototype" run build
  fi
fi

if [ "$DRY_RUN" -eq 1 ]; then
  echo "DATA=\$(mktemp -d)"
  echo "\"$BIN\" --data-dir \"\$DATA\" --no-file-log --exit-with-stdin --codex-app-server \"$CODEX\" --session-workdir \"$WORKDIR\" --codex-config 'model_reasoning_effort=\"low\"' --browser-listen 127.0.0.1:$PORT --browser-assets-dir \"$ASSETS\""
  echo "\"$BIN\" --data-dir \"\$DATA\" client enroll --label browser --allow-session-create"
  echo "prints url=http://127.0.0.1:$PORT/ and enrollment_code. Does not open a browser."
  exit 0
fi

if [ ! -x "$BIN" ]; then
  echo "shardraild is not built at $BIN. Run without --skip-build." >&2
  exit 1
fi
if [ ! -d "$ASSETS" ]; then
  echo "S5 assets are missing at $ASSETS. Run without --skip-build." >&2
  exit 1
fi

DATA=$(mktemp -d "${TMPDIR:-/tmp}/shardrail-try.XXXXXX")
mkfifo "$DATA/parent.stdin"
# Hold the write end. The daemon does not inherit it, so the pipe reaches EOF
# when this script exits, including after SIGKILL or a closed terminal.
exec 3<>"$DATA/parent.stdin"
export SHARDRAIL_DEV_TRY_MARK=$DATA
"$BIN" \
  --data-dir "$DATA" \
  --no-file-log \
  --exit-with-stdin \
  --codex-app-server "$CODEX" \
  --session-workdir "$WORKDIR" \
  --codex-config 'model_reasoning_effort="low"' \
  --browser-listen "127.0.0.1:$PORT" \
  --browser-assets-dir "$ASSETS" \
  <"$DATA/parent.stdin" 3>&- \
  >"$DATA/daemon.stdout" 2>"$DATA/daemon.stderr" &
DAEMON_PID=$!
printf '%s\n' "$DAEMON_PID" >"$DATA/daemon.pid"

ready=0
i=0
while [ "$i" -lt 30 ]; do
  if grep -q "WebSocket listening at ws://127.0.0.1:$PORT/ws" "$DATA/daemon.stderr" 2>/dev/null; then
    ready=1
    break
  fi
  if ! kill -0 "$DAEMON_PID" 2>/dev/null; then
    break
  fi
  i=$((i + 1))
  sleep 1
done

if [ "$ready" -ne 1 ]; then
  echo "daemon did not listen on 127.0.0.1:$PORT" >&2
  cat "$DATA/daemon.stderr" >&2 || true
  exit 1
fi

ALLOW=$(grep "session workdir allowlist:" "$DATA/daemon.stderr" | head -n 1 || true)
if [ -n "$ALLOW" ]; then
  echo "$ALLOW"
fi

ENROLL=$("$BIN" --data-dir "$DATA" client enroll --label browser --allow-session-create)
CODE=$(printf '%s\n' "$ENROLL" | sed -n 's/^enrollment_code=\([0-9a-f][0-9a-f]*\).*/\1/p')
if [ -z "$CODE" ]; then
  echo "enrollment did not return a code" >&2
  printf '%s\n' "$ENROLL" >&2
  exit 1
fi

echo "Open this URL and enter the enrollment code. This script does not open a browser."
echo "url=http://127.0.0.1:$PORT/"
echo "enrollment_code=$CODE"
echo "data_dir=$DATA"
echo "daemon_pid=$DAEMON_PID"

wait "$DAEMON_PID" || true
DAEMON_PID=
