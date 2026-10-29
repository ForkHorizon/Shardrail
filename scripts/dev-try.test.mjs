import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash, createPublicKey, generateKeyPairSync, randomBytes, randomUUID, sign, verify } from 'node:crypto';
import { createConnection, createServer } from 'node:net';
import { existsSync, readFileSync } from 'node:fs';
import { chmod, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const ED25519_SPKI_PREFIX = Buffer.from('302a300506032b6570032100', 'hex');

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
    detached: true,
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
    await stopGroup(child);
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
  assert.match(result.stdout, /--exit-with-stdin/);
  assert.match(result.stdout, /--allow-session-create/);
  assert.doesNotMatch(result.stdout, /--browser-open/);
  assert.doesNotMatch(readFileSync(script, 'utf8'), /\/Users\/daliys/);
  assert.match(result.stdout, /model_reasoning_effort/);
  assert.match(result.stdout, new RegExp(`127\\.0\\.0\\.1:${port}`));
  assert.doesNotMatch(result.stdout, /enrollment_code=[0-9a-f]{64}/);
});

test('dev-try dry-run with --open-browser passes --browser-open to shardraild', async () => {
  await chmod(fake, 0o755);
  const port = await freePort();
  const result = spawnSync('sh', [script, '--dry-run', '--skip-build', '--open-browser', '--codex', fake, '--port', String(port), '--workdir', '/tmp'], {
    cwd: root,
    encoding: 'utf8',
    timeout: 10_000,
  });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /--browser-open/);
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

function stopGroup(child) {
  if (child.pid) {
    try { process.kill(-child.pid, 'SIGKILL'); } catch { /* already gone */ }
  }
  try { child.kill('SIGKILL'); } catch { /* already gone */ }
  return new Promise((resolveExit) => {
    if (child.exitCode !== null || child.signalCode !== null) resolveExit();
    else child.once('exit', () => resolveExit());
  });
}

function processLines(marker) {
  const result = spawnSync('ps', ['-axeww', '-o', 'pid=,command='], { encoding: 'utf8' });
  return (result.stdout || '').split('\n').filter((line) => line.includes(marker));
}

function pidAlive(pid) {
  try {
    process.kill(Number(pid), 0);
    return true;
  } catch {
    return false;
  }
}

async function launchTry() {
  assert.equal(existsSync(binary), true, 'build shardraild before this test');
  await chmod(fake, 0o755);
  const madeDist = await ensureAssets();
  const workdir = await mkdtemp(join(tmpdir(), 'shardrail-try-work-'));
  const pidFile = join(workdir, 'codex.pid');
  const port = await freePort();
  const child = spawn('sh', [script, '--skip-build', '--codex', fake, '--port', String(port), '--workdir', workdir], {
    cwd: root,
    detached: true,
    stdio: ['ignore', 'pipe', 'pipe'],
    env: {
      ...process.env,
      SHARDRAIL_CODEX_FAKE_MODE: 'happy',
      SHARDRAIL_CODEX_PID_OUT: pidFile,
    },
  });
  let stdout = '';
  let stderr = '';
  child.stdout.setEncoding('utf8');
  child.stderr.setEncoding('utf8');
  child.stdout.on('data', (chunk) => { stdout += chunk; });
  child.stderr.on('data', (chunk) => { stderr += chunk; });
  return { child, workdir, port, madeDist, pidFile, text: () => ({ stdout, stderr }) };
}

async function waitReady(session) {
  return new Promise((resolveReady, reject) => {
    const timer = setTimeout(() => {
      const { stdout, stderr } = session.text();
      reject(new Error(`timed out\n${stdout}\n${stderr}`));
    }, 20_000);
    let settled = false;
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (error) reject(error);
      else resolveReady(value);
    };
    const check = () => {
      const { stdout } = session.text();
      const code = stdout.match(/enrollment_code=([0-9a-f]{64})/);
      const dataDir = stdout.match(/data_dir=(\S+)/)?.[1];
      const daemonPid = stdout.match(/daemon_pid=(\d+)/)?.[1];
      if (code && dataDir && daemonPid) finish(null, { code: code[1], dataDir, daemonPid });
    };
    session.child.stdout.on('data', check);
    session.child.once('exit', (status) => {
      const { stdout, stderr } = session.text();
      finish(new Error(`exited ${status}\n${stdout}\n${stderr}`));
    });
    check();
  });
}

