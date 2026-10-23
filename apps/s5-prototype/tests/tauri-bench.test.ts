import assert from 'node:assert/strict';
import test from 'node:test';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, utimesSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { spawn } from 'node:child_process';
import { join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { setTimeout as delay } from 'node:timers/promises';

test('B4 harness configures daemon delivery without synthetic scrolling', () => {
  const script = readFileSync('b4-bench.sh', 'utf8');
  const app = readFileSync('src/main.tsx', 'utf8');
  assert.match(script, /S5_BENCH_QUERY=.*&b4=1/);
  assert.match(script, /&autostart=0/);
  assert.doesNotMatch(script, /open -a/);
  assert.match(script, /check-bench-app-freshness\.sh/);
  assert.match(script, /cleanup-bench-processes\.sh "\$app_pid" "\$temp_dir\/webkit-before\.txt"/);
  assert.match(app, /waitForB4Samples\(minimumSamples/);
  assert.match(app, /if \(!query\.has\('b4'\)\) \{[\s\S]*el\.scrollTop =/);
});

test('B4 shell harness accepts visible results and rejects hidden-window samples', () => {
  const directory = mkdtempSync(join(tmpdir(), 's5-b4-harness-'));
  const resultsDir = join(directory, 'results');
  const app = join(directory, 'stub-app.mjs');
  const daemon = join(directory, 'stub-daemon');
  const fakeAgent = join(directory, 'stub-fake-agent');
  const fakeOpen = join(directory, 'fake-bin');
  const openMarker = join(directory, 'open-called');
  const fakeBundle = join(directory, 'fake.app');
  try {
    mkdirSync(fakeOpen);
    mkdirSync(fakeBundle);
    writeFileSync(join(fakeOpen, 'open'), `#!/bin/bash\nprintf called > '${openMarker}'\n`);
    writeFileSync(app, `#!/usr/bin/env node
const query = new URLSearchParams(process.env.S5_BENCH_QUERY);
const events = Number(query.get('events'));
const response = await fetch('http://127.0.0.1:5174/result', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ filename: query.get('run'), result: { ...(process.env.S5_TEST_ERROR ? { error: process.env.S5_TEST_ERROR } : {}), b4: { count: Number(process.env.S5_TEST_SAMPLES ?? events) }, daemonAgentChunkCount: events, pageVisibility: process.env.S5_TEST_VISIBILITY ?? 'visible', pageFocused: process.env.S5_TEST_FOCUSED !== 'false' } }) });
if (!response.ok) process.exit(1);
`);
    writeFileSync(daemon, '#!/bin/bash\nsleep 60\n');
    writeFileSync(fakeAgent, '#!/bin/bash\nexit 0\n');
    for (const file of [app, daemon, fakeAgent]) chmodSync(file, 0o755);
    const env = { ...process.env, PATH: `${fakeOpen}:${process.env.PATH}`, S5_BENCH_APP: app, S5_BENCH_APP_BUNDLE: fakeBundle, SHARDRAILD_BIN: daemon, SHARDRAIL_FAKE_ACP_BIN: fakeAgent, S5_BENCH_WAIT_SECONDS: '3', S5_BENCH_RESULTS_DIR: resultsDir, S5_B4_EVENTS: '3', S5_B4_STREAMS: '1', S5_B4_TEMPERATURES: 'cold', S5_B4_REPETITIONS: '1' };
    const run = spawnSync('bash', ['b4-bench.sh'], { encoding: 'utf8', timeout: 15_000, env });
    assert.equal(run.error, undefined, run.error?.message);
    assert.equal(run.status, 0, run.stderr);
    const result = JSON.parse(readFileSync(join(resultsDir, 'b4-cold-3-1-r1.json'), 'utf8')) as { b4: { count: number }; daemonAgentChunkCount: number };
    assert.equal(result.b4.count, 3);
    assert.equal(result.daemonAgentChunkCount, 3);
    assert.equal(existsSync(openMarker), false, 'benchmark must not invoke LaunchServices and spawn an unmanaged app');
    const unfocusedVisible = spawnSync('bash', ['b4-bench.sh'], { encoding: 'utf8', timeout: 15_000, env: { ...env, S5_TEST_FOCUSED: 'false' } });
    assert.equal(unfocusedVisible.status, 0, unfocusedVisible.stderr);
    const undersampled = spawnSync('bash', ['b4-bench.sh'], { encoding: 'utf8', timeout: 15_000, env: { ...env, S5_B4_EVENTS: '100', S5_TEST_SAMPLES: '19' } });
    assert.equal(undersampled.status, 1);
    assert.match(undersampled.stderr, /wrote incomplete daemon samples \(need at least 20,/);
    const hidden = spawnSync('bash', ['b4-bench.sh'], { encoding: 'utf8', timeout: 15_000, env: { ...env, S5_TEST_VISIBILITY: 'hidden', S5_TEST_FOCUSED: 'false' } });
    assert.equal(hidden.error, undefined, hidden.error?.message);
    assert.equal(hidden.status, 1);
    assert.match(hidden.stderr, /wrote incomplete daemon samples/);
    const rejected = JSON.parse(readFileSync(join(resultsDir, 'b4-cold-3-1-r1.json'), 'utf8')) as { pageVisibility: string; pageFocused: boolean };
    assert.equal(rejected.pageVisibility, 'hidden');
    assert.equal(rejected.pageFocused, false);
    const error = spawnSync('bash', ['b4-bench.sh'], { encoding: 'utf8', timeout: 15_000, env: { ...env, S5_TEST_ERROR: 'fixture failure' } });
    assert.equal(error.status, 1);
    assert.match(error.stderr, /B4 scenario failed: fixture failure/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('benchmark rejects a packaged app older than its frontend source', () => {
  const directory = mkdtempSync(join(tmpdir(), 's5-app-freshness-'));
  const app = join(directory, 's5-ui');
  const source = join(directory, 'src');
  const input = join(source, 'main.tsx');
  const older = new Date(Date.now() - 10_000);
  const newer = new Date(Date.now() + 10_000);
  try {
    mkdirSync(source);
    writeFileSync(app, 'app');
    writeFileSync(input, 'source');
    utimesSync(app, older, older);
    utimesSync(input, newer, newer);
    const stale = spawnSync('bash', ['check-bench-app-freshness.sh', app, source], { encoding: 'utf8' });
    assert.notEqual(stale.status, 0);
    assert.match(stale.stderr, /Benchmark app is stale/);
    utimesSync(app, newer, newer);
    assert.equal(spawnSync('bash', ['check-bench-app-freshness.sh', app, source]).status, 0);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('benchmark cleanup kills a process tree and confirms the root is gone', async () => {
  const pidFile = join(tmpdir(), `s5-cleanup-${process.pid}.pid`);
  const baselineFile = join(tmpdir(), `s5-cleanup-${process.pid}.baseline`);
  const root = spawn('bash', ['-c', `sleep 60 & echo $! > '${pidFile}'; wait`], { stdio: 'ignore' });
  let unrelated: ReturnType<typeof spawn> | undefined;
  let webkit: ReturnType<typeof spawn> | undefined;
  try {
    const currentProcesses = spawnSync('ps', ['-Ao', 'pid=,command='], { encoding: 'utf8' }).stdout;
    const webkitBaseline = currentProcesses.split('\n').filter((line) => /com\.apple\.WebKit\.(WebContent|Networking|GPU)( |$)/.test(line)).map((line) => line.trim().split(/\s+/, 1)[0]).join('\n');
    writeFileSync(baselineFile, webkitBaseline);
    unrelated = spawn(process.execPath, ['-e', "process.title='s5-ui';setTimeout(()=>{},60000)"], { stdio: 'ignore' });
    webkit = spawn(process.execPath, ['-e', "process.title='com.apple.WebKit.WebContent';setTimeout(()=>{},60000)"], { stdio: 'ignore' });
    for (let attempt = 0; attempt < 100; attempt++) {
      const command = spawnSync('ps', ['-o', 'command=', '-p', String(webkit.pid)], { encoding: 'utf8' }).stdout;
      if (command.includes('com.apple.WebKit.WebContent')) break;
      await delay(10);
    }
    for (let attempt = 0; !existsSync(pidFile) && attempt < 200; attempt++) await delay(10);
    assert.equal(existsSync(pidFile), true, 'process tree did not start');
    const childPid = Number(readFileSync(pidFile, 'utf8').trim());
    const rootExited = new Promise<void>((resolveExit) => root.once('exit', () => resolveExit()));
    const result = spawnSync('bash', ['cleanup-bench-processes.sh', String(root.pid), baselineFile], { encoding: 'utf8', timeout: 10_000 });
    assert.equal(result.status, 0, result.stderr);
    await rootExited;
    await new Promise<void>((resolveExit) => webkit!.once('exit', () => resolveExit()));
    for (const pid of [root.pid!, childPid]) {
      const state = spawnSync('ps', ['-o', 'stat=', '-p', String(pid)], { encoding: 'utf8' }).stdout.trim();
      assert.ok(!state || state.startsWith('Z'), `process ${pid} is still running (${state})`);
    }
    assert.equal(spawnSync('kill', ['-0', String(unrelated.pid)]).status, 0, 'unrelated s5-ui process was killed');
  } finally {
    root.kill('SIGKILL');
    unrelated?.kill('SIGKILL');
    webkit?.kill('SIGKILL');
    rmSync(pidFile, { force: true });
    rmSync(baselineFile, { force: true });
  }
});

test('Tauri harness exits unsuccessfully when a scenario times out', () => {
  const resultsDir = mkdtempSync(join(tmpdir(), 's5-tauri-bench-test-'));
  const app = join(resultsDir, 'stub-app');
  try {
    writeFileSync(app, '#!/bin/bash\nsleep 60\n');
    chmodSync(app, 0o755);
    const future = new Date(Date.now() + 10_000);
    utimesSync(app, future, future);
    const run = spawnSync('bash', ['tauri-bench.sh'], {
      encoding: 'utf8',
      timeout: 15000,
      env: { ...process.env, S5_BENCH_APP: app, S5_BENCH_WAIT_SECONDS: '1', S5_BENCH_SMOKE: '1', S5_BENCH_RESULTS_DIR: resultsDir },
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
    const samplesMs = Array.from({ length: 50_000 }, (_, index) => Math.sin(index) * 10_000 + 10_000.123456789);
    const body = JSON.stringify({ filename: 'cold-0-1-r1', result: { b4: { count: samplesMs.length, samplesMs }, extraMetric: 'x'.repeat(150_000) } });
    assert.ok(body.length > 1_000_000);
    const response = await fetch('http://127.0.0.1:5174/result', {
      method: 'POST',
      headers: { origin: 'tauri://localhost', 'content-type': 'application/json' },
      body,
    });
    assert.equal(response.status, 200);
    const saved = JSON.parse(readFileSync(join(resultsDir, 'tauri-cold-0-1-r1.json'), 'utf8')) as { b4: { count: number; samplesMs: number[] } };
    assert.equal(saved.b4.count, 50_000);
    assert.deepEqual(saved.b4.samplesMs, samplesMs);
  } finally {
    server.kill('SIGTERM');
    rmSync(resultsDir, { recursive: true, force: true });
  }
});
