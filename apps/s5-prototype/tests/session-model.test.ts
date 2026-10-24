import assert from 'node:assert/strict';
import test from 'node:test';
import { agentText, approvalsForSession, eventsForSession, ingestJournalEvents, sessionSummaries, type JournalEvent } from '../src/session-model';
import { applyDaemonEvent, eventCount, reset, sessionJournalEvents, subscribe } from '../src/store';

const event = (cursor: number, sessionId: string, kind: string, payload: unknown, occurredAtMs = cursor * 1000): JournalEvent => ({ cursor, sessionId, kind, payload, occurredAtMs });
const snapshot = (lifecycle: string, turn: string, process: string) => ({ lifecycle, turn, process, queue: 'Empty' });

test('ingests journal events in cursor order without duplicates', () => {
  const first = ingestJournalEvents([], [event(2, 'b', 'session_created', {}), event(1, 'a', 'session_created', {})]);
  assert.deepEqual(first.events.map((item) => item.cursor), [1, 2]);
  const replay = ingestJournalEvents(first.events, [event(2, 'b', 'session_created', { changed: true }), event(3, 'a', 'agent_message_chunk', {})]);
  assert.equal(replay.added, 1);
  assert.deepEqual(replay.events.map((item) => item.cursor), [1, 2, 3]);
  assert.equal((replay.events[1]?.payload as { changed?: boolean }).changed, undefined);
  assert.equal(replay.cursor, 3);
  assert.equal(ingestJournalEvents(replay.events, [event(1, 'a', 'session_created', {})]).added, 0);
});

test('groups sessions by id with agent kind, lifecycle, and latest activity', () => {
  const events = [
    event(1, 'session-a', 'session_created', { session_snapshot: snapshot('open', 'Idle', 'Exited') }, 10),
    event(2, 'session-b', 'session_created', { session_snapshot: snapshot('open', 'Idle', 'Exited') }, 20),
    event(3, 'session-a', 'process_started', { provider: 'fake-acp', session_snapshot: snapshot('open', 'Idle', 'Alive') }, 30),
    event(4, 'session-a', 'turn_started', { session_snapshot: snapshot('open', 'Running', 'Alive') }, 40),
    event(5, 'session-a', 'agent_message_chunk', { content: { type: 'text', text: 'hello' } }, 15),
    event(6, 'session-b', 'process_exited', { session_snapshot: snapshot('open', 'Idle', 'Exited') }, 50),
  ];
  const sessions = sessionSummaries(events);
  assert.deepEqual(sessions.map((item) => item.id), ['session-b', 'session-a']);
  assert.equal(sessions[1]?.agentKind, 'fake-acp');
  assert.equal(sessions[1]?.lifecycle, 'open');
  assert.equal(sessions[1]?.status, 'open · running · alive');
  assert.equal(sessions[1]?.lastActivityMs, 40);
  assert.equal(sessions[0]?.agentKind, null);
  assert.equal(sessions[0]?.status, 'open · idle · exited');
  assert.deepEqual(eventsForSession(events, 'session-a').map((item) => item.cursor), [1, 3, 4, 5]);
  assert.equal(agentText(events[4]!), 'hello');
  assert.equal(agentText(events[0]!), null);
});

test('approval cards expose once options only and follow the daemon state', () => {
  const requested = event(4, 'session-a', 'approval_requested', {
    approval_id: 'approval-1',
    approval_kind: 'execute',
    tool_call_id: 'fake-tool-call',
    options: [
      { option_id: 'allow_once', name: 'Allow once', decision: 'allow', class: 'once' },
      { option_id: 'allow_always', name: 'Allow always', decision: 'allow', class: 'widening' },
      { option_id: 'allow_boundary', name: 'Allow permission change', decision: 'allow', class: 'boundary_breaking' },
      { option_id: 'reject_once', name: 'Reject once', decision: 'deny', class: 'once' },
    ],
    agent_description: { title: 'Run fixture command', description: 'untrusted' },
  });
  const pending = approvalsForSession([requested], 'session-a');
  assert.equal(pending.length, 1);
  assert.equal(pending[0]?.state, 'pending');
  assert.equal(pending[0]?.allowOnceOptionId, 'allow_once');
  assert.equal(pending[0]?.denyOnceOptionId, 'reject_once');
  assert.equal(pending[0]?.toolKind, 'execute');
  const responding = approvalsForSession([
    requested,
    event(5, 'session-a', 'approval_responding', { approval_id: 'approval-1', decision: 'deny', option_id: 'reject_once' }),
    event(6, 'session-a', 'approval_responded_unacked', { approval_id: 'approval-1' }),
  ], 'session-a');
  assert.equal(responding[0]?.state, 'responded_unacked');
  assert.equal(responding[0]?.decision, 'deny');
  const terminal = approvalsForSession([
    ...[requested],
    event(7, 'session-a', 'denied', { approval_id: 'approval-1', reason: null }),
    event(8, 'session-a', 'approval_requested', { approval_id: 'approval-1', options: [] }),
  ], 'session-a');
  assert.equal(terminal[0]?.state, 'denied');
  assert.equal(approvalsForSession([event(9, 'session-b', 'allowed', { approval_id: 'approval-2', decision: 'allow' })], 'session-a').length, 0);
});

test('coalesces session events once per frame and ignores a replayed cursor', () => {
  const callbacks: FrameRequestCallback[] = [];
  const previous = globalThis.requestAnimationFrame;
  globalThis.requestAnimationFrame = (callback) => { callbacks.push(callback); return callbacks.length; };
  try {
    reset(0, 1, false);
    let notifications = 0;
    const unsubscribe = subscribe(() => notifications++);
    notifications = 0;
    applyDaemonEvent(1, { session_id: 'session-a', kind: 'session_created', payload: {}, occurred_at_ms: 10 }, null);
    applyDaemonEvent(2, { session_id: 'session-a', kind: 'agent_message_chunk', payload: { content: { text: 'one' } }, occurred_at_ms: 20 }, null);
    applyDaemonEvent(2, { session_id: 'session-a', kind: 'agent_message_chunk', payload: { content: { text: 'duplicate' } }, occurred_at_ms: 21 }, null);
    assert.equal(callbacks.length, 1);
    assert.equal(notifications, 0);
    callbacks[0]!(0);
    assert.equal(notifications, 1);
    assert.equal(eventCount(), 2);
    assert.deepEqual(sessionJournalEvents().map((item) => item.cursor), [1, 2]);
    assert.equal(agentText(sessionJournalEvents()[1]!), 'one');
    applyDaemonEvent(2, { session_id: 'session-a', kind: 'agent_message_chunk', payload: { content: { text: 'again' } } }, null);
    assert.equal(callbacks.length, 1);
    unsubscribe();
    reset(0, 1, false);
  } finally {
    globalThis.requestAnimationFrame = previous;
  }
});
