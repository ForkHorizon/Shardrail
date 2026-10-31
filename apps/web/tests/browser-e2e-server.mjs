import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';

const binary = resolve(process.env.SHARDRAILD_BIN ?? '../../target/debug/shardraild');
const fakeAcp = resolve(process.env.SHARDRAIL_FAKE_ACP_BIN ?? join(dirname(binary), 'shardrail-fake-acp'));
const stateFile = process.env.SHARDRAIL_E2E_STATE;
if (!stateFile) throw new Error('SHARDRAIL_E2E_STATE is required');
const dataDir = await mkdtemp(join(tmpdir(), 'shardrail-browser-e2e-'));
const run = (args) => {
  const result = spawnSync(binary, ['--data-dir', dataDir, ...args], { encoding: 'utf8' });
  if (result.status !== 0) throw new Error(result.stderr || `shardraild ${args.join(' ')} failed`);
  return result.stdout;
};
run(['client', 'add', '--label', 'Playwright event source', '--scope', 'full']);
const registry = JSON.parse(await readFile(join(dataDir, 'authorized-clients.json'), 'utf8'));
const client = registry.clients[0];
const privateKey = (await readFile(join(dataDir, `client-${client.client_id}.key`))).toString('hex');
const identity = run(['identity']);
const hostPublicKey = identity.match(/host_public_key=([0-9a-f]+)/)?.[1];
const hostFingerprint = identity.match(/host_fingerprint=([0-9a-f]+)/)?.[1];
if (!hostPublicKey || !hostFingerprint) throw new Error('shardraild did not report its identity');
const dist = resolve(process.env.SHARDRAIL_E2E_ASSETS_DIR ?? 'dist');
const daemon = spawn(binary, [
  '--data-dir', dataDir,
  '--no-file-log',
  '--browser-listen', '127.0.0.1:0',
  '--browser-assets-dir', dist,
  '--fake-acp-path', fakeAcp,
], { stdio: ['ignore', 'ignore', 'pipe'] });
let stopping = false;
let stderr = '';
const endpoint = new Promise((resolveEndpoint, reject) => {
  const timeout = setTimeout(() => reject(new Error(`daemon did not report its browser listener: ${stderr}`)), 10_000);
  daemon.stderr.setEncoding('utf8');
  daemon.stderr.on('data', (chunk) => {
    stderr += chunk;
    const address = stderr.match(/browser WebSocket listening at ws:\/\/127\.0\.0\.1:(\d+)\/ws/)?.[1];
    if (address) {
      clearTimeout(timeout);
      resolveEndpoint(`http://127.0.0.1:${address}/`);
    }
  });
  daemon.once('exit', (code) => reject(new Error(`test daemon exited with ${code}: ${stderr}`)));
});

async function cleanup() {
  if (stopping) return;
  stopping = true;
  daemon.kill('SIGTERM');
  await new Promise((resolveDone) => {
    const timer = setTimeout(() => { daemon.kill('SIGKILL'); resolveDone(); }, 2000);
    daemon.once('exit', () => { clearTimeout(timer); resolveDone(); });
  });
  try {
    const current = JSON.parse(await readFile(stateFile, 'utf8'));
    if (current.daemonPid && current.daemonPid !== daemon.pid) {
      try { process.kill(current.daemonPid, 'SIGTERM'); } catch (error) {
        if (error.code !== 'ESRCH') throw error;
      }
      for (let attempt = 0; attempt < 100; attempt++) {
        try { process.kill(current.daemonPid, 0); } catch { break; }
        await new Promise((resolve) => setTimeout(resolve, 25));
      }
    }
  } catch (error) {
    if (error.code !== 'ENOENT' && !(error instanceof SyntaxError)) throw error;
  }
  await rm(stateFile, { force: true });
  await rm(dataDir, { recursive: true, force: true });
}
try {
  if (process.env.SHARDRAIL_E2E_TEST_PIDS_FILE) {
    await writeFile(process.env.SHARDRAIL_E2E_TEST_PIDS_FILE, JSON.stringify({ serverPid: process.pid, daemonPid: daemon.pid }));
  }
  const baseUrl = await endpoint;
  const ready = await fetch(baseUrl);
  if (!ready.ok) throw new Error(`test daemon served ${ready.status}`);
  const stateTemp = `${stateFile}.${process.pid}.tmp`;
  try {
    const state = process.env.SHARDRAIL_E2E_TEST_INVALID_STATE === '1'
      ? '{"baseUrl":'
      : JSON.stringify({ baseUrl, dataDir, clientId: client.client_id, publicKey: client.public_key, privateKey, hostPublicKey, hostFingerprint, daemonPid: daemon.pid, assetsDir: dist, browserPort: new URL(baseUrl).port });
    await writeFile(stateTemp, state);
    await rename(stateTemp, stateFile);
  } catch (error) {
    await rm(stateTemp, { force: true });
    throw error;
  }
  await new Promise((resolve) => {
    const keepAlive = setInterval(() => {}, 60_000);
    const stop = () => { clearInterval(keepAlive); resolve(); };
    process.once('SIGTERM', stop);
    process.once('SIGINT', stop);
  });
} finally {
  await cleanup();
}
