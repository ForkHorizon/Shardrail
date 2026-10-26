#!/bin/sh
set -u

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
daemon_bin="$repo_root/target/debug/shardraild"
fake_acp="$repo_root/target/debug/shardrail-fake-acp"
fake_codex="$repo_root/crates/shardraild/tests/fixtures/fake_codex_app_server.py"

test_process_pids() {
  ps -axww -o pid= -o command= | awk -v daemon="$daemon_bin" -v acp="$fake_acp" -v codex="$fake_codex" '
    {
      command = $0
      sub(/^[[:space:]]*[0-9]+[[:space:]]+/, "", command)
      split(command, argv, /[[:space:]]+/)
      if (substr(command, 1, length(daemon)) == daemon && substr(command, length(daemon) + 1, 1) ~ /[[:space:]]/ && index(command, "--data-dir")) print $1
      else if (substr(command, 1, length(acp)) == acp && substr(command, length(acp) + 1, 1) ~ /[[:space:]]/) print $1
      else if (index(command, codex) && argv[1] ~ /(^|\/)python[0-9.]*$/) print $1
    }
  ' | sort -n
}

before_file=$(mktemp /private/tmp/shardrail-daemon-pids.XXXXXX)
after_file=$(mktemp /private/tmp/shardrail-daemon-pids.XXXXXX)
trap 'rm -f "$before_file" "$after_file"' EXIT HUP INT TERM
test_process_pids >"$before_file"
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
if [ "$daemon_status" -eq 0 ]; then
  nice -n 10 cargo test -p shardraild --test session_create -- --test-threads=1
  daemon_status=$?
fi
test_process_pids >"$after_file"
new_pids=$(comm -13 "$before_file" "$after_file")
before_count=$(wc -l <"$before_file" | tr -d ' ')
after_count=$(wc -l <"$after_file" | tr -d ' ')
new_count=$(printf '%s\n' "$new_pids" | sed '/^$/d' | wc -l | tr -d ' ')
printf 'daemon test process check (worktree target): before=%s after=%s new=%s\n' "$before_count" "$after_count" "$new_count"
if [ -n "$new_pids" ]; then
  printf 'new daemon test processes survived: %s\n' "$new_pids" >&2
  daemon_status=1
fi
exit "$daemon_status"