function transcriptHash(clientNonce, serverNonce, clientKey, hostKey) {
  const hash = createHash('sha256');
  hash.update('Shardrail mutual client handshake v1\0', 'utf8');
  for (const version of [1, 1, 1]) {
    const bytes = Buffer.alloc(4);
    bytes.writeUInt32BE(version);
    hash.update(bytes);
  }
  for (const field of [clientNonce, serverNonce, clientKey, hostKey]) {
    const raw = Buffer.from(field, 'hex');
    if (raw.length !== 32) throw new Error('handshake field is not 32 bytes');
    const length = Buffer.alloc(4);
    length.writeUInt32BE(raw.length);
    hash.update(length);
    hash.update(raw);
  }
  return hash.digest();
}

function openLineSocket(socketPath) {
  return new Promise((resolveSocket, rejectSocket) => {
    const socket = createConnection(socketPath);
    let settled = false;
    let buffer = '';
    const pending = [];
    const waiters = [];
    const failWaiters = (error) => {
      while (waiters.length) waiters.shift().reject(error);
    };
    const failConnect = (error) => {
      if (settled) {
        failWaiters(error);
        return;
      }
      settled = true;
      rejectSocket(error);
    };
    socket.setEncoding('utf8');
    socket.on('error', failConnect);
    socket.on('close', () => failConnect(new Error('daemon socket closed')));
    socket.on('data', (chunk) => {
      buffer += chunk;
      let newline = buffer.indexOf('\n');
      while (newline !== -1) {
        const line = buffer.slice(0, newline);
        buffer = buffer.slice(newline + 1);
        const waiter = waiters.shift();
        if (waiter) waiter.resolve(line);
        else pending.push(line);
        newline = buffer.indexOf('\n');
      }
    });
    socket.once('connect', () => {
      if (settled) return;
      settled = true;
      resolveSocket({
        send(value) {
          socket.write(`${JSON.stringify(value)}\n`);
        },
        next(timeoutMs) {
          const queued = pending.shift();
          if (queued !== undefined) return Promise.resolve(queued);
          return new Promise((resolveLine, rejectLine) => {
            const timer = setTimeout(() => {
              const index = waiters.findIndex((entry) => entry.reject === wrappedReject);
              if (index >= 0) waiters.splice(index, 1);
              rejectLine(new Error('timed out waiting for a daemon frame'));
            }, timeoutMs);
            const wrappedReject = (error) => {
              clearTimeout(timer);
              rejectLine(error);
            };
            waiters.push({
              resolve(line) {
                clearTimeout(timer);
                resolveLine(line);
              },
              reject: wrappedReject,
            });
          });
        },
        close() {
          socket.destroy();
        },
      });
    });
  });
}

async function startFakeSession(dataDir) {
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const spki = publicKey.export({ format: 'der', type: 'spki' });
  const publicHex = spki.subarray(spki.length - 32).toString('hex');
  const added = spawnSync(binary, [
    '--data-dir', dataDir,
    'client', 'add',
    '--label', 'signal-test',
    '--scope', 'full',
    '--public-key', publicHex,
  ], { encoding: 'utf8', timeout: 10_000 });
  assert.equal(added.status, 0, added.stderr);
  const clientId = added.stdout.match(/client_id=([0-9a-f-]+)/)?.[1];
  assert.ok(clientId, 'client add did not return an id');
  const socket = await openLineSocket(join(dataDir, 'shardraild.sock'));
  try {
    const clientNonce = randomBytes(32).toString('hex');
    socket.send({
      min_version: 1,
      max_version: 1,
      client_id: clientId,
      public_key: publicHex,
      nonce: clientNonce,
      resume_after: 0,
      journal_epoch: null,
    });
    const hello = JSON.parse(await socket.next(5_000));
    if (hello.error) throw new Error(hello.error);
    const digest = transcriptHash(clientNonce, hello.nonce, publicHex, hello.public_key);
    const hostKey = createPublicKey({
      key: Buffer.concat([ED25519_SPKI_PREFIX, Buffer.from(hello.public_key, 'hex')]),
      format: 'der',
      type: 'spki',
    });
    assert.equal(verify(null, digest, hostKey, Buffer.from(hello.signature, 'hex')), true, 'daemon handshake signature is invalid');
    socket.send({ kind: 'authenticate', signature: sign(null, digest, privateKey).toString('hex') });
    const authenticated = JSON.parse(await socket.next(5_000));
    assert.equal(authenticated.kind, 'authenticated', JSON.stringify(authenticated));
    socket.send({
      kind: 'request',
      request: {
        command_id: randomUUID(),
        kind: 'session.create',
        expected_state: null,
        expected_turn_id: null,
        payload: {},
      },
    });
    const deadline = Date.now() + 25_000;
    let reply;
    while (Date.now() < deadline) {
      const frame = JSON.parse(await socket.next(deadline - Date.now()));
      if (frame.kind === 'command_accepted' || frame.kind === 'command_rejected' || frame.kind === 'error') {
        reply = frame;
        break;
      }
    }
    assert.equal(reply?.kind, 'command_accepted', `session.create did not start the fake: ${JSON.stringify(reply)}`);
    return socket;
  } catch (error) {
    socket.close();
    throw error;
  }
}

