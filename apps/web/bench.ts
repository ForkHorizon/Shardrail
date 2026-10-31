import { chromium, webkit, type Browser, type BrowserType } from '@playwright/test';
import { spawn } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { dirname } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import os from 'node:os';
import { minimumGeneratedTokens, playwrightOutputPath, withScenarioConfiguration } from './src/bench-utils';

const base = 'http://127.0.0.1:4173';
const repetitions = Number(process.env.S5_REPETITIONS ?? 5);
const smoke = process.env.S5_BENCH_SMOKE === '1';
const engines: [string, BrowserType][] = smoke ? [['webkit', webkit], ['chromium', chromium]] : [['webkit', webkit], ['chromium', chromium]];
const scenarios: [number, number][] = smoke ? [[0, 1]] : [[0, 1], ...[5000, 50000].flatMap(n => [1, 4, 8].map(s => [n, s] as [number, number]))];
const durationMs = 6000;
const results: Record<string, unknown>[] = [];
const browserVersions: Record<string, string> = { playwright: '1.64.1' };
const outputPath = playwrightOutputPath(smoke, process.env.S5_BENCH_OUTPUT);
execFileSync('npm', ['run', 'build'], { stdio: 'inherit' });
const preview = spawn('npm', ['run', 'preview', '--', '--host', '127.0.0.1', '--port', '4173', '--strictPort'], { stdio: 'ignore' });
async function waitPreview() {
  for (let n = 0; n < 80; n++) { try { await fetch(base); return; } catch { await delay(250); } }
  throw new Error('Vite production preview did not become ready on 127.0.0.1:4173');
}
async function openPage(browser: Browser, events: number, streams: number) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  const page = await context.newPage();
  const errors: string[] = [];
  page.on('pageerror', e => errors.push(e.message));
  const url = `${base}/?events=${events}&streams=${streams}&duration=${durationMs}&autostart=${events ? '1' : '0'}`;
  const navigationStarted = Date.now();
  await page.goto(url, { waitUntil: 'networkidle' });
  await page.waitForFunction(() => (window as Window & { __s5Metrics?: () => { eventStoreCount: number } }).__s5Metrics?.().eventStoreCount >= 0);
  return { context, page, errors, navigationMs: Date.now() - navigationStarted };
}
async function run(browser: Browser, engine: string, temperature: 'cold' | 'warm', repetition: number, events: number, streams: number, coldLaunchMs: number | null) {
  const { context, page, errors, navigationMs } = await openPage(browser, events, streams);
  const inputText = events ? '/s5-loaded-turn-input-trace' : '/s5-empty-turn-input-trace';
  const requiredTokens = minimumGeneratedTokens(streams, durationMs);
  try {
    if (events) await page.waitForFunction(target => (window as Window & { __s5Metrics?: () => { generatedTokens: number } }).__s5Metrics?.().generatedTokens >= target, requiredTokens, { timeout: 15000 });
    const firstTranscriptRenderOpportunityMs = await page.evaluate(() => (window as Window & { __s5Metrics: () => { firstTranscriptRenderOpportunityMs: number } }).__s5Metrics().firstTranscriptRenderOpportunityMs);
    await page.evaluate(() => (window as Window & { __s5ResetInputTrace: () => void }).__s5ResetInputTrace());
    await page.locator('input[aria-label="Message composer"]').pressSequentially(inputText, { delay: 35 });
    await page.waitForFunction(expected => {
      const m = (window as Window & { __s5Metrics?: () => { composerSamples: number; firstFrameSamples: number } }).__s5Metrics?.();
      return m?.composerSamples >= expected && m.firstFrameSamples >= expected;
    }, inputText.length);
    const measure = await page.evaluate(() => (window as Window & { __s5Metrics: () => object }).__s5Metrics());
    const scrollSmoothness = await page.evaluate(`new Promise(resolve => {
      const el = document.querySelector('.transcript');
      const intervals = [];
      let previous = performance.now();
      const until = previous + 1000;
      function tick(time) {
        intervals.push(time - previous); previous = time;
        el.scrollTop = (el.scrollTop + 14) % Math.max(1, el.scrollHeight - el.clientHeight);
        if (time >= until) { intervals.sort((a, b) => a - b); resolve({ frameP95Ms: intervals[Math.min(intervals.length - 1, Math.floor(intervals.length * .95))] || null, frameMaxMs: Math.max(...intervals), frames: intervals.length }); }
        else requestAnimationFrame(tick);
      }
      requestAnimationFrame(tick);
    })`);
    if (events) await page.waitForFunction(target => {
      const footer = document.querySelector('footer')?.textContent ?? '';
      const m = (window as Window & { __s5Metrics?: () => { generatedTokens: number } }).__s5Metrics?.();
      return footer.includes('Ready') && m?.generatedTokens >= target;
    }, requiredTokens, { timeout: 15000 });
    results.push({ engine, temperature, repetition, ...withScenarioConfiguration(measure, events, streams), scenario: events ? `${events} events × ${streams} streams` : 'empty turn', coldLaunchMs, navigationMs, firstTranscriptRenderOpportunityMs, requiredGeneratedTokens: events ? requiredTokens : 0, scrollSmoothness, errors });
    console.log(JSON.stringify(results.at(-1)));
  } catch (error) {
    results.push({ engine, temperature, repetition, events, streams, coldLaunchMs, navigationMs, error: String(error), errors });
    console.error(JSON.stringify(results.at(-1)));
  } finally { await context.close(); }
}
try {
  await waitPreview();
  for (const [engine, type] of engines) {
    for (const [events, streams] of scenarios) {
      for (let repetition = 1; repetition <= (smoke ? 1 : repetitions); repetition++) {
        const launchStarted = Date.now();
        const browser = await type.launch({ headless: false, args: engine === 'chromium' ? ['--no-sandbox'] : [] });
        browserVersions[engine] = browser.version();
        await run(browser, engine, 'cold', repetition, events, streams, Date.now() - launchStarted);
        await browser.close();
      }
    }
    if (!smoke) {
      const browser = await type.launch({ headless: false, args: engine === 'chromium' ? ['--no-sandbox'] : [] });
      browserVersions[engine] = browser.version();
      for (const [events, streams] of scenarios) {
        for (let repetition = 1; repetition <= repetitions; repetition++) await run(browser, engine, 'warm', repetition, events, streams, null);
      }
      await browser.close();
    }
  }
} finally {
  preview.kill('SIGTERM');
  await mkdir('results', { recursive: true });
  await mkdir(dirname(outputPath), { recursive: true });
  await writeFile(outputPath, JSON.stringify({ date: new Date().toISOString(), mode: 'production preview', repetitions, machine: { model: 'MacBook Pro Mac17,2', chip: 'Apple M5', memoryGB: 32, macOS: '27.0.1 (26A434)', kernel: os.release(), arch: os.arch() }, browserVersions, results }, null, 2));
}
if (results.length !== engines.length * scenarios.length * (smoke ? 1 : repetitions * 2) || results.some(r => 'error' in r || (r as { errors?: string[] }).errors?.length)) process.exitCode = 1;
