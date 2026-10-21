import assert from 'node:assert/strict';
import test from 'node:test';
import { listenAndAnnounceReady, type TauriEventBridge } from '../src/tauri-bridge';

test('registers frontend listeners before emitting the payload required by Tauri', async () => {
  const order: string[] = [];
  const received: unknown[] = [];
  const bridge: TauriEventBridge = {
    async listen(event) {
      order.push(`listen:${event}`);
      return () => { order.push(`unlisten:${event}`); };
    },
    async emit(event, payload) {
      order.push(`emit:${event}`);
      received.push(payload);
    },
  };
  const remove = await listenAndAnnounceReady(bridge, () => {}, () => {});
  assert.deepEqual(order, ['listen:daemon-frames', 'listen:daemon-status', 'emit:s5-ready']);
  assert.deepEqual(received, [{}]);
  remove();
  assert.deepEqual(order.slice(-2), ['unlisten:daemon-frames', 'unlisten:daemon-status']);
});

test('removes frontend listeners when readiness emit fails', async () => {
  const removed: string[] = [];
  const bridge: TauriEventBridge = {
    async listen(event) { return () => { removed.push(event); }; },
    async emit() { throw new Error('bridge unavailable'); },
  };
  await assert.rejects(listenAndAnnounceReady(bridge, () => {}, () => {}), /bridge unavailable/);
  assert.deepEqual(removed, ['daemon-frames', 'daemon-status']);
});

test('removes a listener that registered before another listener failed', async () => {
  const removed: string[] = [];
  const bridge: TauriEventBridge = {
    async listen(event) {
      if (event === 'daemon-status') throw new Error('registration failed');
      return () => { removed.push(event); };
    },
    async emit() { assert.fail('readiness must not be emitted after setup failure'); },
  };
  await assert.rejects(listenAndAnnounceReady(bridge, () => {}, () => {}), /registration failed/);
  assert.deepEqual(removed, ['daemon-frames']);
});
