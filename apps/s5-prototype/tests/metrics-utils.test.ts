import assert from 'node:assert/strict';
import test from 'node:test';
import { matchedInFirstFrame, measureAfterPaint, measureReadyCommitAfterFrame, nominalFrameIntervalMs, percentile, supportsLongTaskEntries, takeCommittedInput } from '../src/metrics-utils';

test('reports long task support only when the entry type is advertised', () => {
  assert.equal(supportsLongTaskEntries(['mark', 'measure']), false);
  assert.equal(supportsLongTaskEntries(['longtask']), true);
  assert.equal(supportsLongTaskEntries(undefined), false);
});

test('does not turn an empty input sample into a zero-latency result', () => {
  assert.equal(percentile([], 0.95), null);
  assert.equal(percentile([4, 1, 3, 2], 0.95), 4);
});

test('ends an input sample in a task after the next animation frame', () => {
  const order: string[] = [];
  let frame: (() => void) | undefined;
  let task: (() => void) | undefined;
  let now = 10;
  let elapsed = 0;
  measureAfterPaint(2, () => now, (callback) => { frame = callback; return 1; }, (callback) => { order.push('task-scheduled'); task = callback; }, (value) => { elapsed = value; });
  assert.deepEqual(order, []);
  frame?.();
  assert.deepEqual(order, ['task-scheduled']);
  assert.equal(elapsed, 0);
  now = 19;
  task?.();
  assert.equal(elapsed, 17);
});

test('starts first transcript render opportunity timing only after the ready commit', () => {
  const frames: (() => void)[] = [];
  let measured = 0;
  assert.equal(measureReadyCommitAfterFrame(false, 2, () => 20, (callback) => { frames.push(callback); return 1; }, () => {}, (elapsed) => { measured = elapsed; }), false);
  assert.equal(frames.length, 0);
  assert.equal(measureReadyCommitAfterFrame(true, 2, () => 20, (callback) => { frames.push(callback); return 1; }, (callback) => callback(), (elapsed) => { measured = elapsed; }), true);
  assert.equal(frames.length, 1);
  frames[0]!();
  assert.equal(measured, 18);
});

test('first-frame match requires both a DOM match and an rAF timestamp within one nominal 60 Hz interval', () => {
  assert.deepEqual(matchedInFirstFrame(100, 115, 116, true), { delay: 16, frameTimestampDelta: 15, matched: true });
  assert.deepEqual(matchedInFirstFrame(100, 117, 118, true), { delay: 18, frameTimestampDelta: 17, matched: false });
  assert.deepEqual(matchedInFirstFrame(100, 95, 110, true), { delay: 10, frameTimestampDelta: -5, matched: true });
  assert.deepEqual(matchedInFirstFrame(100, 95, 120, true), { delay: 20, frameTimestampDelta: -5, matched: false });
  assert.deepEqual(matchedInFirstFrame(100, 110, 111, false), { delay: 11, frameTimestampDelta: 10, matched: false });
  assert.equal(nominalFrameIntervalMs, 1000 / 60);
});

test('batched input commits consume the matching latest draft and discard skipped commits', () => {
  const pending = [{ startedAt: 100, draft: 'a' }, { startedAt: 105, draft: 'ab' }];
  assert.deepEqual(takeCommittedInput(pending, 'ab'), { startedAt: 105, draft: 'ab' });
  assert.deepEqual(pending, []);
});
