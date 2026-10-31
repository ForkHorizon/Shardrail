import json
import os
import subprocess
import time
from pathlib import Path

os.chdir(Path(__file__).resolve().parent)

APP = Path('src-tauri/target/release/bundle/macos/S5 UI Spike.app/Contents/MacOS/s5-ui').resolve()
RESULTS = Path('results')
HELPER = '/System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices/com.apple.WebKit.'

def processes():
    rows = subprocess.check_output(['ps', '-Ao', 'pid=,command='], text=True).splitlines()
    return {int(row.strip().split(None, 1)[0]): row.strip().split(None, 1)[1] for row in rows if row.strip()}

def run(label, query, settle, repetition):
    before = processes()
    env = os.environ | {'S5_BENCH_QUERY': query}
    app = subprocess.Popen([str(APP)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(settle)
    current = processes()
    helpers = [pid for pid, command in current.items() if HELPER in command and pid not in before]
    pids = [app.pid, *helpers]
    ps = subprocess.check_output(['ps', '-o', 'pid=,rss=', '-p', ','.join(map(str, pids))], text=True)
    rss_by_pid = {int(line.split()[0]): int(line.split()[1]) * 1024 for line in ps.splitlines()}
    footprint_path = RESULTS / f'b3-{label}-r{repetition}.json'
    subprocess.run(['footprint', *[arg for pid in pids for arg in ['--pid', str(pid)]], '-j', str(footprint_path)], check=True)
    raw = json.loads(footprint_path.read_text())
    time.sleep(1)
    app.terminate()
    try:
        app.wait(timeout=5)
    except subprocess.TimeoutExpired:
        app.kill(); app.wait()
    physical = sum(item['footprint'] for item in raw['processes'])
    rss = sum(rss_by_pid.values())
    return {'repetition': repetition, 'pids': pids, 'processes': [item['name'] for item in raw['processes']], 'physicalBytes': physical, 'rssBytes': rss, 'physicalMB': physical / 1e6, 'rssMB': rss / 1e6, 'footprintFile': str(footprint_path)}

RESULTS.mkdir(exist_ok=True)
report = {'date': time.strftime('%Y-%m-%dT%H:%M:%S%z'), 'machine': 'MacBook Pro Mac17,2 · Apple M5 · 32 GB · macOS 27.0.1 · arm64', 'scope': 'release Tauri shell plus newly spawned WebKit helpers; unrelated pre-existing WebKit helper PIDs excluded', 'idle': [], 'loaded50k8': []}
for repetition in range(1, 6):
    report['idle'].append(run('idle-footprint', 'events=0&streams=1&autostart=0', 61, repetition))
for repetition in range(1, 6):
    report['loaded50k8'].append(run('50k8-footprint', 'events=50000&streams=8&duration=120000&autostart=1', 12, repetition))
Path('results/tauri-b3-footprint.json').write_text(json.dumps(report, indent=2))
