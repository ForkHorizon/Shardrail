import { test, expect } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import { connect } from '@shardrail/client';
import { createServer } from 'node:http';
import { randomBytes, randomUUID } from 'node:crypto';
import { chmod, readFile, writeFile } from 'node:fs/promises';
import { execFileSync, spawn } from 'node:child_process';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';

type State = { baseUrl: string; dataDir: string; clientId: string; publicKey: string; privateKey: string; hostPublicKey: string; hostFingerprint: string; daemonPid: number; assetsDir: string; browserPort: string };
const stateFile = process.env.SHARDRAIL_E2E_STATE ?? join(tmpdir(), 'shardrail-browser-e2e-state.json');
const state = JSON.parse(await readFile(stateFile, 'utf8')) as State;

async function stopDaemon(pid: number) {
  try { process.kill(pid, 'SIGTERM'); } catch { return; }
  for (let attempt = 0; attempt < 100; attempt++) {
    try { process.kill(pid, 0); } catch { return; }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  process.kill(pid, 'SIGKILL');
}

async function startDaemon() {
  const binary = process.env.SHARDRAILD_BIN ?? '../../target/debug/shardraild';
  const fakeAcp = process.env.SHARDRAIL_FAKE_ACP_BIN ?? join(dirname(binary), 'shardrail-fake-acp');
  const daemon = spawn(binary, ['--data-dir', state.dataDir, '--no-file-log', '--browser-listen', `127.0.0.1:${state.browserPort}`, '--browser-assets-dir', state.assetsDir, '--fake-acp-path', fakeAcp], { stdio: ['ignore', 'ignore', 'pipe'] });
  let stderr = '';
  try {
    await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`rotated daemon did not start: ${stderr}`)), 10_000);
    daemon.stderr.setEncoding('utf8');
    daemon.stderr.on('data', (chunk: string) => {
      stderr += chunk;
      if (stderr.includes(`127.0.0.1:${state.browserPort}/ws`)) { clearTimeout(timer); resolve(); }
    });
    daemon.once('exit', (code) => { clearTimeout(timer); reject(new Error(`rotated daemon exited with ${code}: ${stderr}`)); });
    });
  } catch (error) {
    daemon.kill('SIGTERM');
    throw error;
  }
  state.daemonPid = daemon.pid!;
  await writeFile(stateFile, JSON.stringify(state));
  return daemon.pid!;
}

test('pairing page has no axe violations in either theme', async ({ page }) => {
  await page.goto(state.baseUrl);
  const initial = (await page.evaluate(() => matchMedia('(prefers-color-scheme: dark)').matches)) ? 'dark' : 'light';
  const first = (await new AxeBuilder({ page }).setLegacyMode().analyze()).violations;
  expect(first, first.map((violation) => `${violation.id}: ${violation.help}`).join('\n')).toEqual([]);
  const next = initial === 'dark' ? 'light' : 'dark';
  await page.getByRole('button', { name: `Use ${next} theme` }).click();
  const second = (await new AxeBuilder({ page }).setLegacyMode().analyze()).violations;
  expect(second, second.map((violation) => `${violation.id}: ${violation.help}`).join('\n')).toEqual([]);
});

test('pairs, persists the key on reload, reconnects, receives events, and blocks a changed daemon key', async ({ page }) => {
  const binary = process.env.SHARDRAILD_BIN ?? '../../target/debug/shardraild';
  const code = execFileSync(binary, ['--data-dir', state.dataDir, 'client', 'enroll', '--label', 'Playwright browser'], { encoding: 'utf8' }).match(/enrollment_code=([0-9a-f]+)/)?.[1];
  if (!code) throw new Error('shardraild did not issue an enrollment code');
  await page.goto(state.baseUrl);
  await expect(page.getByRole('heading', { name: 'Pair this browser' })).toBeVisible();
  await page.getByRole('button', { name: 'Review fingerprint' }).click();
  await expect(page.getByLabel('Daemon SHA-256 fingerprint')).toHaveText(state.hostFingerprint);
  await page.getByRole('textbox', { name: 'One-time enrollment code' }).fill(code);
  await page.getByRole('button', { name: 'I verified this fingerprint — pair' }).click();
  await expect(page.getByText('Connected · browser WebSocket')).toBeVisible();
  await expect(page.getByRole('note')).toHaveCount(0);
  await expect.poll(() => page.evaluate(async () => {
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const opening = indexedDB.open('shardrail-browser-keys', 1);
      opening.onsuccess = () => resolve(opening.result);
      opening.onerror = () => reject(opening.error);
    });
    return await new Promise<string>((resolve, reject) => {
      const request = db.transaction('clients').objectStore('clients').getAll();
      request.onsuccess = () => resolve(request.result[0]?.storage ?? 'missing');
      request.onerror = () => reject(request.error);
    });
  })).toBe('webcrypto');
  await page.reload();
  await expect(page.getByText('Connected · browser WebSocket')).toBeVisible();
  await page.getByRole('button', { name: 'Reconnect' }).click();
  await expect(page.getByText('Connected · browser WebSocket')).toBeVisible();

  const initialCount = await page.evaluate(() => (window as Window & { __s5Metrics: () => { eventStoreCount: number } }).__s5Metrics().eventStoreCount);
  const client = await connect({
    socketPath: join(state.dataDir, 'shardraild.sock'),
    clientId: state.clientId,
    publicKey: state.publicKey,
    privateKey: Uint8Array.from(Buffer.from(state.privateKey, 'hex')),
    hostPublicKey: state.hostPublicKey,
  });
  try {
    const delivered = new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('daemon event was not delivered')), 5000);
      client.onFrame((frame) => {
        if (frame.kind === 'durable_event' && frame.event.kind === 'session_created') {
          clearTimeout(timer);
          resolve();
        }
      });
    });
    client.request({ command_id: randomUUID(), kind: 'session.create', expected_state: null, expected_turn_id: null, payload: {} });
    await delivered;
    await expect.poll(() => page.evaluate(() => (window as Window & { __s5Metrics: () => { eventStoreCount: number } }).__s5Metrics().eventStoreCount)).toBeGreaterThan(initialCount);
  } finally {
    client.close();
  }

  const originalHostKey = await readFile(join(state.dataDir, 'host.key'));
  const rotatedKey = randomBytes(32);
  await stopDaemon(state.daemonPid);
  await writeFile(join(state.dataDir, 'host.key'), rotatedKey);
  await chmod(join(state.dataDir, 'host.key'), 0o600);
  try {
    await startDaemon();
    await page.reload();
    await expect(page.getByRole('alert')).toContainText('daemon key changed');
    await expect(page.getByRole('alert')).toContainText('Pairing is blocked');
  } finally {
    await stopDaemon(state.daemonPid);
    await writeFile(join(state.dataDir, 'host.key'), originalHostKey);
    await chmod(join(state.dataDir, 'host.key'), 0o600);
    await startDaemon();
  }
});

