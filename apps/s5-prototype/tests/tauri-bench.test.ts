import assert from 'node:assert/strict';
import test from 'node:test';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { spawn } from 'node:child_process';
import { join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { setTimeout as delay } from 'node:timers/promises';

test('B4 harness disables mock streams and checks chunk delivery independently of virtualized samples', () => {
  const script = readFileSync('b4-bench.sh', 'utf8');
  const app = readFileSync('src/main.tsx', 'utf8');
  assert.match(app, /while \(getDaemonAgentChunkCount\(\) < count/);
  assert.match(script, /S5_BENCH_QUERY=.*&b4=1/);
  assert.match(script, /&autostart=0/);
  assert.match(script, /!r\.b4\?\.count \|\| r\.daemonAgentChunkCount !== Number\(process\.argv\[2\]\)/);
  assert.match(script, /cleanup-bench-processes\.sh "\$app_pid"/);
  assert.match(app, /if \(!query\.has\('b4'\)\) \{[\s\S]*el\.scrollTop =/);
});

test('benchmark cleanup kills a process tree and confirms the root is gone', async () => {
  const pidFile = join(tmpdir(), `s5-cleanup-${process.pid}.pid`);
  const root = spawn('bash', ['-c', `sleep 60 & echo $! > '${pidFile}'; wait`], { stdio: 'ignore' });
  try {
    for (let attempt = 0; !existsSync(pidFile) && attempt < 200; attempt++) await delay(10);
    assert.equal(existsSync(pidFile), true, 'process tree did not start');
    const childPid = Number(readFileSync(pidFile, 'utf8').trim());
    const rootExited = new Promise<void>((resolveExit) => root.once('exit', () => resolveExit()));
    const result = spawnSync('bash', ['cleanup-bench-processes.sh', String(root.pid)], { encoding: 'utf8', timeout: 10_000 });
    assert.equal(result.status, 0, result.stderr);
    await rootExited;
    for (const pid of [root.pid!, childPid]) {
      const state = spawnSync('ps', ['-o', 'stat=', '-p', String(pid)], { encoding: 'utf8' }).stdout.trim();
      assert.ok(!state || state.startsWith('Z'), `process ${pid} is still running (${state})`);
    }
  } finally {
    root.kill('SIGKILL');
    rmSync(pidFile, { force: true });
  }
});

test('Tauri harness exits unsuccessfully when a scenario times out', () => {
  const resultsDir = mkdtempSync(join(tmpdir(), 's5-tauri-bench-test-'));
  try {
    const run = spawnSync('bash', ['tauri-bench.sh'], {
      encoding: 'utf8',
      timeout: 15000,
      env: { ...process.env, S5_BENCH_APP: '/usr/bin/false', S5_BENCH_WAIT_SECONDS: '1', S5_BENCH_SMOKE: '1', S5_BENCH_RESULTS_DIR: resultsDir },
    });
    assert.equal(run.error, undefined, run.error?.message);
    assert.notEqual(run.status, 0);
    const result = JSON.parse(readFileSync(join(resultsDir, 'tauri-cold-0-1-r1.json'), 'utf8')) as { error?: string };
    assert.match(result.error ?? '', /did not write results/);
  } finally {
    rmSync(resultsDir, { recursive: true, force: true });
  }
});

test('Tauri result server honors the configured results directory', async () => {
  const resultsDir = mkdtempSync(join(tmpdir(), 's5-tauri-results-test-'));
  const server = spawn('node', [resolve('tauri-results-server.mjs')], {
    cwd: process.cwd(),
    env: { ...process.env, S5_BENCH_RESULTS_DIR: resultsDir },
    stdio: 'ignore',
  });
  try {
    let ready = false;
    for (let attempt = 0; attempt < 40; attempt++) {
      try {
        if ((await fetch('http://127.0.0.1:5174/health')).ok) { ready = true; break; }
      } catch (error) {
        if (attempt === 39) throw error;
      }
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 50));
    }
    assert.equal(ready, true, 'result server did not become ready');
    const response = await fetch('http://127.0.0.1:5174/result', {
      method: 'POST',
      headers: { origin: 'tauri://localhost', 'content-type': 'application/json' },
      body: JSON.stringify({ filename: 'cold-0-1-r1', result: { ok: true } }),
    });
    assert.equal(response.status, 200);
    assert.deepEqual(JSON.parse(readFileSync(join(resultsDir, 'tauri-cold-0-1-r1.json'), 'utf8')), { ok: true });
  } finally {
    server.kill('SIGTERM');
    rmSync(resultsDir, { recursive: true, force: true });
  }
});
