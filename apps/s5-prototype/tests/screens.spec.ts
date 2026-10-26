import { test, expect } from '@playwright/test';
import { connect } from '@shardrail/client';
import { execFileSync, spawn, type ChildProcess } from 'node:child_process';
import { readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';

type State = { baseUrl: string; dataDir: string; daemonPid: number; assetsDir: string; browserPort: string; clientId: string; publicKey: string; privateKey: string; hostPublicKey: string };
const stateFile = process.env.SHARDRAIL_E2E_STATE ?? join(tmpdir(), 'shardrail-browser-screens-state.json');
const state = JSON.parse(await readFile(stateFile, 'utf8')) as State;
const outDir = process.env.SHARDRAIL_SCREEN_OUT;
if (!outDir) throw new Error('SHARDRAIL_SCREEN_OUT is required');

async function startDaemon() {
  const binary = process.env.SHARDRAILD_BIN ?? '../../target/debug/shardraild';
  const fakeAcp = process.env.SHARDRAIL_FAKE_ACP_BIN ?? resolve(dirname(binary), 'shardrail-fake-acp');
  const daemon: ChildProcess = spawn(binary, ['--data-dir', state.dataDir, '--no-file-log', '--browser-listen', `127.0.0.1:${state.browserPort}`, '--browser-assets-dir', state.assetsDir, '--fake-acp-path', fakeAcp], { stdio: ['ignore', 'ignore', 'pipe'] });
  await new Promise<void>((resolve, reject) => {
    let stderr = '';
    const timer = setTimeout(() => reject(new Error(`screenshot daemon did not restart: ${stderr}`)), 10_000);
    daemon.stderr?.setEncoding('utf8');
    daemon.stderr?.on('data', (chunk: string) => {
      stderr += chunk;
      if (stderr.includes(`127.0.0.1:${state.browserPort}/ws`)) { clearTimeout(timer); resolve(); }
    });
    daemon.once('exit', (code) => { clearTimeout(timer); reject(new Error(`screenshot daemon exited ${code}: ${stderr}`)); });
  });
  state.daemonPid = daemon.pid!;
  await writeFile(stateFile, JSON.stringify(state));
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
    return await new Promise<string>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('test daemon did not create a session')), 10_000);
      client.onFrame((frame) => {
        if (frame.kind !== 'durable_event' || frame.event.kind !== 'session_created') return;
        if ((frame.event.payload as { command_id?: string }).command_id !== commandId) return;
        clearTimeout(timer);
        resolve(frame.event.session_id);
      });
      client.request({ command_id: commandId, kind: 'session.create', expected_state: null, expected_turn_id: null, payload: {} });
    });
  } finally {
    client.close();
  }
}

