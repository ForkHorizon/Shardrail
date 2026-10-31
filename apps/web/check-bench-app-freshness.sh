#!/bin/bash
set -u

app="$1"
shift
for path in "$@"; do
  if [ -f "$path" ] && [ "$path" -nt "$app" ] || [ -d "$path" ] && find "$path" -type f -newer "$app" -print -quit | grep -q .; then
    printf 'Benchmark app is stale; rebuild it after changing %s (CI=true npm run tauri -- build)\n' "$path" >&2
    exit 1
  fi
done
