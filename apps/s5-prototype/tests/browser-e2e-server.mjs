import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const binary = resolve(process.env.SHARDRAILD_BIN ?? '../../target/debug/shardraild');
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
const hostPublicKey = run(['identity']).match(/host_public_key=([0-9a-f]+)/)?.[1];
if (!hostPublicKey) throw new Error('shardraild did not report its identity');
const dist = resolve('dist');
const daemon = spawn(binary, [
  '--data-dir', dataDir,
  '--browser-listen', '127.0.0.1:42937',
  '--browser-assets-dir', dist,
], { stdio: 'ignore' });
let stopping = false;
async function cleanup() {
  if (stopping) return;
  stopping = true;
  daemon.kill('SIGTERM');
  await new Promise((resolveDone) => {
    const timer = setTimeout(() => { daemon.kill('SIGKILL'); resolveDone(); }, 2000);
    daemon.once('exit', () => { clearTimeout(timer); resolveDone(); });
  });
  await rm(stateFile, { force: true });
  await rm(dataDir, { recursive: true, force: true });
}
process.once('SIGTERM', () => { void cleanup().finally(() => process.exit()); });
process.once('SIGINT', () => { void cleanup().finally(() => process.exit()); });
await writeFile(stateFile, JSON.stringify({ dataDir, clientId: client.client_id, publicKey: client.public_key, privateKey, hostPublicKey }));
for (let attempt = 0; attempt < 100; attempt++) {
  try {
    const response = await fetch('http://127.0.0.1:42937/');
    if (response.ok) break;
  } catch (error) {
    if (attempt === 99) throw error;
  }
  if (daemon.exitCode !== null) throw new Error(`test daemon exited with ${daemon.exitCode}`);
  await new Promise((resolveDelay) => setTimeout(resolveDelay, 100));
  if (attempt === 99) throw new Error('test daemon did not serve the built SPA');
}
await new Promise(() => {});
