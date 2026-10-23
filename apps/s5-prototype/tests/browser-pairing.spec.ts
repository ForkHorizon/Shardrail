import { test, expect } from '@playwright/test';
import { connect } from '@shardrail/client';
import { createServer } from 'node:http';
import { randomUUID } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

type State = { baseUrl: string; dataDir: string; clientId: string; publicKey: string; privateKey: string; hostPublicKey: string; hostFingerprint: string };
const stateFile = process.env.SHARDRAIL_E2E_STATE ?? join(tmpdir(), 'shardrail-browser-e2e-state.json');
const state = JSON.parse(await readFile(stateFile, 'utf8')) as State;

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

  await page.evaluate(() => localStorage.setItem('shardrail-daemon-public-key', '00'));
  await page.reload();
  await expect(page.getByRole('alert')).toContainText('daemon key changed');
  await expect(page.getByRole('alert')).toContainText('Pairing is blocked');
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
