#!/bin/bash
set -euo pipefail
repo="$(cd "$(dirname "$0")/../../.." && pwd)"
app="${S5_SWIFT_APP:-$repo/apps/s5-swift/.build/release/shardrail-swift}"
daemon="${SHARDRAILD_BIN:-$repo/target/release/shardraild}"
fake="${SHARDRAIL_FAKE_ACP_BIN:-$repo/target/release/shardrail-fake-acp}"
results="${S5_SWIFT_RESULTS_DIR:-$repo/apps/s5-prototype/bench-results/w4k2-swift}"
wait_seconds="${S5_BENCH_WAIT_SECONDS:-300}"
mode="full"
for arg in "$@"; do
  case "$arg" in
    --check|--dry-run) mode="check" ;;
    --smoke) mode="smoke" ;;
    *) printf 'Unknown option: %s\n' "$arg" >&2; exit 2 ;;
  esac
done
for n in "$wait_seconds"; do [[ "$n" =~ ^[1-9][0-9]*$ ]] || { echo 'timeout must be a positive integer' >&2; exit 2; }; done
if [[ "$mode" == check ]]; then
  for executable in "$app" "$daemon" "$fake"; do [[ -x "$executable" ]] || { printf 'Missing executable: %s\n' "$executable" >&2; exit 1; }; done
  printf 'Configuration valid. Full matrix: 60 runs; output: %s; timeout: %ss per run. No processes launched.\n' "$results" "$wait_seconds"
  exit 0
fi
for executable in "$app" "$daemon" "$fake"; do [[ -x "$executable" ]] || { printf 'Missing executable: %s\n' "$executable" >&2; exit 1; }; done
mkdir -p "$results"
kill_tree() {
  local pid="$1" child
  while read -r child; do [[ -n "$child" ]] && kill_tree "$child"; done < <(pgrep -P "$pid" || true)
  kill "$pid" 2>/dev/null || true
  for _ in {1..20}; do kill -0 "$pid" 2>/dev/null || return 0; sleep 0.05; done
  kill -KILL "$pid" 2>/dev/null || true
}
temp=""; daemon_pid=""; app_pid=""
cleanup_all() {
  local status=$?
  trap - EXIT INT TERM
  if [[ -n "$app_pid" ]]; then kill_tree "$app_pid"; wait "$app_pid" 2>/dev/null || true; fi
  if [[ -n "$daemon_pid" ]]; then kill_tree "$daemon_pid"; wait "$daemon_pid" 2>/dev/null || true; fi
  [[ -z "$temp" || ! -d "$temp" ]] || rm -rf "$temp"
  exit "$status"
}
trap cleanup_all EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
if [[ "$mode" == smoke ]]; then sizes=(5000); streams_list=(1); temperatures=(cold); reps=(1); else sizes=(5000 50000); streams_list=(1 4 8); temperatures=(cold warm); reps=(1 2 3 4 5); fi
failed=0
B4_MINIMUM_SAMPLE_RATE=0.005
for events in "${sizes[@]}"; do
  for streams in "${streams_list[@]}"; do
    chunks=$((events / streams))
    for temperature in "${temperatures[@]}"; do
      for repetition in "${reps[@]}"; do
        name="$temperature-$events-$streams-r$repetition"
        temp="$(mktemp -d "${TMPDIR:-/tmp}/shardrail-swift-b4.XXXXXX")"
        result="$results/b4-$name.json"
        rm -f "$result"
        SHARDRAIL_FAKE_ACP_STREAM_EVENTS="$chunks" "$daemon" --data-dir "$temp" --fake-acp-path "$fake" >"$results/b4-daemon-$name.log" 2>&1 & daemon_pid=$!
        ready=0
        for _ in $(seq 1 200); do [[ -S "$temp/shardraild.sock" ]] && { ready=1; break; }; sleep 0.05; done
        if [[ "$ready" != 1 ]]; then kill_tree "$daemon_pid"; wait "$daemon_pid" 2>/dev/null || true; daemon_pid=""; rm -rf "$temp"; temp=""; printf '{"error":"daemon socket startup timed out","stack":"swift-appkit"}\n' >"$result"; failed=1; continue; fi
        S5_SHARDRAIL_DATA_DIR="$temp" SHARDRAIL_DAEMON="$daemon" S5_BENCH_QUERY="events=$events&streams=$streams&temperature=$temperature" S5_SWIFT_RESULT_PATH="$result" "$app" "events=$events" "streams=$streams" "temperature=$temperature" b4=1 >"$results/b4-app-$name.log" 2>&1 & app_pid=$!
        complete=0
        for ((second=0; second<wait_seconds; second++)); do
          if [[ -s "$result" ]]; then complete=1; break; fi
          if ! kill -0 "$app_pid" 2>/dev/null; then break; fi
          sleep 1
        done
        kill_tree "$app_pid"; kill_tree "$daemon_pid"
        wait "$app_pid" 2>/dev/null || true; wait "$daemon_pid" 2>/dev/null || true
        app_pid=""; daemon_pid=""
        if [[ -d "$temp" ]]; then rm -rf "$temp"; fi
        temp=""
        if [[ "$complete" != 1 ]]; then printf '{"error":"B4 run timed out or app exited","events":%s,"streams":%s,"repetition":%s,"stack":"swift-appkit"}\n' "$events" "$streams" "$repetition" >"$result"; failed=1; continue; fi
        if ! node -e 'const r=JSON.parse(require("node:fs").readFileSync(process.argv[1],"utf8")); const e=+process.argv[2],floor=Math.min(e,Math.max(20,Math.ceil(e*Number(process.argv[3])))); if(r.error||r.pageVisibility!=="visible"||r.daemonAgentChunkCount!==e||(r.b4?.count??0)<floor) process.exit(1)' "$result" "$events" "$B4_MINIMUM_SAMPLE_RATE"; then echo "Incomplete B4 result: $name" >&2; failed=1; fi
      done
    done
  done
done
exit "$failed"
