#!/bin/bash
set -u
cd "$(dirname "$0")"
app="${S5_BENCH_APP:-$PWD/src-tauri/target/release/bundle/macos/S5 UI Spike.app/Contents/MacOS/s5-ui}"
wait_seconds="${S5_BENCH_WAIT_SECONDS:-60}"
results_dir="${S5_BENCH_RESULTS_DIR:-results}"
mkdir -p "$results_dir"
failed=0
if [ ! -x "$app" ]; then
  printf 'Tauri benchmark executable is missing or not executable: %s\n' "$app" >&2
  exit 1
fi
bash check-bench-app-freshness.sh "$app" src dist src-tauri/src src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/build.rs
node tauri-results-server.mjs >"$results_dir/tauri-results-server.log" 2>&1 &
receiver_pid=$!
trap 'kill "$receiver_pid" 2>/dev/null || true' EXIT
receiver_ready=0
for _ in $(seq 1 20); do
  if curl -fsS http://127.0.0.1:5174/health >/dev/null; then receiver_ready=1; break; fi
  sleep 0.25
done
if [ "$receiver_ready" != 1 ]; then
  printf 'Tauri result receiver did not become healthy on 127.0.0.1:5174\n' >&2
  exit 1
fi
if [ "${S5_BENCH_SMOKE:-0}" = 1 ]; then event_sizes=(0); stream_counts=(1); repetitions=(1); temperatures=(cold); else
  read -r -a event_sizes <<< "${S5_TAURI_EVENTS:-0 5000 50000}"
  read -r -a stream_counts <<< "${S5_TAURI_STREAMS:-1 4 8}"
  read -r -a repetitions <<< "${S5_TAURI_REPETITIONS:-1 2 3 4 5}"
  read -r -a temperatures <<< "${S5_TAURI_TEMPERATURES:-cold warm}"
fi
for events in "${event_sizes[@]}"; do
  for streams in "${stream_counts[@]}"; do
    if [ "$events" = 0 ] && [ "$streams" != 1 ]; then continue; fi
    for temperature in "${temperatures[@]}"; do
    for repetition in "${repetitions[@]}"; do
      name="$temperature-$events-$streams-r$repetition"
      rm -f "$results_dir/tauri-$name.json"
      query="events=$events&streams=$streams&duration=6000&autobench=1&run=$name&temperature=$temperature"
      if [ "${S5_BENCH_B4:-0}" = 1 ] && [ "$events" != 0 ]; then query="$query&b4=1"; fi
      if [ "$events" = 0 ]; then query="$query&autostart=0"; fi
      webkit_before="$(mktemp "${TMPDIR:-/tmp}/s5-webkit-before.XXXXXX")"
      ps -Ao pid=,command= | awk '/com\.apple\.WebKit\.(WebContent|Networking|GPU)( |$)/ { print $1 }' >"$webkit_before"
      S5_BENCH_QUERY="$query" "$app" >"$results_dir/tauri-$name.log" 2>&1 &
      pid=$!
      done_run=0
      : >"$results_dir/tauri-$name-processes.txt"
      for _ in $(seq 1 "$wait_seconds"); do
        if [ -s "$results_dir/tauri-$name.json" ]; then done_run=1; break; fi
        ps -Ao pid,ppid,%cpu,rss,command | grep -E '/s5-ui|com\.apple\.WebKit\.(WebContent|Networking|GPU)' >>"$results_dir/tauri-$name-processes.txt" || true
        sleep 1
      done
      bash cleanup-bench-processes.sh "$pid" "$webkit_before"
      rm -f "$webkit_before"
      wait "$pid" 2>/dev/null || true
      if [ "$done_run" != 1 ]; then
        printf '{"error":"Tauri run did not write results within %s seconds","events":%s,"streams":%s,"repetition":%s}\n' "$wait_seconds" "$events" "$streams" "$repetition" >"$results_dir/tauri-$name.json"
        failed=1
      elif grep -q '"error"' "$results_dir/tauri-$name.json"; then
        printf 'Tauri scenario %s wrote an error result\n' "$name" >&2
        failed=1
      fi
    done
    done
  done
done
exit "$failed"