function processIdentity(pid) {
  const result = spawnSync('ps', ['-p', String(pid), '-ww', '-o', 'ppid=,command='], { encoding: 'utf8' });
  const match = (result.stdout || '').trim().match(/^(\d+)\s+([\s\S]+)$/);
  if (!match) return null;
  return { ppid: match[1], command: match[2] };
}

async function readLiveCodexPid(pidFile, daemonPid) {
  const deadline = Date.now() + 5_000;
  let saw = 'missing';
  while (Date.now() < deadline) {
    if (existsSync(pidFile)) {
      const pid = readFileSync(pidFile, 'utf8').trim();
      const identity = /^\d+$/.test(pid) ? processIdentity(pid) : null;
      saw = identity ? `${pid} ppid=${identity.ppid}` : (pid || 'empty');
      if (
        identity
        && identity.ppid === String(daemonPid)
        && identity.command.includes('fake_codex_app_server.py')
      ) {
        return pid;
      }
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  }
  throw new Error(`fake codex child was not running under daemon ${daemonPid} (${saw})`);
}

async function waitGone(marker, pids) {
  const deadline = Date.now() + 8_000;
  let lines = [];
  while (Date.now() < deadline) {
    lines = processLines(marker);
    if (lines.length === 0 && pids.every((pid) => !pidAlive(pid))) return;
    await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  }
  throw new Error(`still running for ${marker}: ${lines.join(' | ')}`);
}

for (const signal of ['SIGINT', 'SIGTERM', 'SIGKILL']) {
  test(`dev-try ${signal} stops the daemon and the fake codex child`, { timeout: 60_000 }, async () => {
    const session = await launchTry();
    let dataDir = '';
    let socket;
    try {
      const ready = await waitReady(session);
      dataDir = ready.dataDir;
      assert.equal(existsSync(dataDir), true);
      assert.equal(pidAlive(ready.daemonPid), true);
      // The fake starts only for a session. Record that pid before the signal.
      socket = await startFakeSession(dataDir);
      const codexPid = await readLiveCodexPid(session.pidFile, ready.daemonPid);
      assert.notEqual(codexPid, String(ready.daemonPid));
      assert.equal(pidAlive(codexPid), true);
      process.kill(session.child.pid, signal);
      await new Promise((resolveExit, reject) => {
        const timer = setTimeout(() => reject(new Error(`${signal} did not stop the script`)), 5_000);
        if (session.child.exitCode !== null || session.child.signalCode !== null) {
          clearTimeout(timer);
          resolveExit();
          return;
        }
        session.child.once('exit', () => {
          clearTimeout(timer);
          resolveExit();
        });
      });
      await waitGone(dataDir, [ready.daemonPid, codexPid, String(session.child.pid)]);
      assert.equal(pidAlive(codexPid), false, `fake codex ${codexPid} survived ${signal}`);
      assert.equal(pidAlive(ready.daemonPid), false, `daemon ${ready.daemonPid} survived ${signal}`);
      if (signal !== 'SIGKILL') {
        for (let attempt = 0; attempt < 50 && existsSync(dataDir); attempt += 1) {
          await new Promise((resolveWait) => setTimeout(resolveWait, 100));
        }
        assert.equal(existsSync(dataDir), false, `data dir was left behind: ${dataDir}`);
      }
    } finally {
      socket?.close();
      await stopGroup(session.child);
      await rm(session.workdir, { recursive: true, force: true });
      if (dataDir) await rm(dataDir, { recursive: true, force: true });
      if (session.madeDist) await rm(dist, { recursive: true, force: true });
    }
  });
}
