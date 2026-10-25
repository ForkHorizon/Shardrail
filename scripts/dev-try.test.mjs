import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createServer } from 'node:net';
import { existsSync } from 'node:fs';
import { chmod, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts/dev-try.sh');
const fake = join(root, 'crates/shardraild/tests/fixtures/fake_codex_app_server.py');
const binary = join(root, 'target/debug/shardraild');
const dist = join(root, 'apps/s5-prototype/dist');

function freePort() {
  return new Promise((resolvePort, reject) => {
    const server = createServer();
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      const port = address && typeof address === 'object' ? address.port : 0;
      server.close(() => resolvePort(port));
    });
    server.on('error', reject);
  });
}

async function ensureAssets() {
  if (existsSync(dist)) return false;
  await mkdir(dist, { recursive: true });
  await writeFile(join(dist, 'index.html'), '<!doctype html><title>shardrail</title>\n');
  return true;
}

test('dev-try prints an enrollment code and removes its data dir', async () => {
  assert.equal(existsSync(binary), true, 'build shardraild before this test');
  await chmod(fake, 0o755);
  const madeDist = await ensureAssets();
  const workdir = await mkdtemp(join(tmpdir(), 'shardrail-try-work-'));
  const port = await freePort();
  const child = spawn('sh', [script, '--skip-build', '--codex', fake, '--port', String(port), '--workdir', workdir], {
    cwd: root,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let stdout = '';
  let stderr = '';
  child.stdout.setEncoding('utf8');
  child.stderr.setEncoding('utf8');
  child.stdout.on('data', (chunk) => { stdout += chunk; });
  child.stderr.on('data', (chunk) => { stderr += chunk; });
  let settled = false;
  try {
    const code = await new Promise((resolveCode, reject) => {
      const timer = setTimeout(() => reject(new Error(`timed out\n${stdout}\n${stderr}`)), 20_000);
      const finish = (error, value) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        if (error) reject(error);
        else resolveCode(value);
      };
      // data_dir is printed on the next line. A pipe can deliver that write
      // after the code line, so wait until both are present.
      child.stdout.on('data', () => {
        const found = stdout.match(/enrollment_code=([0-9a-f]{64})/);
        if (found && /data_dir=\S+/.test(stdout)) finish(null, found[1]);
      });
      child.once('exit', (status) => finish(new Error(`exited ${status}\n${stdout}\n${stderr}`)));
    });
    assert.match(code, /^[0-9a-f]{64}$/);
    assert.match(stdout, new RegExp(`url=http://127\\.0\\.0\\.1:${port}/`));
    assert.match(stdout, /does not open a browser/);
    const dataDir = stdout.match(/data_dir=(\S+)/)?.[1];
    assert.ok(dataDir);
    assert.equal(existsSync(dataDir), true);
    child.kill('SIGINT');
    await new Promise((resolveExit) => child.once('exit', resolveExit));
    for (let attempt = 0; attempt < 50 && existsSync(dataDir); attempt += 1) {
      await new Promise((resolveWait) => setTimeout(resolveWait, 100));
    }
    assert.equal(existsSync(dataDir), false, `data dir was left behind: ${dataDir}`);
    const stillBusy = await new Promise((resolveBusy) => {
      const probe = createServer();
      probe.once('error', () => resolveBusy(true));
      probe.listen(port, '127.0.0.1', () => probe.close(() => resolveBusy(false)));
    });
    assert.equal(stillBusy, false);
  } finally {
    if (child.exitCode === null) child.kill('SIGKILL');
    await rm(workdir, { recursive: true, force: true });
    if (madeDist) await rm(dist, { recursive: true, force: true });
  }
});

test('dev-try dry-run prints the commands and does not enroll', async () => {
  await chmod(fake, 0o755);
  const port = await freePort();
  const result = spawnSync('sh', [script, '--dry-run', '--skip-build', '--codex', fake, '--port', String(port), '--workdir', '/tmp'], {
    cwd: root,
    encoding: 'utf8',
    timeout: 10_000,
  });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /shardraild/);
  assert.match(result.stdout, /--allow-session-create/);
  assert.match(result.stdout, /model_reasoning_effort/);
  assert.match(result.stdout, new RegExp(`127\\.0\\.0\\.1:${port}`));
  assert.doesNotMatch(result.stdout, /enrollment_code=[0-9a-f]{64}/);
});

test('dev-try refuses a dry-run when codex is missing or the port is busy', async () => {
  const missing = spawnSync('sh', [script, '--dry-run', '--codex', join(tmpdir(), 'no-such-codex')], {
    cwd: root,
    encoding: 'utf8',
    timeout: 10_000,
  });
  assert.notEqual(missing.status, 0);
  assert.match(missing.stderr, /codex is missing/);

  await chmod(fake, 0o755);
  const port = await freePort();
  const holder = createServer();
  await new Promise((resolveListen) => holder.listen(port, '127.0.0.1', resolveListen));
  try {
    const busy = spawnSync('sh', [script, '--dry-run', '--codex', fake, '--port', String(port)], {
      cwd: root,
      encoding: 'utf8',
      timeout: 10_000,
    });
    assert.notEqual(busy.status, 0);
    assert.match(busy.stderr, /already in use/);
  } finally {
    await new Promise((resolveClose) => holder.close(() => resolveClose()));
  }
});
