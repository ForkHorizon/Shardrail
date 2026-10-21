import assert from 'node:assert/strict';
import test from 'node:test';
import { benchShouldFail, daemonStreamFinished, draftPreview, generatedTokenLoadReached, minimumGeneratedTokens, missingRequiredSamples, playwrightOutputPath, shouldWaitForDaemonStream, withScenarioConfiguration } from '../src/bench-utils';

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
