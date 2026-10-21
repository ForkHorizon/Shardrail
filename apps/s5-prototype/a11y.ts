import AxeBuilder from '@axe-core/playwright';
import { webkit, type Browser } from '@playwright/test';
import assert from 'node:assert/strict';
import { spawn, type ChildProcess } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import { mkdir, writeFile } from 'node:fs/promises';

const server: ChildProcess = spawn('npm', ['run', 'dev'], { stdio: 'ignore' });
let browser: Browser | undefined;
let context: Awaited<ReturnType<Browser['newContext']>> | undefined;
const results: Record<string, unknown>[] = [];
try {
  let ready = false;
  for (let n = 0; n < 60; n++) { try { await fetch('http://127.0.0.1:5173'); ready = true; break; } catch { await delay(250); } }
  if (!ready) throw new Error('Vite did not become ready on 127.0.0.1:5173');
  browser = await webkit.launch({ headless: true });
  context = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  const page = await context.newPage();
  await page.goto('http://127.0.0.1:5173/?events=0&streams=1&autostart=0', { waitUntil: 'networkidle' });
  await page.waitForFunction(() => (window as Window & { __s5Metrics?: () => { eventStoreCount: number } }).__s5Metrics?.().eventStoreCount === 0);
  assert.equal(await page.evaluate(() => (window as Window & { __s5Metrics: () => { frameSamples: number } }).__s5Metrics().frameSamples), 0);
  await page.evaluate(() => (window as Window & { __s5ResetInputTrace: () => void }).__s5ResetInputTrace());
  await page.locator('input[aria-label="Message composer"]').pressSequentially('s5emptyturninputtrace', { delay: 40 });
  await page.waitForFunction(() => (window as Window & { __s5Metrics?: () => { composerP95: number | null; composerSamples: number } }).__s5Metrics?.().composerP95 !== null && (window as Window & { __s5Metrics?: () => { composerSamples: number } }).__s5Metrics?.().composerSamples >= 21);
  const emptyViolations = (await new AxeBuilder({ page }).setLegacyMode().analyze()).violations;
  results.push({ screen: 'empty composer', violations: emptyViolations });
  if (emptyViolations.length) throw new Error(`Empty turn: ${emptyViolations.map((v) => `${v.id} (${v.nodes.length})`).join(', ')}`);

  await page.goto('http://127.0.0.1:5173/?events=5000&streams=1&duration=800&autostart=0', { waitUntil: 'networkidle' });
  await page.waitForFunction(() => (window as Window & { __s5Metrics?: () => { eventStoreCount: number } }).__s5Metrics?.().eventStoreCount >= 5000);
  await page.locator('iframe[title="Agent-authored Markdown"]').first().contentFrame().locator('p').first().waitFor();
  assert.equal(await page.evaluate(() => (window as Window & { __s5Metrics: () => { frameSamples: number } }).__s5Metrics().frameSamples), 0);
  await page.getByRole('button', { name: 'Start mock streams' }).click();
  await page.waitForFunction(() => (window as Window & { __s5Metrics?: () => { eventStoreCount: number; frameSamples: number } }).__s5Metrics?.().eventStoreCount > 5000 && (window as Window & { __s5Metrics?: () => { frameSamples: number } }).__s5Metrics?.().frameSamples > 0);
  await page.waitForFunction(() => document.querySelector('footer')?.textContent?.includes('Streams running'));
  await page.waitForFunction(() => document.querySelector('footer')?.textContent?.includes('Ready'));
  const stoppedFrames = await page.evaluate(() => (window as Window & { __s5Metrics: () => { frameSamples: number } }).__s5Metrics().frameSamples);
  await delay(120);
  assert.equal(await page.evaluate(() => (window as Window & { __s5Metrics: () => { frameSamples: number } }).__s5Metrics().frameSamples), stoppedFrames);
  const approvalViolations = (await new AxeBuilder({ page }).setLegacyMode().analyze()).violations;
  results.push({ screen: 'loaded transcript', violations: approvalViolations });
  if (approvalViolations.length) throw new Error(`Approval/transcript: ${approvalViolations.map((v) => `${v.id} (${v.nodes.length})`).join(', ')}`);
} finally {
  await browser?.close();
  server.kill('SIGTERM');
  await mkdir('results', { recursive: true });
  await writeFile('results/a11y.json', JSON.stringify({ date: new Date().toISOString(), engine: 'Playwright WebKit', axe: '4.14.0', results }, null, 2));
}
