import { test, expect } from '@playwright/test';
import { connect } from '@shardrail/client';
import { randomUUID } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

type State = { dataDir: string; clientId: string; publicKey: string; privateKey: string; hostPublicKey: string };
const state = JSON.parse(await readFile(join(tmpdir(), 'shardrail-browser-e2e-state.json'), 'utf8')) as State;

test('pairs, reloads with the pinned browser key, reconnects, and receives a daemon event', async ({ page }) => {
  const code = execFileSync(process.env.SHARDRAILD_BIN ?? '../../target/debug/shardraild', ['--data-dir', state.dataDir, 'client', 'enroll', '--label', 'Playwright browser'], { encoding: 'utf8' }).match(/enrollment_code=([0-9a-f]+)/)?.[1];
  if (!code) throw new Error('shardraild did not issue an enrollment code');
  await page.goto('http://127.0.0.1:42937/');
  await expect(page.getByRole('heading', { name: 'Pair this browser' })).toBeVisible();
  await page.getByRole('button', { name: 'Review fingerprint' }).click();
  await page.getByRole('textbox', { name: 'One-time enrollment code' }).fill(code);
  await page.getByRole('button', { name: 'I verified this fingerprint — pair' }).click();
  await expect(page.getByText('Connected · browser WebSocket')).toBeVisible();
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
});
