import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { spawn } from 'node:child_process';
import { join, resolve } from 'node:path';
import { tmpdir } from 'node:os';

test('B4 harness enables daemon-stream synchronization and rejects incomplete samples', () => {
  const script = readFileSync('b4-bench.sh', 'utf8');
  assert.match(script, /S5_BENCH_QUERY=.*&b4=1/);
  assert.match(script, /r\.b4\?\.count !== Number\(process\.argv\[2\]\)/);
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
