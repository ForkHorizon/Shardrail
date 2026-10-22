#!/bin/bash
set -u

descendants() {
  local parent="$1" child
  for child in $(pgrep -P "$parent" 2>/dev/null || true); do
    descendants "$child"
    printf '%s\n' "$child"
  done
}

new_webkit_helpers() {
  ps -Ao pid=,command= | awk '/com\.apple\.WebKit\.(WebContent|Networking|GPU)( |$)/ { print $1 }' | while read -r pid; do
    grep -qxF "$pid" "$baseline_file" || printf '%s\n' "$pid"
  done
}

is_running() {
  local state
  state="$(ps -o stat= -p "$1" 2>/dev/null | tr -d ' ')"
  [ -n "$state" ] && [ "${state#Z}" = "$state" ]
}

root_pid="$1"
baseline_file="$2"
processes=''
for _ in $(seq 1 50); do
  processes="$(printf '%s\n' "$processes" "$(descendants "$root_pid")" "$(new_webkit_helpers)" | sed '/^$/d' | sort -u)"
  while read -r pid; do [ -n "$pid" ] && kill "$pid" 2>/dev/null || true; done <<< "$processes"
  kill "$root_pid" 2>/dev/null || true
  alive=0
  while read -r pid; do [ -n "$pid" ] && is_running "$pid" && alive=1; done <<< "$(printf '%s\n' "$processes" "$root_pid")"
  [ "$alive" = 0 ] && exit 0
  sleep 0.1
done

while read -r pid; do [ -n "$pid" ] && kill -KILL "$pid" 2>/dev/null || true; done <<< "$(printf '%s\n' "$processes" "$root_pid")"
for _ in $(seq 1 20); do
  processes="$(printf '%s\n' "$processes" "$(descendants "$root_pid")" "$(new_webkit_helpers)" | sed '/^$/d' | sort -u)"
  alive=0
  while read -r pid; do [ -n "$pid" ] && is_running "$pid" && alive=1; done <<< "$(printf '%s\n' "$processes" "$root_pid")"
  [ "$alive" = 0 ] && exit 0
  sleep 0.1
done
printf 'Benchmark cleanup left running PIDs: %s %s\n' "$root_pid" "$processes" >&2
exit 1
