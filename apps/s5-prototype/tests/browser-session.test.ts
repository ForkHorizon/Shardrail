import assert from 'node:assert/strict';
import test from 'node:test';
import type { DaemonFrame } from '@shardrail/contracts';
import { connectionStatusLabel, createBrowserSession, isKeyChangedError, parseStoredCursor, reconnectDelayMs, type ConnectionPhase, type SessionSocket, type StoredCursor } from '../src/browser-session';
import type { JournalEvent } from '../src/session-model';

test('reconnect delay backs off and the status labels stay explicit', () => {
  assert.deepEqual([1, 2, 3, 4, 5, 6, 7, 8].map(reconnectDelayMs), [200, 400, 800, 1600, 3200, 6400, 8000, 8000]);
  assert.equal(reconnectDelayMs(0), 200);
  assert.equal(connectionStatusLabel('connected'), 'Connected · browser WebSocket');
  assert.equal(connectionStatusLabel('reconnecting'), 'Reconnecting · browser WebSocket');
  assert.equal(connectionStatusLabel('offline'), 'Offline · browser WebSocket');
  assert.equal(connectionStatusLabel('key-changed'), 'Key changed · pairing is blocked');
  assert.equal(isKeyChangedError(new Error('daemon host key changed; pairing is blocked')), true);
  assert.equal(isKeyChangedError(new Error('daemon WebSocket closed')), false);
  assert.deepEqual(parseStoredCursor(JSON.stringify({ cursor: 4, journalEpoch: 'epoch-1' })), { cursor: 4, journalEpoch: 'epoch-1' });
  assert.equal(parseStoredCursor('{"cursor":-1,"journalEpoch":"epoch-1"}'), undefined);
  assert.equal(parseStoredCursor('not-json'), undefined);
});

test('resumes from the last cursor without replaying events or sending an approval while down', async () => {
  const statuses: ConnectionPhase[] = [];
  const delivered: JournalEvent[] = [];
  const sent: { approval_id: string; option_id: string }[] = [];
  const cursors: Array<number | undefined> = [];
  const timers: { delay: number; run: () => void }[] = [];
  let online = true;
  let saved: StoredCursor | undefined;
  const scripts: Array<(socket: FakeSocket) => void> = [
    (socket) => {
      socket.emit(durable(1, 'session-a', 'session_created'));
      socket.emit(durable(2, 'session-a', 'agent_message_chunk'));
      socket.emit(durable(2, 'session-a', 'agent_message_chunk'));
      socket.emit(durable(4, 'session-a', 'agent_message_chunk'));
      socket.emit(durable(3, 'session-a', 'turn_started'));
      socket.fail(new Error('daemon WebSocket closed'));
    },
    (socket) => {
      socket.emit(durable(2, 'session-a', 'agent_message_chunk'));
      socket.emit(durable(4, 'session-a', 'agent_message_chunk'));
      socket.emit(durable(5, 'session-a', 'process_exited'));
    },
  ];
  const session = createBrowserSession({
    connect: fakeConnect(scripts, cursors, sent),
    url: 'ws://127.0.0.1:9/ws',
    identity: { hostPublicKey: 'host', clientId: 'client', publicKey: 'key', storage: 'webcrypto' },
    loadCursor: () => saved,
    saveCursor: (cursor, journalEpoch) => { saved = { cursor, journalEpoch }; },
    schedule: (delay, run) => { timers.push({ delay, run }); return () => undefined; },
    online: () => online,
    onStatus: (phase) => statuses.push(phase),
    onDurableEvent: (event) => delivered.push(event),
    onControl: () => undefined,
  });
  await flush();
  assert.deepEqual(delivered.map((event) => event.cursor), [1, 2, 4, 3]);
  assert.equal(saved?.cursor, 4);
  assert.deepEqual(statuses, ['connecting', 'connected', 'reconnecting']);
  assert.equal(session.resolveApproval('approval-1', 'allow_once'), 'blocked');
  assert.deepEqual(sent, []);
  assert.equal(timers.length, 1);
  assert.equal(timers[0]?.delay, 200);
  timers[0]?.run();
  await flush();
  assert.deepEqual(cursors, [undefined, 4]);
  assert.deepEqual(delivered.map((event) => event.cursor), [1, 2, 4, 3, 5]);
  assert.equal(statuses.at(-1), 'connected');
  assert.equal(session.resolveApproval('approval-1', 'allow_once'), 'sent');
  assert.deepEqual(sent, [{ approval_id: 'approval-1', option_id: 'allow_once' }]);
  online = false;
  session.setOnline(false);
  assert.equal(statuses.at(-1), 'offline');
  assert.equal(session.resolveApproval('approval-1', 'allow_once'), 'blocked');
  assert.equal(sent.length, 1);
  session.stop();
});

