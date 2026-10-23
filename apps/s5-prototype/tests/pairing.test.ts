import assert from 'node:assert/strict';
import { test } from 'node:test';
import { beginConfirmation, confirmPairing, stateFromInfo, unpairClient, type PairingApi, type PairingInfo } from '../src/pairing';

const info: PairingInfo = { daemonPublicKey: 'a'.repeat(64), fingerprint: 'SHA256:one', clientId: null, pinnedPublicKey: null };
function api(overrides: Partial<PairingApi> = {}): PairingApi {
  return { inspect: async () => info, pair: async () => 'client-id', unpair: async () => {}, ...overrides };
}

test('pairing requires explicit confirmation and pins the inspected daemon key', async () => {
  let registered = '';
  const state = await confirmPairing(beginConfirmation(info, 100), api({ pair: async (key) => { registered = key; return 'client-id'; } }), 101);
  assert.equal(registered, info.daemonPublicKey);
  assert.deepEqual(state, { kind: 'paired', info: { ...info, clientId: 'client-id', pinnedPublicKey: info.daemonPublicKey } });
});

test('pairing forwards fallback-key acknowledgement only after explicit consent', async () => {
  const fallback = { ...info, keyStorage: 'noble-fallback' as const };
  let acknowledged = false;
  const state = await confirmPairing(beginConfirmation(fallback, 100), api({ inspect: async () => fallback, pair: async (_key, _code, consent) => { acknowledged = consent === true; return 'client-id'; } }), 101, 'enrollment-code', true);
  assert.equal(acknowledged, true);
  assert.equal(state.kind, 'paired');
});

test('expired confirmation never registers a client', async () => {
  let called = false;
  const state = await confirmPairing(beginConfirmation(info, 100), api({ pair: async () => { called = true; return 'client-id'; } }), 60_100);
  assert.equal(state.kind, 'expired');
  assert.equal(called, false);
});

test('confirmation that expires while daemon identity is inspected never registers', async () => {
  let now = 100;
  let called = false;
  const state = await confirmPairing(
    beginConfirmation(info, 100),
    api({
      inspect: async () => { now = 60_100; return info; },
      pair: async () => { called = true; return 'client-id'; },
    }),
    () => now,
  );
  assert.equal(state.kind, 'expired');
  assert.equal(called, false);
});

test('daemon key changes during confirmation produce mismatch without registration', async () => {
  let called = false;
  const changed = { ...info, daemonPublicKey: 'b'.repeat(64), fingerprint: 'SHA256:two' };
  const state = await confirmPairing(beginConfirmation(info), api({ inspect: async () => changed, pair: async () => { called = true; return 'client-id'; } }));
  assert.deepEqual(state, { kind: 'mismatch', info: changed });
  assert.equal(called, false);
});

test('changed pinned daemon key blocks automatic reconnect', () => {
  assert.equal(stateFromInfo({ ...info, clientId: 'client-id', pinnedPublicKey: 'b'.repeat(64) }).kind, 'mismatch');
});

test('failed registration and unpair remain explicit errors', async () => {
  assert.equal((await confirmPairing(beginConfirmation(info), api({ pair: async () => { throw new Error('secret'); } }))).kind, 'error');
  assert.equal((await unpairClient(api({ unpair: async () => { throw new Error('secret'); } }))).kind, 'error');
});