test('capture the browser UI states at desktop and phone sizes in both themes', async ({ browser }) => {
  test.setTimeout(240_000);
  const variants = [
    { size: 'desktop', viewport: { width: 1440, height: 900 } },
    { size: 'phone', viewport: { width: 390, height: 844 } },
  ] as const;
  const themes = ['light', 'dark'] as const;
  const results: string[] = [];
  const captures: { suffix: string; page: import('@playwright/test').Page; context: import('@playwright/test').BrowserContext; capture: (number: string, name: string, ready: () => Promise<unknown>) => Promise<void> }[] = [];
  for (const variant of variants) for (const colorScheme of themes) {
    const context = await browser.newContext({ viewport: variant.viewport, colorScheme });
    const page = await context.newPage();
    const suffix = `${variant.size}-${colorScheme}`;
    const capture = async (number: string, name: string, ready: () => Promise<unknown>) => {
      try {
        await ready();
        await page.screenshot({ path: `${outDir}/${number}-${name}-${suffix}.png`, fullPage: true });
        results.push(`CAPTURE ${number}-${name}-${suffix}`);
      } catch (error) {
        results.push(`SKIP ${number}-${name}-${suffix}: ${String(error).split('\n')[0]}`);
      }
    };
    captures.push({ suffix, page, context, capture });
    try {
      await page.goto(state.baseUrl);
      await capture('01', 'pairing', () => expect(page.getByRole('heading', { name: 'Pair this browser' })).toBeVisible());
      await capture('02', 'pairing-error', async () => {
        await page.getByRole('button', { name: 'Review fingerprint' }).click();
        await page.getByRole('textbox', { name: 'One-time enrollment code' }).fill('00000000');
        await page.getByRole('button', { name: 'I verified this fingerprint — pair' }).click();
        await expect(page.getByRole('alert')).toBeVisible({ timeout: 5_000 });
      });
      const binary = process.env.SHARDRAILD_BIN ?? '../../target/debug/shardraild';
      const code = execFileSync(binary, ['--data-dir', state.dataDir, 'client', 'enroll', '--label', `Screenshot ${randomUUID()}`, '--allow-session-create'], { encoding: 'utf8' }).match(/enrollment_code=([0-9a-f]+)/)?.[1];
      if (!code) throw new Error('test daemon did not issue an enrollment code');
      await page.reload();
      await page.getByRole('button', { name: 'Review fingerprint' }).click();
      await page.getByRole('textbox', { name: 'One-time enrollment code' }).fill(code);
      await page.getByRole('button', { name: 'I verified this fingerprint — pair' }).click();
      await expect(page.getByText('Connected · browser WebSocket')).toBeVisible();
      await capture('03', 'sessions-empty', () => expect(page.getByText('No sessions yet.')).toBeVisible());
      await capture('04', 'new-session', () => expect(page.getByRole('button', { name: 'New session' })).toBeVisible());
    } catch (error) {
      results.push(`SKIP remaining ${suffix}: ${String(error).split('\n')[0]}`);
    }
  }

  try {
    const sessionId = await createSession();
    for (const { suffix, page, capture } of captures) {
      try {
        const sessionOption = page.getByRole('option', { name: new RegExp(sessionId) });
        await expect(sessionOption).toBeVisible({ timeout: 10_000 });
        await sessionOption.focus();
        await capture('05', 'session-streaming', async () => {
          await expect(page.locator('.transcript')).toContainText(/fake streamed response/);
          await expect(page.locator('.transcript')).toContainText(/tool|command|file/i);
        });
        await capture('06', 'approval-pending', () => expect(page.getByRole('button', { name: 'Allow once' }).first()).toBeVisible());
        await page.getByRole('button', { name: 'Allow once' }).first().click();
        await capture('07', 'approval-confirm', () => expect(page.getByRole('button', { name: 'Confirm allow once' }).first()).toBeVisible());
        await capture('08', 'checklist', () => expect(page.getByRole('heading', { name: 'Verification checklist' })).toBeVisible());
      } catch (error) {
        results.push(`SKIP remaining session screens ${suffix}: ${String(error).split('\n')[0]}`);
      }
    }
    try { process.kill(state.daemonPid, 'SIGTERM'); } catch { /* already stopped */ }
    for (const { page, capture } of captures) {
      await capture('09', 'offline', () => expect(page.getByText('Reconnecting · browser WebSocket')).toBeVisible({ timeout: 8_000 }));
      await capture('10', 'error', async () => {
        await page.locator('#session-prompt').fill('screenshot blocked prompt');
        await page.getByRole('button', { name: 'Send prompt' }).click();
        await expect(page.locator('.prompt').getByRole('alert')).toBeVisible();
      });
    }
    await startDaemon();
  } catch (error) {
    results.push(`SKIP session/offline states: ${String(error).split('\n')[0]}`);
  } finally {
    await Promise.all(captures.map(({ context }) => context.close()));
  }
  console.log(`\nScreenshot summary (${results.filter((line) => line.startsWith('CAPTURE')).length} captured, ${results.filter((line) => line.startsWith('SKIP')).length} skipped):\n${results.join('\n')}`);
});
