#!/bin/bash
set -u

descendants() {
  local parent="$1" child
  for child in $(pgrep -P "$parent" 2>/dev/null || true); do
    descendants "$child"
    printf '%s\n' "$child"
  done
}

root_pid="$1"
processes="$(descendants "$root_pid")"
processes="$(printf '%s\n' "$processes" "$(pgrep -x s5-ui 2>/dev/null || true)" | sed '/^$/d' | awk '!seen[$0]++')"
is_running() {
  local state
  state="$(ps -o stat= -p "$1" 2>/dev/null | tr -d ' ')"
  [ -n "$state" ] && [ "${state#Z}" = "$state" ]
}
if [ -n "$processes" ]; then kill $processes 2>/dev/null || true; fi
kill "$root_pid" 2>/dev/null || true
for _ in $(seq 1 50); do
  alive=0
  for pid in $processes "$root_pid"; do is_running "$pid" && alive=1; done
  [ "$alive" = 0 ] && exit 0
  sleep 0.1
done
for pid in $processes "$root_pid"; do kill -KILL "$pid" 2>/dev/null || true; done
for pid in $processes "$root_pid"; do
  if is_running "$pid"; then printf 'Process %s survived benchmark cleanup\n' "$pid" >&2; exit 1; fi
done