test('a changed host key stops reconnect and an offline browser waits', async () => {
  const statuses: ConnectionPhase[] = [];
  const timers: { delay: number }[] = [];
  let online = true;
  const session = createBrowserSession({
    connect: async () => { throw new Error('daemon host key changed; pairing is blocked'); },
    url: 'ws://127.0.0.1:9/ws',
    identity: { hostPublicKey: 'host', clientId: 'client', publicKey: 'key', storage: 'webcrypto' },
    loadCursor: () => undefined,
    saveCursor: () => undefined,
    schedule: (delay) => { timers.push({ delay }); return () => undefined; },
    online: () => online,
    onStatus: (phase) => statuses.push(phase),
    onDurableEvent: () => undefined,
    onControl: () => undefined,
  });
  await flush();
  assert.deepEqual(statuses, ['connecting', 'key-changed']);
  assert.equal(timers.length, 0);
  online = false;
  session.setOnline(false);
  assert.equal(statuses.at(-1), 'key-changed');
  session.stop();
});

test('manual reconnect resets backoff after failures', async () => {
  const timers: { delay: number; run: () => void }[] = [];
  let attempts = 0;
  const session = createBrowserSession({
    connect: async () => {
      attempts += 1;
      if (attempts < 4) throw new Error('daemon WebSocket closed');
      return openSocket(() => undefined);
    },
    url: 'ws://127.0.0.1:9/ws',
    identity: { hostPublicKey: 'host', clientId: 'client', publicKey: 'key', storage: 'webcrypto' },
    loadCursor: () => ({ cursor: 9, journalEpoch: 'epoch-1' }),
    saveCursor: () => undefined,
    schedule: (delay, run) => { const timer = { delay, run }; timers.push(timer); return () => { timer.run = () => undefined; }; },
    online: () => true,
    onStatus: () => undefined,
    onDurableEvent: () => undefined,
    onControl: () => undefined,
  });
  await flush();
  assert.equal(timers[0]?.delay, 200);
  timers[0]?.run();
  await flush();
  assert.equal(timers[1]?.delay, 400);
  session.reconnect();
  await flush();
  assert.equal(attempts, 3);
  assert.equal(timers.at(-1)?.delay, 200);
  timers.at(-1)?.run();
  await flush();
  assert.equal(attempts, 4);
  session.stop();
});

function durable(cursor: number, sessionId: string, kind: string): DaemonFrame {
  return { kind: 'durable_event', cursor, event: { event_id: cursor, journal_epoch: 'epoch-1', host_id: 'host', session_id: sessionId, schema_version: 1, kind, occurred_at_ms: cursor, provider_ref: null, causation: null, redaction_flags: [], payload: {} }, receipt_monotonic_ns: null, receipt_wall_clock_ms: null };
}

interface FakeSocket extends SessionSocket {
  emit(frame: DaemonFrame): void;
  fail(error: Error): void;
}

function fakeConnect(scripts: Array<(socket: FakeSocket) => void>, cursors: Array<number | undefined>, sent: { approval_id: string; option_id: string }[]) {
  let call = 0;
  return async (options: { cursor?: number }) => {
    cursors.push(options.cursor);
    const script = scripts[Math.min(call, scripts.length - 1)]!;
    call += 1;
    const socket = openSocket((request) => sent.push(request));
    setTimeout(() => script(socket), 0);
    return socket;
  };
}

function openSocket(onResolve: (request: { approval_id: string; option_id: string }) => void): FakeSocket {
  const frames = new Set<(frame: DaemonFrame) => void>();
  const errors = new Set<(error: Error) => void>();
  const socket: FakeSocket = {
    host: { journal_epoch: 'epoch-1' },
    onFrame(listener) { frames.add(listener); return () => frames.delete(listener); },
    onError(listener) { errors.add(listener); return () => errors.delete(listener); },
    resolveApproval(request) { onResolve(request); },
    close() { errors.forEach((listener) => listener(new Error('daemon WebSocket closed'))); },
    emit(frame) { frames.forEach((listener) => listener(frame)); },
    fail(error) { errors.forEach((listener) => listener(error)); },
  };
  return socket;
}

function flush() {
  return new Promise((resolve) => setTimeout(resolve, 0));
}