test('uses the disclosed Noble fallback when the test flag disables WebCrypto Ed25519', async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(window, '__SHARDRAIL_E2E_FORCE_NOBLE__', { value: true });
    const subtle = crypto.subtle;
    const generateKey = subtle.generateKey.bind(subtle);
    Object.defineProperty(subtle, 'generateKey', {
      configurable: true,
      value: (algorithm: AlgorithmIdentifier, extractable: boolean, keyUsages: KeyUsage[]) => {
        const forced = (window as Window & { __SHARDRAIL_E2E_FORCE_NOBLE__?: boolean }).__SHARDRAIL_E2E_FORCE_NOBLE__;
        if (forced && typeof algorithm === 'object' && 'name' in algorithm && algorithm.name === 'Ed25519') throw new DOMException('forced by e2e test flag', 'NotSupportedError');
        return generateKey(algorithm, extractable, keyUsages);
      },
    });
  });
  const binary = process.env.SHARDRAILD_BIN ?? '../../target/debug/shardraild';
  const code = execFileSync(binary, ['--data-dir', state.dataDir, 'client', 'enroll', '--label', `Playwright fallback ${randomUUID()}`], { encoding: 'utf8' }).match(/enrollment_code=([0-9a-f]+)/)?.[1];
  if (!code) throw new Error('shardraild did not issue an enrollment code');
  await page.goto(state.baseUrl);
  await page.getByRole('button', { name: 'Review fingerprint' }).click();
  await expect(page.getByLabel('Daemon SHA-256 fingerprint')).toHaveText(state.hostFingerprint);
  await page.getByRole('textbox', { name: 'One-time enrollment code' }).fill(code);
  await expect(page.getByRole('note')).toContainText('extractable private key seed');
  await expect(page.getByRole('button', { name: 'I verified this fingerprint — pair' })).toBeDisabled();
  const fallbackConsent = page.getByRole('checkbox', { name: 'I understand the browser key can be read by same-origin script' });
  await fallbackConsent.focus();
  await fallbackConsent.press('Space');
  await expect(fallbackConsent).toBeChecked();
  await page.getByRole('button', { name: 'I verified this fingerprint — pair' }).click();
  await expect(page.getByText('Connected · browser WebSocket')).toBeVisible();
  await expect(page.getByRole('note')).toContainText('extractable private key seed');
});

test('a browser on another origin cannot open the daemon WebSocket', async ({ page }) => {
  const foreign = createServer((_request, response) => {
    response.writeHead(200, { 'content-type': 'text/html' });
    response.end('<!doctype html><title>foreign origin</title>');
  });
  await new Promise<void>((resolve) => foreign.listen(0, '127.0.0.1', resolve));
  const address = foreign.address();
  if (!address || typeof address === 'string') throw new Error('foreign-origin server did not bind');
  try {
    await page.goto(`http://127.0.0.1:${address.port}/`);
    const opened = await page.evaluate(async (url) => new Promise<boolean>((resolve) => {
      const socket = new WebSocket(url);
      const timer = setTimeout(() => { socket.close(); resolve(false); }, 1500);
      socket.onopen = () => { clearTimeout(timer); socket.close(); resolve(true); };
      socket.onerror = () => { clearTimeout(timer); resolve(false); };
    }), `${new URL(state.baseUrl).origin.replace('http:', 'ws:')}/ws`);
    expect(opened).toBe(false);
  } finally {
    await new Promise<void>((resolve, reject) => foreign.close((error) => error ? reject(error) : resolve()));
  }
});
