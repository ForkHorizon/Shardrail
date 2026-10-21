import assert from 'node:assert/strict';
import test from 'node:test';
import { createFrameSampler } from '../src/frame-sampler';

test('frame sampling is idle until started and stops scheduling after a run', () => {
  const callbacks: FrameRequestCallback[] = [];
  const cancelled: number[] = [];
  const intervals: number[] = [];
  let time = 10;
  const sampler = createFrameSampler((callback) => { callbacks.push(callback); return callbacks.length; }, (id) => cancelled.push(id), (ms) => intervals.push(ms), () => time);
  assert.equal(callbacks.length, 0);
  sampler.start();
  assert.equal(callbacks.length, 1);
  time += 16;
  callbacks[0]!(time);
  assert.deepEqual(intervals, [16]);
  assert.equal(callbacks.length, 2);
  sampler.stop();
  assert.deepEqual(cancelled, [2]);
  time += 16;
  callbacks[1]!(time);
  assert.equal(callbacks.length, 2);
  assert.deepEqual(intervals, [16]);
});
