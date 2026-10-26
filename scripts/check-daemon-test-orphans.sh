#!/bin/sh
set -u

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
daemon_bin="$repo_root/target/debug/shardraild"

daemon_pids() {
  ps -axo pid=,command= | awk -v bin="$daemon_bin" '
    index($0, bin) && /(^|[[:space:]])shardraild([[:space:]]|$)/ && /--data-dir/ { print $1 }
  ' | sort -n
}

before_file=$(mktemp /private/tmp/shardrail-daemon-pids.XXXXXX)
after_file=$(mktemp /private/tmp/shardrail-daemon-pids.XXXXXX)
trap 'rm -f "$before_file" "$after_file"' EXIT HUP INT TERM
daemon_pids >"$before_file"
nice -n 10 cargo build -p shardrail-fake-acp
daemon_status=$?
if [ "$daemon_status" -ne 0 ]; then
  exit "$daemon_status"
fi
nice -n 10 cargo test -p shardraild --test daemon -- --test-threads=1
daemon_status=$?
if [ "$daemon_status" -eq 0 ]; then
  nice -n 10 cargo test -p shardraild --test codex -- --test-threads=1
  daemon_status=$?
fi
daemon_pids >"$after_file"
new_pids=$(comm -13 "$before_file" "$after_file")
before_count=$(wc -l <"$before_file" | tr -d ' ')
after_count=$(wc -l <"$after_file" | tr -d ' ')
new_count=$(printf '%s\n' "$new_pids" | sed '/^$/d' | wc -l | tr -d ' ')
printf 'shardraild process check (worktree target): before=%s after=%s new=%s\n' "$before_count" "$after_count" "$new_count"
if [ -n "$new_pids" ]; then
  printf 'new shardraild test processes survived: %s\n' "$new_pids" >&2
  daemon_status=1
fi
exit "$daemon_status"
