#!/bin/bash
set -u

root_pid="${1:-}"
baseline_file="${2:-}"

if [ -z "$root_pid" ] || [ -z "$baseline_file" ]; then
  printf 'usage: %s ROOT_PID BASELINE_WEBKIT_FILE\n' "$(basename "$0")" >&2
  exit 2
fi

python3 - "$root_pid" "$baseline_file" <<'PY'
import os
import signal
import subprocess
import sys
import time

root_pid = int(sys.argv[1])
baseline_file = sys.argv[2]


def read_processes():
    try:
        output = subprocess.check_output(
            ["ps", "-eo", "pid=,ppid=,comm="],
            text=True,
            stderr=subprocess.DEVNULL,
        )
    except (FileNotFoundError, subprocess.CalledProcessError):
        return []
    rows = []
    for line in output.splitlines():
        parts = line.strip().split()
        if len(parts) < 3:
            continue
        try:
            pid = int(parts[0])
            ppid = int(parts[1])
            comm = parts[2]
        except ValueError:
            continue
        rows.append((pid, ppid, comm))
    return rows

rows = read_processes()
children = {}
for pid, ppid, _ in rows:
    children.setdefault(ppid, set()).add(pid)

descendants = set()
stack = [root_pid]
while stack:
    pid = stack.pop()
    if pid in descendants:
        continue
    descendants.add(pid)
    stack.extend(sorted(children.get(pid, set()), reverse=True))

baseline_pids = set()
if os.path.exists(baseline_file):
    with open(baseline_file, 'r', encoding='utf-8', errors='ignore') as fh:
        for line in fh:
            try:
                baseline_pids.add(int(line.strip()))
            except ValueError:
                pass

target = set(descendants)
for pid, _, comm in rows:
    if comm in {"com.apple.WebKit.WebContent", "com.apple.WebKit.Networking", "com.apple.WebKit.GPU"}:
        if pid not in baseline_pids and pid in descendants:
            target.add(pid)

for pid in sorted(target, reverse=True):
    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    except PermissionError:
        pass

time.sleep(0.2)
for pid in sorted(target, reverse=True):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        continue
    try:
        os.kill(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    except PermissionError:
        pass
PY

# Best-effort verification: the root process should be gone after the cleanup.
if ! ps -o stat= -p "$root_pid" >/dev/null 2>&1; then
  exit 0
fi

if ps -o stat= -p "$root_pid" | grep -qv '^Z'; then
  kill -KILL "$root_pid" 2>/dev/null || true
fi

exit 0
