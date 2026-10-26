import { expect, test, type Page } from '@playwright/test';
import { execFileSync, spawn, type ChildProcess } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { chmod, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';

const binary = resolve(process.env.SHARDRAILD_BIN ?? '../../target/debug/shardraild');
const assets = resolve(process.env.SHARDRAIL_E2E_ASSETS_DIR ?? 'dist');
const fakeCodex = resolve('../../crates/shardraild/tests/fixtures/fake_codex_app_server.py');

let daemon: ChildProcess | null = null;
let dataDir = '';
let workdir = '';
let baseUrl = '';

async function stopDaemon() {
  const child = daemon;
  daemon = null;
  if (!child || child.exitCode !== null) return;
  child.kill('SIGTERM');
  await new Promise<void>((resolveDone) => {
    const timer = setTimeout(() => { child.kill('SIGKILL'); resolveDone(); }, 2000);
    child.once('exit', () => { clearTimeout(timer); resolveDone(); });
  });
}

async function pair(page: Page, allowCreate: boolean, root = dataDir, url = baseUrl) {
  const args = ['--data-dir', root, 'client', 'enroll', '--label', `Playwright new session ${randomUUID()}`];
  if (allowCreate) args.push('--allow-session-create');
  const code = execFileSync(binary, args, { encoding: 'utf8' }).match(/enrollment_code=([0-9a-f]+)/)?.[1];
  if (!code) throw new Error('shardraild did not issue an enrollment code');
  await page.goto(url);
  await page.getByRole('button', { name: 'Review fingerprint' }).click();
  await page.getByRole('textbox', { name: 'One-time enrollment code' }).fill(code);
  await page.getByRole('button', { name: 'I verified this fingerprint — pair' }).click();
  await expect(page.getByRole('heading', { name: 'Shardrail' })).toBeVisible();
  await expect(page.getByText('Connected · browser WebSocket')).toBeVisible();
}

test.beforeAll(async () => {
  await chmod(fakeCodex, 0o755);
  dataDir = await mkdtemp(`${tmpdir()}/shardrail-new-session-`);
  workdir = await mkdtemp(`${tmpdir()}/shardrail-new-session-work-`);
  daemon = spawn(binary, [
    '--data-dir', dataDir,
    '--no-file-log',
    '--codex-app-server', fakeCodex,
    '--session-workdir', workdir,
    '--browser-listen', '127.0.0.1:0',
    '--browser-assets-dir', assets,
  ], { stdio: ['ignore', 'ignore', 'pipe'], env: { ...process.env, SHARDRAIL_CODEX_FAKE_MODE: 'happy' } });
  let stderr = '';
  baseUrl = await new Promise<string>((resolveUrl, reject) => {
    const timer = setTimeout(() => reject(new Error(`daemon did not listen: ${stderr}`)), 10_000);
    daemon?.stderr?.setEncoding('utf8');
    daemon?.stderr?.on('data', (chunk: string) => {
      stderr += chunk;
      const port = stderr.match(/browser WebSocket listening at ws:\/\/127\.0\.0\.1:(\d+)\/ws/)?.[1];
      if (port) { clearTimeout(timer); resolveUrl(`http://127.0.0.1:${port}/`); }
    });
    daemon?.once('exit', (code) => { clearTimeout(timer); reject(new Error(`daemon exited ${code}: ${stderr}`)); });
  });
});

test.afterAll(async () => {
  await stopDaemon();
  await rm(dataDir, { recursive: true, force: true });
  await rm(workdir, { recursive: true, force: true });
});

test('a browser with session.create can start a session and send a prompt', async ({ page }) => {
  test.setTimeout(60_000);
  await pair(page, true);
  const workdir = page.getByRole('button', { name: /Workdir/ });
  await expect(workdir).toBeVisible();
  await workdir.click();
  await expect(page.getByRole('option')).toHaveCount(1);
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'New session' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Waiting for the daemon to confirm the new session.' })).toBeVisible();
  const option = page.getByRole('option').filter({ hasText: /[0-9a-f]{8}-/ });
  await expect(option).toBeVisible({ timeout: 15_000 });
  await expect(page.getByRole('status').filter({ hasText: 'Waiting for the daemon to confirm the new session.' })).toHaveCount(0);
  const sessionId = (await option.innerText()).match(/[0-9a-f]{8}-[0-9a-f-]{27}/)?.[0];
  if (!sessionId) throw new Error('new session id was not shown');
  await expect(page.getByRole('heading', { name: `Events for ${sessionId}` })).toBeVisible();
  await page.locator('#session-prompt').fill('Add a function sub');
  await page.getByRole('button', { name: 'Send prompt' }).click();
  await expect(page.getByRole('button', { name: 'Deny' })).toBeVisible({ timeout: 15_000 });
  await expect(page.locator('[data-check=sandbox]')).toHaveAttribute('data-status', 'verified');
  await expect(page.locator('[data-check=bypass]')).toHaveAttribute('data-status', 'verified');
  await expect(page.locator('[data-check=routing]')).toHaveAttribute('data-status', 'verified');
  await expect(page.locator('[data-check=config]')).toHaveAttribute('data-status', 'verified');
  await expect(page.locator('[data-check=config]')).toContainText(/config snapshot (session-start|pre-turn) sha256:[0-9a-f]{64}/);
  await expect(page.locator('[data-check=config]')).toContainText('Last checked');
  await expect(page.locator('[data-check=widening]')).toHaveAttribute('data-status', 'verified');
  await expect(page.locator('[data-check=mcp]')).toHaveAttribute('data-status', 'verified');
  await expect(page.locator('[data-check=mcp]')).toContainText('MCP servers disabled');
  await expect(page.locator('[data-check=mcp]')).toContainText('zero MCP');
  for (const id of ['containment', 'credentials', 'writes', 'identity', 'boundary', 'gaps']) {
    await expect(page.locator(`[data-check=${id}]`)).toHaveAttribute('data-status', 'not verified');
    await expect(page.locator(`[data-check=${id}]`)).toContainText(/process group|SSH_AUTH_SOCK|workspace-write|socket|journal|read-only/);
  }
  await expect(page.locator('.host-banner')).toContainText('approvals not protected');
  await expect(page.locator('.host-banner')).toContainText('containment, credentials, writes, identity, boundary, gaps not verified');
  await expect(page.locator('.host-banner')).not.toContainText('session checks verified');
});

test('config drift fails the row and blocks the prompt', async ({ page }) => {
  test.setTimeout(60_000);
  const driftData = await mkdtemp(`${tmpdir()}/shardrail-new-session-drift-`);
  const driftWork = await mkdtemp(`${tmpdir()}/shardrail-new-session-drift-work-`);
  const drift = spawn(binary, [
    '--data-dir', driftData,
    '--no-file-log',
    '--codex-app-server', fakeCodex,
    '--session-workdir', driftWork,
    '--browser-listen', '127.0.0.1:0',
    '--browser-assets-dir', assets,
  ], { stdio: ['ignore', 'ignore', 'pipe'], env: { ...process.env, SHARDRAIL_CODEX_FAKE_MODE: 'config-drift-policy' } });
  let stderr = '';
  try {
    const url = await new Promise<string>((resolveUrl, reject) => {
      const timer = setTimeout(() => reject(new Error(`daemon did not listen: ${stderr}`)), 10_000);
      drift.stderr?.setEncoding('utf8');
      drift.stderr?.on('data', (chunk: string) => {
        stderr += chunk;
        const port = stderr.match(/browser WebSocket listening at ws:\/\/127\.0\.0\.1:(\d+)\/ws/)?.[1];
        if (port) { clearTimeout(timer); resolveUrl(`http://127.0.0.1:${port}/`); }
      });
      drift.once('exit', (code) => { clearTimeout(timer); reject(new Error(`daemon exited ${code}: ${stderr}`)); });
    });
    await pair(page, true, driftData, url);
    await page.getByRole('button', { name: 'New session' }).click();
    const option = page.getByRole('option').filter({ hasText: /[0-9a-f]{8}-/ });
    await expect(option).toBeVisible({ timeout: 15_000 });
    await expect(page.locator('[data-check=config]')).toHaveAttribute('data-status', 'verified');
    await page.locator('#session-prompt').fill('hi');
    await page.getByRole('button', { name: 'Send prompt' }).click();
    await expect(page.locator('.prompt').getByRole('alert')).toContainText('config drifted: approval_policy', { timeout: 15_000 });
    await expect(page.locator('[data-check=config]')).toHaveAttribute('data-status', 'failed');
    await expect(page.locator('.host-banner')).toContainText('approvals not protected');
    await expect(page.locator('.host-banner')).toContainText('config failed');
    await expect(page.getByRole('button', { name: 'Deny' })).toHaveCount(0);
  } finally {
    if (drift.exitCode === null) {
      drift.kill('SIGTERM');
      await new Promise<void>((resolveDone) => {
        const timer = setTimeout(() => { drift.kill('SIGKILL'); resolveDone(); }, 2000);
        drift.once('exit', () => { clearTimeout(timer); resolveDone(); });
      });
    }
    await rm(driftData, { recursive: true, force: true });
    await rm(driftWork, { recursive: true, force: true });
  }
});

test('a failed sandbox echo shows failed and blocks the next prompt', async ({ page }) => {
  test.setTimeout(60_000);
  const failedData = await mkdtemp(`${tmpdir()}/shardrail-new-session-sandbox-`);
  const failedWork = await mkdtemp(`${tmpdir()}/shardrail-new-session-sandbox-work-`);
  const failed = spawn(binary, [
    '--data-dir', failedData,
    '--no-file-log',
    '--codex-app-server', fakeCodex,
    '--session-workdir', failedWork,
    '--browser-listen', '127.0.0.1:0',
    '--browser-assets-dir', assets,
  ], { stdio: ['ignore', 'ignore', 'pipe'], env: { ...process.env, SHARDRAIL_CODEX_FAKE_MODE: 'readonly-network' } });
  let stderr = '';
  try {
    const url = await new Promise<string>((resolveUrl, reject) => {
      const timer = setTimeout(() => reject(new Error(`daemon did not listen: ${stderr}`)), 10_000);
      failed.stderr?.setEncoding('utf8');
      failed.stderr?.on('data', (chunk: string) => {
        stderr += chunk;
        const port = stderr.match(/browser WebSocket listening at ws:\/\/127\.0\.0\.1:(\d+)\/ws/)?.[1];
        if (port) { clearTimeout(timer); resolveUrl(`http://127.0.0.1:${port}/`); }
      });
      failed.once('exit', (code) => { clearTimeout(timer); reject(new Error(`daemon exited ${code}: ${stderr}`)); });
    });
    await pair(page, true, failedData, url);
    await page.getByRole('button', { name: 'New session' }).click();
    const option = page.getByRole('option').filter({ hasText: /[0-9a-f]{8}-/ });
    await expect(option).toBeVisible({ timeout: 15_000 });
    await option.focus();
    await expect(page.locator('[data-check=sandbox]')).toHaveAttribute('data-status', 'failed');
    await expect(page.locator('.host-banner')).toContainText('approvals not protected');
    await expect(page.locator('.host-banner')).toContainText('sandbox failed');
    await page.locator('#session-prompt').fill('hi');
    await page.getByRole('button', { name: 'Send prompt' }).click();
    await expect(page.locator('.prompt').getByRole('alert')).toContainText('codex sandbox is not workspace-write or read-only', { timeout: 15_000 });
    await expect(page.locator('[data-check=sandbox]')).toHaveAttribute('data-status', 'failed');
  } finally {
    if (failed.exitCode === null) {
      failed.kill('SIGTERM');
      await new Promise<void>((resolveDone) => {
        const timer = setTimeout(() => { failed.kill('SIGKILL'); resolveDone(); }, 2000);
        failed.once('exit', () => { clearTimeout(timer); resolveDone(); });
      });
    }
    await rm(failedData, { recursive: true, force: true });
    await rm(failedWork, { recursive: true, force: true });
  }
});

test('a daemon rejection of New session stays on the create alert', async ({ page }) => {
  test.setTimeout(60_000);
  const rejectedData = await mkdtemp(`${tmpdir()}/shardrail-new-session-rejected-`);
  const rejectedWork = await mkdtemp(`${tmpdir()}/shardrail-new-session-rejected-work-`);
  const rejected = spawn(binary, [
    '--data-dir', rejectedData,
    '--no-file-log',
    '--codex-app-server', fakeCodex,
    '--session-workdir', rejectedWork,
    '--browser-listen', '127.0.0.1:0',
    '--browser-assets-dir', assets,
  ], { stdio: ['ignore', 'ignore', 'pipe'], env: { ...process.env, SHARDRAIL_CODEX_FAKE_MODE: 'extensions-on' } });
  let stderr = '';
  try {
    const url = await new Promise<string>((resolveUrl, reject) => {
      const timer = setTimeout(() => reject(new Error(`daemon did not listen: ${stderr}`)), 10_000);
      rejected.stderr?.setEncoding('utf8');
      rejected.stderr?.on('data', (chunk: string) => {
        stderr += chunk;
        const port = stderr.match(/browser WebSocket listening at ws:\/\/127\.0\.0\.1:(\d+)\/ws/)?.[1];
        if (port) { clearTimeout(timer); resolveUrl(`http://127.0.0.1:${port}/`); }
      });
      rejected.once('exit', (code) => { clearTimeout(timer); reject(new Error(`daemon exited ${code}: ${stderr}`)); });
    });
    await pair(page, true, rejectedData, url);
    await page.getByRole('button', { name: 'New session' }).click();
    await expect(page.getByRole('alert').filter({ hasText: 'enabled plugins remain' })).toBeVisible({ timeout: 15_000 });
    await expect(page.getByRole('option').filter({ hasText: /[0-9a-f]{8}-/ })).toHaveCount(0);
    await expect(page.getByText('No sessions yet.')).toBeVisible();
  } finally {
    if (rejected.exitCode === null) {
      rejected.kill('SIGTERM');
      await new Promise<void>((resolveDone) => {
        const timer = setTimeout(() => { rejected.kill('SIGKILL'); resolveDone(); }, 2000);
        rejected.once('exit', () => { clearTimeout(timer); resolveDone(); });
      });
    }
    await rm(rejectedData, { recursive: true, force: true });
    await rm(rejectedWork, { recursive: true, force: true });
  }
});

test('a browser without session.create has no New session button', async ({ browser }) => {
  const context = await browser.newContext();
  const page = await context.newPage();
  try {
    await pair(page, false);
    await expect(page.getByRole('button', { name: 'New session' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: /Workdir/ })).toHaveCount(0);
    await expect(page.getByText('Session creation unavailable')).toBeVisible();
  } finally {
    await context.close();
  }
});
