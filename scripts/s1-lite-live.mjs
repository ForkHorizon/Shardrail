#!/usr/bin/env node
// Real Codex app-server smoke for S1-lite.
// Exits immediately unless SHARDRAIL_S1_LIVE=1. Headless Chromium only.
// The daemon browser listener binds to 127.0.0.1. This starts the owner's
// existing Codex login; do not run it from an agent session.

import { spawn, spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { createRequire } from 'node:module';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { existsSync, readFileSync, realpathSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { INITIAL_COMMIT_PREFIX, assertHeadIsCommit, assertRepoIdentity, assertResultDirectory, codexHomePathHits, turn1Failure } from './s1-lite-guard.mjs';

const TEST_REPO = '/Users/daliys/Daliys/Shardrail/spikes/s1-testrepo';
const SHARDRAIL = '/Users/daliys/Daliys/Shardrail';
const HARD_TIMEOUT_MS = 15 * 60 * 1000;
const TURN_WAIT_MS = 8 * 60 * 1000;
const worktree = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const s5Dir = join(worktree, 'apps/s5-prototype');

if (!process.execArgv.includes('--experimental-strip-types')) {
  const child = spawn(process.execPath, ['--experimental-strip-types', fileURLToPath(import.meta.url)], {
    stdio: 'inherit',
    env: process.env,
  });
  child.on('exit', (code, signal) => process.exit(code ?? (signal ? 1 : 0)));
} else {
  await main();
}

async function main() {
  if (process.env.SHARDRAIL_S1_LIVE !== '1') {
    console.error('Refusing to run. Set SHARDRAIL_S1_LIVE=1 for the real Codex smoke.');
    process.exit(2);
  }
  if (!process.env.S1_RESULT_DIR) {
    console.error('S1_RESULT_DIR is required and must be outside the Shardrail checkouts.');
    process.exit(2);
  }

  const started = Date.now();
  const timings = { started_at: new Date(started).toISOString() };
  const mark = (name) => { timings[name] = Date.now() - started; };
  const daemonLog = [];
  let daemon = null;
  let browser = null;
  let client = null;
  let dataDir = null;
  let assetsDir = null;
  let resultDir = null;
  let settling = false;

  const shutdown = async (code) => {
    if (settling) return;
    settling = true;
    try { client?.close(); } catch { /* socket already closed */ }
    try { await browser?.close(); } catch { /* browser already closed */ }
    if (daemon && daemon.exitCode === null) {
      daemon.kill('SIGTERM');
      await new Promise((resolveDone) => {
        const timer = setTimeout(() => {
          if (daemon.exitCode === null) daemon.kill('SIGKILL');
          resolveDone();
        }, 3000);
        daemon.once('exit', () => { clearTimeout(timer); resolveDone(); });
      });
    }
    if (dataDir) await rm(dataDir, { recursive: true, force: true }).catch(() => undefined);
    if (assetsDir) await rm(assetsDir, { recursive: true, force: true }).catch(() => undefined);
    process.exit(code);
  };
  const hardTimer = setTimeout(() => {
    console.error('S1-lite hard timeout (15 minutes).');
    void shutdown(1);
  }, HARD_TIMEOUT_MS);

  try {
    const testRepo = assertTestRepo();
    resultDir = prepareResultDir(process.env.S1_RESULT_DIR);
    const statusBefore = { shardrail: porcelain(SHARDRAIL), worktree: porcelain(worktree) };
    resetTestRepo(testRepo);
    mark('repo_reset_ms');
    ensureS5Dependencies();
    ensureClientDependencies();
    dataDir = await mkdtemp(join(tmpdir(), 'shardrail-s1-lite-'));
    assetsDir = await mkdtemp(join(tmpdir(), 'shardrail-s1-lite-assets-'));
    const binary = ensureDaemonBinary();
    buildAssets(assetsDir);
    mark('assets_built_ms');
    const full = addFullClient(binary, dataDir);
    const identity = daemonIdentity(binary, dataDir);
    daemon = spawn(binary, [
      '--data-dir', dataDir,
      '--no-file-log',
      '--codex-app-server', codexProgram(),
      '--codex-workdir', testRepo.repo,
      '--codex-config', 'model_reasoning_effort="low"',
      '--browser-listen', '127.0.0.1:0',
      '--browser-assets-dir', assetsDir,
    ], { stdio: ['ignore', 'ignore', 'pipe'] });
    daemon.stderr.setEncoding('utf8');
    daemon.stderr.on('data', (chunk) => daemonLog.push(chunk));
    const baseUrl = await waitForBrowser(daemon, daemonLog);
    mark('daemon_ready_ms');

    const { connect } = await import(pathToFileURL(join(worktree, 'packages/client/src/index.ts')).href);
    const frames = [];
    client = await connect({
      socketPath: join(dataDir, 'shardraild.sock'),
      clientId: full.clientId,
      publicKey: full.publicKey,
      privateKey: Buffer.from(full.privateKey, 'hex'),
      hostPublicKey: identity.hostPublicKey,
    });
    client.onFrame((frame) => frames.push(frame));
    client.onError((error) => console.error(`full client: ${error.message}`));
    // The browser creates the session. This full client only watches the journal:
    // the transcript virtualizes rows, and the turn driver counts journal events.

    const requireFromS5 = createRequire(join(s5Dir, 'package.json'));
    const playwright = await import(pathToFileURL(requireFromS5.resolve('playwright-core')).href);
    const chromium = playwright.chromium ?? playwright.default?.chromium;
    if (!chromium) throw new Error('playwright-core did not expose chromium');
    browser = await chromium.launch({ headless: true, channel: 'chromium' });
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    page.setDefaultTimeout(120_000);
    await pairBrowser(page, binary, dataDir, baseUrl);
    mark('browser_paired_ms');
    const sessionId = await createSessionInBrowser(page);
    mark('session_created_ms');
    await waitUntil(
      () => frames.some((frame) => frame.kind === 'durable_event' && frame.event.kind === 'session_created' && frame.event.session_id === sessionId),
      30_000,
      'observer session_created',
    );
    const streamed = await page.evaluate(() => window.__s5Metrics?.().eventStoreCount ?? 0);
    if (!(streamed > 0)) throw new Error('browser main screen did not stream session events');

    await sendPrompt(page, 'Add a function `sub(a, b)` to calc.py and a unittest for it, then run `python3 -m unittest`.');
    mark('turn1_sent_ms');
    let turn1 = await driveTurn1(page, frames, sessionId);
    mark('turn1_done_ms');
    if (!turn1.allowed) {
      // Codex often stops after a denial. Ask once more and allow each request once.
      const retry = await driveAllowTurn(page, frames, sessionId);
      mark('turn1b_done_ms');
      turn1 = { decisions: [...turn1.decisions, ...retry.decisions], allowed: retry.allowed, outcome: retry.outcome };
    }

    const chunksBefore = countEvents(frames, sessionId, 'agent_message_chunk');
    await sendPrompt(page, 'Explain calc.py in detail, step by step');
    mark('turn2_sent_ms');
    // The transcript virtualizes rows, so the first delta is a journal event.
    await waitUntil(
      () => countEvents(frames, sessionId, 'agent_message_chunk') > chunksBefore,
      TURN_WAIT_MS,
      'turn 2 agent_message_chunk',
    );
    mark('turn2_first_delta_ms');
    await page.getByRole('button', { name: 'Interrupt' }).click();
    mark('turn2_interrupt_sent_ms');
    const interrupted = await waitForEvent(
      frames,
      (frame) => turnFinished(frame, sessionId) && isInterrupted(frame),
      TURN_WAIT_MS,
      'interrupted turn_finished',
    );
    mark('turn2_done_ms');

    const events = frames.filter((frame) => frame.kind === 'durable_event').map((frame) => frame.event);
    const verifiedExtensions = events.filter((event) => event.kind === 'codex_extensions_verified');
    const rejectedExtensions = events.filter((event) => event.kind === 'codex_extensions_rejected');
    const extensionLog = daemonLog.join('').split('\n').filter((line) => line.includes('codex extension'));
    const codexHomeHits = [
      ...codexHomePathHits(events),
      ...codexHomePathHits(extensionLog.map((line) => ({ kind: 'daemon_log', payload: { line } }))),
    ];
    const refused = events.filter((event) => event.kind === 'provider_request_refused');
    const usage = events.filter((event) => JSON.stringify(event.payload ?? {}).toLowerCase().includes('token'));
    const statusAfter = { shardrail: porcelain(SHARDRAIL), worktree: porcelain(worktree) };
    const shardrailUnchanged = statusBefore.shardrail === statusAfter.shardrail && statusBefore.worktree === statusAfter.worktree;
    const result = {
      session_id: sessionId,
      thread_ids: uniqueStrings(events, 'thread_id'),
      turn_ids: uniqueStrings(events, 'turn_id'),
      event_counts_by_kind: countKinds(events),
      approvals: events.filter((event) => String(event.kind).includes('approval') || event.kind === 'denied' || event.kind === 'allowed' || event.kind === 'expired').map(summarizeEvent),
      decisions: turn1.decisions,
      interrupt: {
        outcome: interrupted.event.payload?.provider_status ?? interrupted.event.payload?.session_snapshot?.last_turn_outcome ?? null,
        event_id: interrupted.event.event_id,
      },
      unknown_request: refused.length ? refused.map(summarizeEvent) : 'not observed',
      timings,
      token_usage: usage.map(summarizeEvent),
      test_repo: {
        status: git(testRepo.repo, ['status', '--porcelain']),
        diff_stat: git(testRepo.repo, ['diff', '--stat']),
      },
      allow_once: turn1.allowed ? 'observed' : 'not observed',
      routing_rejected: events.filter((event) => event.kind === 'approval_routing_rejected').map(summarizeEvent),
      extensions: {
        verified: verifiedExtensions.map(summarizeEvent),
        rejected: rejectedExtensions.map(summarizeEvent),
        log: extensionLog,
      },
      codex_home_paths: codexHomeHits,
      shardrail_git_status_unchanged: shardrailUnchanged,
    };
    await writeResult(resultDir, result, daemonLog);
    if (!shardrailUnchanged) throw new Error('Shardrail or worktree git status changed during the live run');
    if (result.routing_rejected.length) throw new Error('the daemon rejected approval routing during the live run');
    if (codexHomeHits.length) throw new Error(`journal references ${[...new Set(codexHomeHits.flatMap((hit) => hit.markers))].join(', ')}`);
    if (rejectedExtensions.length) throw new Error('codex extensions were rejected');
    if (!verifiedExtensions.length) throw new Error('codex extension verification was not journaled');
    const turnFailure = turn1Failure({ ...turn1, finished: true });
    if (turnFailure) throw new Error(turnFailure);
    console.log(`S1-lite result written to ${resultDir}`);
    clearTimeout(hardTimer);
    await shutdown(0);
  } catch (error) {
    console.error(error instanceof Error ? error.stack ?? error.message : error);
    if (resultDir) {
      await writeFile(join(resultDir, 's1-lite-result.json'), `${JSON.stringify({ error: String(error), timings, daemon_log_tail: daemonLog.join('').slice(-4000) }, null, 2)}\n`).catch(() => undefined);
    }
    clearTimeout(hardTimer);
    await shutdown(1);
  }
}

function assertTestRepo() {
  return assertRepoIdentity({
    testRepo: TEST_REPO,
    shardrail: SHARDRAIL,
    worktree,
    commitPrefix: INITIAL_COMMIT_PREFIX,
    git: (cwd, args) => git(cwd, args),
  });
}

function resetTestRepo(identity) {
  const reset = spawnSync('git', ['-C', identity.repo, 'reset', '--hard', identity.commit], { encoding: 'utf8' });
  if (reset.status !== 0) throw new Error(reset.stderr || 'git reset failed');
  const clean = spawnSync('git', ['-C', identity.repo, 'clean', '-fdx'], { encoding: 'utf8' });
  if (clean.status !== 0) throw new Error(clean.stderr || 'git clean failed');
  assertHeadIsCommit(git(identity.repo, ['rev-parse', 'HEAD']), identity.commit);
}

function prepareResultDir(path) {
  const absolute = resolve(path);
  const roots = [realpathSync(SHARDRAIL), realpathSync(worktree)];
  const allowed = join(roots[0], 'spikes/reviews19/live-1');
  spawnSync('mkdir', ['-p', absolute], { stdio: 'inherit' });
  const real = realpathSync(absolute);
  if (absolute === allowed && real !== allowed) throw new Error('S1_RESULT_DIR must not resolve through a symlink');
  return assertResultDirectory(real, { shardrail: roots[0], worktree: roots[1], allowedResultDir: allowed });
}

function porcelain(cwd) {
  return git(cwd, ['status', '--porcelain']);
}

function git(cwd, args) {
  const result = spawnSync('git', ['-C', cwd, ...args], { encoding: 'utf8' });
  if (result.status !== 0) throw new Error(result.stderr || `git ${args.join(' ')} failed`);
  return result.stdout;
}

function codexProgram() {
  const found = spawnSync('/bin/zsh', ['-lc', 'command -v codex'], { encoding: 'utf8' });
  const program = (found.stdout || '').trim().split('\n').filter(Boolean).at(-1) ?? '';
  if (!program.endsWith('/codex') && program !== 'codex') throw new Error(`codex is not on PATH (${program || 'empty'})`);
  return program;
}

function ensureClientDependencies() {
  const noble = join(worktree, 'packages/client/node_modules/@noble/ed25519/package.json');
  if (existsSync(noble)) return;
  const install = spawnSync('npm', ['ci'], { cwd: join(worktree, 'packages/client'), stdio: 'inherit' });
  if (install.status !== 0) throw new Error('npm ci in packages/client failed');
}

function ensureS5Dependencies() {
  const probe = spawnSync(process.execPath, ['--input-type=module', '-e', "import { createRequire } from 'node:module'; createRequire(process.argv[1]).resolve('playwright-core')", join(s5Dir, 'package.json')], { encoding: 'utf8' });
  if (probe.status === 0) return;
  const install = spawnSync('npm', ['ci'], { cwd: s5Dir, stdio: 'inherit' });
  if (install.status !== 0) throw new Error('npm ci in apps/s5-prototype failed');
}

function ensureDaemonBinary() {
  if (process.env.SHARDRAILD_BIN) return resolve(process.env.SHARDRAILD_BIN);
  const build = spawnSync('nice', ['cargo', 'build', '-p', 'shardraild', '--offline'], { cwd: worktree, stdio: 'inherit' });
  if (build.status !== 0) throw new Error('cargo build -p shardraild failed');
  return join(worktree, 'target/debug/shardraild');
}

function buildAssets(outDir) {
  const build = spawnSync('npm', ['run', 'build', '--', '--outDir', outDir], { cwd: s5Dir, stdio: 'inherit' });
  if (build.status !== 0) throw new Error('S5 asset build failed');
}

function addFullClient(binary, dataDir) {
  const added = spawnSync(binary, ['--data-dir', dataDir, 'client', 'add', '--label', 's1-lite-full', '--scope', 'full'], { encoding: 'utf8' });
  if (added.status !== 0) throw new Error(added.stderr || 'client add failed');
  const registry = JSON.parse(readFileSync(join(dataDir, 'authorized-clients.json'), 'utf8'));
  const client = registry.clients.at(-1);
  const privateKey = readFileSync(join(dataDir, `client-${client.client_id}.key`)).toString('hex');
  return { clientId: client.client_id, publicKey: client.public_key, privateKey };
}

function daemonIdentity(binary, dataDir) {
  const identity = spawnSync(binary, ['--data-dir', dataDir, 'identity'], { encoding: 'utf8' });
  if (identity.status !== 0) throw new Error(identity.stderr || 'identity failed');
  const hostPublicKey = identity.stdout.match(/host_public_key=([0-9a-f]+)/)?.[1];
  if (!hostPublicKey) throw new Error('daemon did not print a host public key');
  return { hostPublicKey };
}

function waitForBrowser(daemon, daemonLog) {
  return new Promise((resolveReady, reject) => {
    const timer = setTimeout(() => reject(new Error(`daemon did not listen: ${daemonLog.join('')}`)), 20_000);
    const look = () => {
      const port = daemonLog.join('').match(/WebSocket listening at ws:\/\/127\.0\.0\.1:(\d+)\/ws/)?.[1];
      if (!port) return;
      clearTimeout(timer);
      daemon.stderr.off('data', look);
      resolveReady(`http://127.0.0.1:${port}/`);
    };
    daemon.stderr.on('data', look);
    daemon.once('exit', (code) => {
      clearTimeout(timer);
      reject(new Error(`daemon exited ${code}: ${daemonLog.join('')}`));
    });
    look();
  });
}

async function createSessionInBrowser(page) {
  await page.locator('#session-workdir').waitFor({ timeout: 30_000 });
  await page.getByRole('button', { name: 'New session' }).click();
  const option = page.locator('.session-list [role="option"]').filter({ hasText: /[0-9a-f]{8}-[0-9a-f-]{27}/ });
  const alert = page.locator('.new-session [role="alert"]');
  const deadline = Date.now() + 120_000;
  while (Date.now() < deadline) {
    if (await option.count()) {
      const sessionId = (await option.first().innerText()).match(/[0-9a-f]{8}-[0-9a-f-]{27}/)?.[0];
      if (sessionId) {
        await option.first().focus();
        await page.getByRole('heading', { name: `Events for ${sessionId}` }).waitFor();
        return sessionId;
      }
    }
    if (await alert.count()) {
      const text = (await alert.innerText()).trim();
      if (text) throw new Error(text);
    }
    await page.waitForTimeout(200);
  }
  throw new Error('browser New session did not create a session');
}

async function pairBrowser(page, binary, dataDir, baseUrl) {
  const enrolled = spawnSync(binary, ['--data-dir', dataDir, 'client', 'enroll', '--label', `s1-lite-${randomUUID()}`, '--allow-session-create'], { encoding: 'utf8' });
  if (enrolled.status !== 0) throw new Error(enrolled.stderr || 'enroll failed');
  const code = enrolled.stdout.match(/enrollment_code=([0-9a-f]+)/)?.[1];
  if (!code) throw new Error('enrollment did not return a code');
  await page.goto(baseUrl);
  await page.getByRole('button', { name: 'Review fingerprint' }).click();
  await page.getByRole('textbox', { name: 'One-time enrollment code' }).fill(code);
  const fallback = page.getByRole('checkbox', { name: 'I understand the browser key can be read by same-origin script' });
  if (await fallback.isVisible().catch(() => false)) await fallback.check();
  await page.getByRole('button', { name: 'I verified this fingerprint — pair' }).click();
  await page.getByRole('heading', { name: 'Shardrail' }).waitFor();
  await page.getByText('Connected · browser WebSocket').waitFor();
}

async function sendPrompt(page, prompt) {
  await page.locator('#session-prompt').fill(prompt);
  await page.getByRole('button', { name: 'Send prompt' }).click();
}

async function driveTurn1(page, frames, sessionId) {
  const finishedBefore = countEvents(frames, sessionId, 'turn_finished');
  const decisions = [];
  const pending = page.locator('article[aria-label^="Approval "]').filter({ has: page.getByRole('button', { name: 'Deny' }) });
  const first = pending.first();
  await first.waitFor({ timeout: TURN_WAIT_MS });
  const deniedLabel = await first.getAttribute('aria-label');
  await first.getByRole('button', { name: 'Deny' }).click();
  decisions.push({ decision: 'deny', approval: deniedLabel });
  await page.locator(`article[aria-label="${deniedLabel}"]`).getByRole('button', { name: 'Deny' }).waitFor({ state: 'detached', timeout: 60_000 });
  return allowUntilFinished(page, frames, sessionId, finishedBefore, decisions, new Set([deniedLabel]));
}

function maxAllows() {
  return 8;
}

async function driveAllowTurn(page, frames, sessionId) {
  const finishedBefore = countEvents(frames, sessionId, 'turn_finished');
  await sendPrompt(page, 'Go ahead now: add `sub(a, b)` to calc.py and a unittest for it, then run `python3 -m unittest`.');
  return allowUntilFinished(page, frames, sessionId, finishedBefore, [], new Set());
}

// Allows each new approval card once (up to MAX_ALLOWS, then denies) until the turn finishes.
async function allowUntilFinished(page, frames, sessionId, finishedBefore, decisions, seen) {
  const cards = page.locator('article[aria-label^="Approval "]').filter({ has: page.getByRole('button', { name: 'Allow once' }) });
  const deadline = Date.now() + TURN_WAIT_MS;
  while (Date.now() < deadline && countEvents(frames, sessionId, 'turn_finished') <= finishedBefore) {
    const total = await cards.count();
    for (let index = 0; index < total; index++) {
      const card = cards.nth(index);
      const label = await card.getAttribute('aria-label').catch(() => null);
      if (!label || seen.has(label)) continue;
      if (!(await card.getByRole('button', { name: 'Allow once' }).isEnabled().catch(() => false))) continue;
      seen.add(label);
      if (decisions.filter((item) => item.decision === 'allow_once').length >= maxAllows()) {
        await card.getByRole('button', { name: 'Deny' }).click();
        decisions.push({ decision: 'deny', approval: label, reason: 'allow cap reached' });
        continue;
      }
      await card.getByRole('button', { name: 'Allow once' }).click();
      const confirm = page.getByRole('button', { name: 'Confirm allow once' });
      if (await confirm.isVisible().catch(() => false)) await confirm.click();
      decisions.push({ decision: 'allow_once', approval: label });
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 200));
  }
  if (countEvents(frames, sessionId, 'turn_finished') <= finishedBefore) throw new Error('turn did not finish');
  const finished = frames.filter((frame) => turnFinished(frame, sessionId)).at(-1);
  return {
    decisions,
    allowed: decisions.some((item) => item.decision === 'allow_once'),
    outcome: finished.event.payload?.provider_status ?? finished.event.payload?.session_snapshot?.last_turn_outcome ?? null,
  };
}

function turnFinished(frame, sessionId) {
  return frame.kind === 'durable_event' && frame.event.session_id === sessionId && frame.event.kind === 'turn_finished';
}

function isInterrupted(frame) {
  const payload = frame.event?.payload ?? {};
  return payload.provider_status === 'interrupted' || payload.session_snapshot?.last_turn_outcome === 'Interrupted';
}

function waitForEvent(frames, predicate, timeoutMs, label) {
  return new Promise((resolveReady, reject) => {
    const timer = setTimeout(() => {
      clearInterval(interval);
      reject(new Error(`timed out waiting for ${label}`));
    }, timeoutMs);
    const interval = setInterval(() => {
      try {
        const found = frames.find((frame) => predicate(frame));
        if (!found) return;
        clearTimeout(timer);
        clearInterval(interval);
        resolveReady(found);
      } catch (error) {
        clearTimeout(timer);
        clearInterval(interval);
        reject(error);
      }
    }, 100);
  });
}

function countEvents(frames, sessionId, kind) {
  return frames.filter((frame) => frame.kind === 'durable_event' && frame.event.session_id === sessionId && frame.event.kind === kind).length;
}

function waitUntil(predicate, timeoutMs, label) {
  return new Promise((resolveReady, reject) => {
    const timer = setTimeout(() => {
      clearInterval(interval);
      reject(new Error(`timed out waiting for ${label}`));
    }, timeoutMs);
    const interval = setInterval(() => {
      try {
        if (!predicate()) return;
        clearTimeout(timer);
        clearInterval(interval);
        resolveReady();
      } catch (error) {
        clearTimeout(timer);
        clearInterval(interval);
        reject(error);
      }
    }, 100);
  });
}

function countKinds(events) {
  const counts = {};
  for (const event of events) counts[event.kind] = (counts[event.kind] ?? 0) + 1;
  return counts;
}

function uniqueStrings(events, field) {
  return [...new Set(events.map((event) => event.payload?.[field]).filter((value) => typeof value === 'string'))];
}

function summarizeEvent(event) {
  return { event_id: event.event_id, kind: event.kind, payload: event.payload };
}

async function writeResult(resultDir, result, daemonLog) {
  await mkdir(resultDir, { recursive: true });
  await writeFile(join(resultDir, 's1-lite-result.json'), `${JSON.stringify(result, null, 2)}\n`);
  await writeFile(join(resultDir, 's1-lite-result.md'), renderMarkdown(result));
  await writeFile(join(resultDir, 'daemon.log'), daemonLog.join(''));
}

function renderMarkdown(result) {
  return [
    '# S1-lite live result',
    '',
    `- Session: \`${result.session_id}\``,
    `- Thread ids: ${result.thread_ids.map((id) => `\`${id}\``).join(', ') || 'none'}`,
    `- Turn ids: ${result.turn_ids.map((id) => `\`${id}\``).join(', ') || 'none'}`,
    `- Interrupt: \`${result.interrupt.outcome}\``,
    `- Unknown server request: ${result.unknown_request === 'not observed' ? 'not observed' : `${result.unknown_request.length} refused`}`,
    `- Shardrail git status unchanged: ${result.shardrail_git_status_unchanged}`,
    `- Codex extensions verified: ${result.extensions?.verified?.length ?? 0}`,
    `- Codex home paths in the journal: ${result.codex_home_paths?.length ?? 0}`,
    '',
    '## Event counts',
    '',
    ...Object.entries(result.event_counts_by_kind).map(([kind, count]) => `- ${kind}: ${count}`),
    '',
    '## Decisions',
    '',
    ...result.decisions.map((decision) => `- ${decision.decision} (${decision.approval ?? 'approval'})`),
    '',
    '## Timings (ms from start)',
    '',
    ...Object.entries(result.timings).map(([name, value]) => `- ${name}: ${value}`),
    '',
    '## Test repo',
    '',
    '```',
    result.test_repo.status || '(clean)',
    result.test_repo.diff_stat || '(no diff stat)',
    '```',
    '',
  ].join('\n');
}
