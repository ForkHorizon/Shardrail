#!/bin/bash
set -u
cd "$(dirname "$0")"
app="${S5_BENCH_APP:-$PWD/src-tauri/target/release/bundle/macos/S5 UI Spike.app/Contents/MacOS/s5-ui}"
daemon_bin="${SHARDRAILD_BIN:-$PWD/../../target/release/shardraild}"
fake_bin="${SHARDRAIL_FAKE_ACP_BIN:-$PWD/../../target/release/shardrail-fake-acp}"
wait_seconds="${S5_BENCH_WAIT_SECONDS:-300}"
results_dir="${S5_BENCH_RESULTS_DIR:-results}"
mkdir -p "$results_dir"
failed=0
if [ ! -x "$app" ] || [ ! -x "$daemon_bin" ] || [ ! -x "$fake_bin" ]; then
  printf 'Missing release app or daemon/fake-agent binaries\n' >&2
  exit 1
fi
bash check-bench-app-freshness.sh "$app" src dist src-tauri/src src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/build.rs
S5_BENCH_PREFIX=b4 S5_BENCH_RESULTS_DIR="$results_dir" node tauri-results-server.mjs >"$results_dir/b4-results-server.log" 2>&1 &
receiver_pid=$!
trap 'kill "$receiver_pid" 2>/dev/null || true' EXIT
for _ in $(seq 1 20); do
  if curl -fsS http://127.0.0.1:5174/health >/dev/null; then break; fi
  sleep 0.25
done
read -r -a event_sizes <<< "${S5_B4_EVENTS:-5000 50000}"
read -r -a stream_counts <<< "${S5_B4_STREAMS:-1 4 8}"
read -r -a temperatures <<< "${S5_B4_TEMPERATURES:-cold warm}"
repetitions="${S5_B4_REPETITIONS:-5}"
for events in "${event_sizes[@]}"; do
  for streams in "${stream_counts[@]}"; do
    chunks=$((events / streams))
    for temperature in "${temperatures[@]}"; do
      for repetition in $(seq 1 "$repetitions"); do
        name="$temperature-$events-$streams-r$repetition"
        : >"$results_dir/b4-$name.json"
        temp_dir=$(mktemp -d "${TMPDIR:-/tmp}/shardrail-b4.XXXXXX")
        SHARDRAIL_FAKE_ACP_STREAM_EVENTS="$chunks" "$daemon_bin" --data-dir "$temp_dir" --fake-acp-path "$fake_bin" >"$results_dir/b4-daemon-$name.log" 2>&1 &
        daemon_pid=$!
        ps -Ao pid=,command= | awk '/com\.apple\.WebKit\.(WebContent|Networking|GPU)( |$)/ { print $1 }' >"$temp_dir/webkit-before.txt"
        S5_SHARDRAIL_DATA_DIR="$temp_dir" SHARDRAIL_DAEMON="$daemon_bin" S5_BENCH_QUERY="events=$events&streams=$streams&duration=6000&autostart=0&autobench=1&run=$name&temperature=$temperature&b4=1" "$app" >"$results_dir/b4-app-$name.log" 2>&1 &
        app_pid=$!
        done_run=0
        for _ in $(seq 1 "$wait_seconds"); do
          if [ -s "$results_dir/b4-$name.json" ]; then done_run=1; break; fi
          sleep 1
        done
        bash cleanup-bench-processes.sh "$app_pid" "$temp_dir/webkit-before.txt"
        bash cleanup-bench-processes.sh "$daemon_pid" "$temp_dir/webkit-before.txt"
        wait "$app_pid" 2>/dev/null || true
        wait "$daemon_pid" 2>/dev/null || true
        rm -rf "$temp_dir"
        if [ "$done_run" != 1 ]; then
          printf '{"error":"B4 run timed out","events":%s,"streams":%s,"repetition":%s}\n' "$events" "$streams" "$repetition" >"$results_dir/b4-$name.json"
          failed=1
        else
          validation=0
          node -e 'const r=JSON.parse(require("node:fs").readFileSync(process.argv[1], "utf8")); const events=Number(process.argv[2]); if (r.error) { console.error(`B4 scenario failed: ${r.error}`); process.exit(2); } if ((r.b4?.count ?? 0) < Math.min(events, 20) || r.daemonAgentChunkCount !== events || r.pageVisibility !== "visible") process.exit(1); console.log(`B4 samples: ${r.b4.count}/${events} (${(100*r.b4.count/events).toFixed(2)}%)`)' "$results_dir/b4-$name.json" "$events" || validation=$?
          if [ "$validation" = 2 ]; then
            failed=1
          elif [ "$validation" != 0 ]; then
            printf 'B4 scenario %s wrote incomplete daemon samples (need at least %s, visible page, and all requested chunks)\n' "$name" "$((events < 20 ? events : 20))" >&2
            failed=1
          fi
        fi
      done
    done
  done
done
exit "$failed"
