import assert from 'node:assert/strict';
import test from 'node:test';
import { applyPendingUpdates, eventCount, getRows, markRowsCommitted, mockCreationToMountedRowFollowupLatency, pushToken, reset, subscribe, tokenCount } from '../src/store';

test('coalesces token deltas into one active message per stream and one store update per frame', () => {
  const callbacks: FrameRequestCallback[] = [];
  const previous = globalThis.requestAnimationFrame;
  globalThis.requestAnimationFrame = (callback) => { callbacks.push(callback); return callbacks.length; };
  try {
    reset(0, 4);
    let notifications = 0;
    const unsubscribe = subscribe(() => notifications++);
    notifications = 0;
    for (let agent = 0; agent < 4; agent++) {
      pushToken(agent, 0);
      pushToken(agent, 1);
    }
    assert.equal(callbacks.length, 1);
    assert.equal(eventCount(), 4);
    assert.equal(tokenCount(), 8);
    assert.equal(notifications, 0);
    callbacks[0]!(performance.now());
    assert.equal(notifications, 1);
    assert.equal(getRows().length, 4);
    assert.equal(getRows()[0]!.text.includes('token 0 token 1'), true);
    const paintFrames: FrameRequestCallback[] = [], paintTasks: (() => void)[] = [];
    let now = performance.now();
    markRowsCommitted([], (callback) => { paintFrames.push(callback); return paintFrames.length; }, (callback) => { paintTasks.push(callback); }, () => now);
    assert.equal(paintFrames.length, 0, 'unmounted rows must not be sampled');
    markRowsCommitted([getRows()[0]!.id], (callback) => { paintFrames.push(callback); return paintFrames.length; }, (callback) => { paintTasks.push(callback); }, () => now);
    markRowsCommitted([getRows()[0]!.id], (callback) => { paintFrames.push(callback); return paintFrames.length; }, (callback) => { paintTasks.push(callback); }, () => now);
    assert.equal(paintFrames.length, 1, 'a mounted row is sampled only once');
    paintFrames.forEach((callback) => callback(now));
    assert.equal(mockCreationToMountedRowFollowupLatency.length, 0);
    now += 16;
    paintTasks.forEach((callback) => callback());
    assert.equal(mockCreationToMountedRowFollowupLatency.length, 1);
    unsubscribe();
  } finally {
    globalThis.requestAnimationFrame = previous;
  }
});

test('applies a multi-stream frame batch with one immutable transcript copy and stable ID indexes', () => {
  const initial = Array.from({ length: 50_000 }, (_, id) => ({ id, agent: 0, kind: 'message' as const, text: 'old', command: '', cwd: '', at: 0 }));
  const indexes = new Map(initial.map((row, index) => [row.id, index]));
  const updates = Array.from({ length: 8 }, (_, agent) => ({ ...initial[10 + agent]!, agent, text: `updated ${agent}` }));
  const updated = applyPendingUpdates(initial, indexes, updates);
  assert.notEqual(updated, initial);
  assert.equal(initial[10]!.text, 'old');
  assert.equal(updated.length, initial.length);
  assert.deepEqual(updated.slice(10, 18).map((row) => row.text), updates.map((row) => row.text));
  assert.equal(indexes.get(17), 17);
});

test('eventCount does not grow when existing streamed rows are queued for update', () => {
  const callbacks: FrameRequestCallback[] = [];
  const previous = globalThis.requestAnimationFrame;
  globalThis.requestAnimationFrame = (callback) => { callbacks.push(callback); return callbacks.length; };
  try {
    reset(5_000, 1);
    pushToken(0, 0);
    assert.equal(eventCount(), 5_001);
    callbacks[0]!(performance.now());
    assert.equal(eventCount(), 5_001);
    pushToken(0, 1);
    assert.equal(eventCount(), 5_001);
  } finally {
    globalThis.requestAnimationFrame = previous;
  }
});
