import { test, expect, type Page } from '@playwright/test';
import { connect } from '@shardrail/client';
import { execFileSync, spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
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
  const daemon = spawn(binary, ['--data-dir', state.dataDir, '--browser-listen', `127.0.0.1:${state.browserPort}`, '--browser-assets-dir', state.assetsDir, '--fake-acp-path', fakeAcp], { stdio: ['ignore', 'ignore', 'pipe'] });
  let stderr = '';
  try {
    await new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`restarted daemon did not start: ${stderr}`)), 10_000);
      daemon.stderr.setEncoding('utf8');
      daemon.stderr.on('data', (chunk: string) => {
        stderr += chunk;
        if (stderr.includes(`127.0.0.1:${state.browserPort}/ws`)) { clearTimeout(timer); resolve(); }
      });
      daemon.once('exit', (code) => { clearTimeout(timer); reject(new Error(`restarted daemon exited with ${code}: ${stderr}`)); });
    });
  } catch (error) {
    daemon.kill('SIGTERM');
    throw error;
  }
  state.daemonPid = daemon.pid!;
  await writeFile(stateFile, JSON.stringify(state));
}

async function pair(page: Page) {
  const binary = process.env.SHARDRAILD_BIN ?? '../../target/debug/shardraild';
  const code = execFileSync(binary, ['--data-dir', state.dataDir, 'client', 'enroll', '--label', `Playwright main ${randomUUID()}`], { encoding: 'utf8' }).match(/enrollment_code=([0-9a-f]+)/)?.[1];
  if (!code) throw new Error('shardraild did not issue an enrollment code');
  await page.goto(state.baseUrl);
  await page.getByRole('button', { name: 'Review fingerprint' }).click();
  await page.getByRole('textbox', { name: 'One-time enrollment code' }).fill(code);
  await page.getByRole('button', { name: 'I verified this fingerprint — pair' }).click();
  await expect(page.getByRole('heading', { name: 'Shardrail' })).toBeVisible();
  await expect(page.getByText('Connected · browser WebSocket')).toBeVisible();
}

function eventCount(page: Page) {
  return page.evaluate(() => (window as Window & { __s5Metrics: () => { eventStoreCount: number } }).__s5Metrics().eventStoreCount);
}

async function createSession() {
  const commandId = randomUUID();
  const client = await connect({
    socketPath: join(state.dataDir, 'shardraild.sock'),
    clientId: state.clientId,
    publicKey: state.publicKey,
    privateKey: Uint8Array.from(Buffer.from(state.privateKey, 'hex')),
    hostPublicKey: state.hostPublicKey,
  });
  try {
    const sessionId = await new Promise<string>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('session was not created')), 10_000);
      client.onFrame((frame) => {
        if (frame.kind !== 'durable_event' || frame.event.kind !== 'session_created') return;
        const payload = frame.event.payload as { command_id?: string };
        if (payload.command_id !== commandId) return;
        clearTimeout(timer);
        resolve(frame.event.session_id);
      });
      client.request({ command_id: commandId, kind: 'session.create', expected_state: null, expected_turn_id: null, payload: {} });
    });
    return sessionId;
  } finally {
    client.close();
  }
}

test('lists a paired session, streams it, denies once, and reconnects without duplicates', async ({ page }) => {
  await pair(page);
  await expect(page.getByRole('listbox', { name: 'Sessions' }).or(page.getByText('No sessions yet.'))).toBeVisible();
  const sessionId = await createSession();
  const option = page.getByRole('option', { name: new RegExp(sessionId) });
  await expect(option).toBeVisible();
  await expect(option).toContainText(/agent unknown|fake-acp/);
  await option.focus();
  await expect(page.getByRole('heading', { name: `Events for ${sessionId}` })).toBeVisible({ timeout: 10_000 });
  await expect(page.getByText('fake streamed response 0')).toHaveCount(1);
  const deny = page.getByRole('button', { name: 'Deny' });
  await expect(deny).toBeEnabled();
  await deny.click();
  const approval = page.getByRole('article', { name: /Approval/ });
  await expect(approval).toContainText(/responded_unacked|denied/);
  await expect(approval).not.toContainText(/\ballowed\b/);
  await expect(option).toContainText(/exited/i, { timeout: 15_000 });

  let previous = -1;
  let settled = 0;
  await expect.poll(async () => {
    const count = await eventCount(page);
    const stable = count === previous && count > 0;
    previous = count;
    if (stable) settled = count;
    return stable;
  }).toBe(true);
  expect(await page.getByText('fake streamed response 0').count()).toBe(1);

  await stopDaemon(state.daemonPid);
  await expect(page.getByText('Reconnecting · browser WebSocket')).toBeVisible();
  await startDaemon();
  await page.getByRole('button', { name: 'Reconnect' }).click();
  await expect(page.getByText('Connected · browser WebSocket')).toBeVisible({ timeout: 15_000 });
  await expect.poll(() => eventCount(page)).toBe(settled);
  await expect(page.getByText('fake streamed response 0')).toHaveCount(1);

  const nextSessionId = await createSession();
  const nextOption = page.getByRole('option', { name: new RegExp(nextSessionId) });
  await expect(nextOption).toBeVisible();
  await nextOption.focus();
  await expect(page.getByRole('heading', { name: `Events for ${nextSessionId}` })).toBeVisible();
  await expect(page.getByText('fake streamed response 0')).toHaveCount(1);
  await page.keyboard.press('ArrowDown');
  await expect(page.getByRole('heading', { name: /Events for / })).toBeVisible();
  await expect.poll(() => eventCount(page)).toBeGreaterThan(settled);
});
