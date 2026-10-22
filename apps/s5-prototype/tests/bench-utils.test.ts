import assert from 'node:assert/strict';
import test from 'node:test';
import { benchShouldFail, daemonStreamFinished, draftPreview, generatedTokenLoadReached, minimumB4Samples, minimumGeneratedTokens, missingRequiredSamples, playwrightOutputPath, shouldWaitForDaemonStream, waitForB4Samples, waitForDaemonStream, withScenarioConfiguration } from '../src/bench-utils';
import { createServer } from 'node:http';
import { applyDaemonEvent, daemonAgentChunkCount, pendingDaemonReceiptCount, reset } from '../src/store';

test('requires enough token deltas without requiring token events to grow the transcript', () => {
  const required = minimumGeneratedTokens(4, 6000);
  assert.equal(required, 900);
  assert.equal(generatedTokenLoadReached(0, 0, required), false);
  assert.equal(generatedTokenLoadReached(900, 0, required), true);
});

test('only B4 waits for daemon stream delivery', () => {
  assert.equal(shouldWaitForDaemonStream(new URLSearchParams('events=50000'), 50_000), false);
  assert.equal(shouldWaitForDaemonStream(new URLSearchParams('events=50000&b4=1'), 50_000), true);
  assert.equal(shouldWaitForDaemonStream(new URLSearchParams('b4=1'), 0), false);
});

test('accepts stream completion after a clean disconnect and validates chunks separately from frame samples', () => {
  assert.equal(daemonStreamFinished(true, 5_000, 5_000), true);
  assert.equal(daemonStreamFinished(false, 5_000, 5_000), false);
  assert.equal(daemonStreamFinished(true, 4_999, 5_000), false);
});

test('B4 waits for a useful sample set after chunk delivery finishes', async () => {
  assert.equal(minimumB4Samples(3), 3);
  assert.equal(minimumB4Samples(5_000), 20);
  let elapsed = 0;
  let samples = 0;
  await waitForB4Samples(3, () => samples, async () => { samples++; elapsed += 50; }, () => elapsed);
  assert.equal(samples, 3);
  await assert.rejects(waitForB4Samples(2, () => 0, async () => { elapsed += 50; }, () => elapsed), /B4 samples incomplete: 0\/2/);
});

test('late B4 stream chunks reach completion and are saved headlessly', async () => {
  const previousRaf = globalThis.requestAnimationFrame;
  globalThis.requestAnimationFrame = (callback) => { callback(performance.now()); return 1; };
  reset(0, 1);
  let polls = 0;
  let saved: { filename: string; result: { daemonAgentChunkCount: number } } | undefined;
  const server = createServer(async (request, response) => {
    let body = '';
    for await (const part of request) body += part;
    saved = JSON.parse(body) as typeof saved;
    response.writeHead(200).end();
  });
  try {
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    let elapsed = 0;
    await waitForDaemonStream(3, () => daemonAgentChunkCount, () => true, async () => {
      applyDaemonEvent(polls + 1, { session_id: 'fake', kind: 'agent_message_chunk', payload: { content: { text: 'fixture' } } }, polls);
      polls++;
      elapsed += 50;
    }, () => elapsed);
    const address = server.address();
    assert.ok(address && typeof address === 'object');
    const response = await fetch(`http://127.0.0.1:${address.port}/result`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ filename: 'cold-3-1-r1', result: { daemonAgentChunkCount } }),
    });
    assert.equal(response.status, 200);
    assert.deepEqual(saved, { filename: 'cold-3-1-r1', result: { daemonAgentChunkCount: 3 } });
  } finally {
    await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
    globalThis.requestAnimationFrame = previousRaf;
    reset(0, 1);
  }
});

test('B4 receipt retention stays bounded when virtual rows never mount', () => {
  const previousRaf = globalThis.requestAnimationFrame;
  globalThis.requestAnimationFrame = () => 1;
  try {
    reset(0, 1);
    for (let cursor = 1; cursor <= 1_000; cursor++) {
      applyDaemonEvent(cursor, { session_id: 'fake', kind: 'agent_message_chunk', payload: {} }, cursor);
    }
    assert.equal(pendingDaemonReceiptCount(), 512);
  } finally {
    globalThis.requestAnimationFrame = previousRaf;
    reset(0, 1);
  }
});

test('fails a benchmark when engines yield launch errors and no samples', () => {
  assert.equal(benchShouldFail([{ engine: 'webkit', launchError: 'missing executable' }], false), true);
  assert.equal(benchShouldFail([{ engine: 'webkit', scenario: 'B5' }], false), false);
  assert.equal(benchShouldFail([{ engine: 'webkit', scenario: 'B5' }], true), true);
  assert.deepEqual(missingRequiredSamples({ frameP95: null, mockCreationToMountedRowFollowupP95: null }), ['frameP95', 'mockCreationToMountedRowFollowupP95']);
  assert.deepEqual(missingRequiredSamples({ frameP95: 16, mockCreationToMountedRowFollowupP95: 4 }), []);
});

test('smoke results do not overwrite the full Playwright matrix and explicit output paths are honored', () => {
  const fixedDate = new Date('2026-10-20T12:34:56.789Z');
  assert.equal(playwrightOutputPath(true, undefined, fixedDate), 'results/playwright-smoke-2026-10-20T12-34-56-789Z.json');
  assert.equal(playwrightOutputPath(false, undefined, fixedDate), 'results/playwright.json');
  assert.equal(playwrightOutputPath(true, 'results/review-smoke.json', fixedDate), 'results/review-smoke.json');
});

test('draft preview parses slash commands with arguments', () => {
  assert.equal(draftPreview('/approve 123'), 'Slash command: approve');
  assert.equal(draftPreview('/s5-loaded-turn-input-trace'), 'Slash command: s5-loaded-turn-input-trace');
  assert.equal(draftPreview('ordinary draft'), 'Draft preview: ordinary draft');
});

test('scenario fixture size is not overwritten by the observed event store count', () => {
  assert.deepEqual(withScenarioConfiguration({ eventStoreCount: 5_008 }, 5_000, 8), { eventStoreCount: 5_008, events: 5_000, streams: 8 });
});
