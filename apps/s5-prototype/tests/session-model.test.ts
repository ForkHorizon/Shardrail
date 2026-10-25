import assert from 'node:assert/strict';
import test from 'node:test';
import { NIL_SESSION_ID, activityTimestamp, agentText, approvalsForSession, checklistSummary, clearPromptOnAccept, controlFailureTarget, eventsForSession, ingestJournalEvents, newSessionButton, plainEventText, promptClearApplies, reduceApprovalFlight, revealHiddenCharacters, sessionSummaries, transcriptItems, VERIFICATION_CHECKS, type JournalEvent } from '../src/session-model';
import { applyDaemonEvent, eventCount, observeJournalEpoch, reset, sessionJournalEvents, subscribe } from '../src/store';

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
  assert.equal(pending[0]?.actionDisclosed, false);
  assert.equal(pending[0]?.command, null);
  assert.equal(pending[0]?.cwd, null);
  const cwdOnly = approvalsForSession([event(5, 'session-a', 'approval_requested', {
    approval_id: 'approval-cwd-only', cwd: '/tmp/work', options: [],
  })], 'session-a');
  assert.equal(cwdOnly[0]?.actionDisclosed, false);
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
  const disclosed = approvalsForSession([event(10, 'session-a', 'approval_requested', {
    approval_id: 'approval-3',
    approval_kind: 'execute',
    command: 'curl\u202E',
    argv: ['curl', 'https://example.test'],
    cwd: '/tmp/work',
    raw_input: { cmd: 'curl' },
    sandbox: 'workspace-write',
    options: [{ option_id: 'allow_once', decision: 'allow', class: 'once' }],
  })], 'session-a').find((card) => card.approvalId === 'approval-3');
  assert.equal(disclosed?.actionDisclosed, true);
  assert.equal(disclosed?.command, 'curl\u202E');
  assert.deepEqual(disclosed?.argv, ['curl', 'https://example.test']);
  assert.equal(disclosed?.cwd, '/tmp/work');
  assert.equal(disclosed?.sandbox, 'workspace-write');
  assert.equal(revealHiddenCharacters(disclosed?.command ?? ''), 'curl\\u202e');
});

test('coalesces chunk text and keeps other events as plain text', () => {
  const items = transcriptItems([
    event(1, 'session-a', 'agent_message_chunk', { content: { text: '```python\n' } }),
    event(2, 'session-a', 'agent_message_chunk', { content: { text: 'print(1)\n```' } }),
    event(3, 'session-a', 'tool_call_started', { name: 'echo', argv: ['echo', 'hi'] }),
    event(4, 'session-a', 'agent_message_chunk', { content: { text: 'after' } }),
  ]);
  assert.equal(items.length, 3);
  assert.equal(items[0]?.markdown, '```python\nprint(1)\n```');
  assert.equal(items[0]?.cursorLabel, '#1–#2');
  assert.equal(items[1]?.plain, plainEventText(event(3, 'session-a', 'tool_call_started', { name: 'echo', argv: ['echo', 'hi'] })));
  assert.equal(items[1]?.markdown, null);
  assert.equal(items[2]?.markdown, 'after');
});

test('activity timestamps reject non-finite values', () => {
  assert.equal(activityTimestamp(Number.NaN), null);
  assert.equal(activityTimestamp(Number.POSITIVE_INFINITY), null);
  assert.equal(activityTimestamp(0), null);
  assert.equal(activityTimestamp(-5), null);
  assert.equal(activityTimestamp(1_700_000_000_000), '2023-11-14T22:13:20.000Z');
});

test('the verification checklist stays not verified and does not claim a single status', () => {
  const summary = checklistSummary();
  assert.equal(summary.unverified, VERIFICATION_CHECKS.length);
  assert.equal(summary.failed, 0);
  assert.equal(summary.checks.some((item) => item.status === 'verified' || /protected/i.test(item.status)), false);
  assert.equal(summary.checks[0]?.lastChecked, null);
});

test('a confirmed approval leaves the in-flight set and a later disconnect does not alert', () => {
  const sent = reduceApprovalFlight({ inFlight: new Set(), error: null }, { type: 'sent', approvalId: 'approval-1' });
  const resolved = reduceApprovalFlight(sent, { type: 'resolved', approvalId: 'approval-1' });
  assert.equal(resolved.inFlight.size, 0);
  assert.equal(resolved.error, null);
  const disconnected = reduceApprovalFlight(resolved, { type: 'disconnect' });
  assert.equal(disconnected, resolved);
  const journaled = reduceApprovalFlight(sent, { type: 'journal', kind: 'allowed', approvalId: 'approval-1' });
  assert.equal(journaled.inFlight.size, 0);
  assert.equal(reduceApprovalFlight(journaled, { type: 'disconnect' }).error, null);
  const lost = reduceApprovalFlight(sent, { type: 'disconnect' });
  assert.equal(lost.inFlight.size, 0);
  assert.match(lost.error ?? '', /not confirmed/);
  assert.match(reduceApprovalFlight(sent, { type: 'blocked' }).error ?? '', /not allowed/);
  assert.equal(reduceApprovalFlight(sent, { type: 'journal', kind: 'agent_message_chunk', approvalId: 'approval-1' }), sent);
});

test('a changed journal epoch replaces stale cursors instead of dropping the new journal', () => {
  const callbacks: FrameRequestCallback[] = [];
  const previous = globalThis.requestAnimationFrame;
  globalThis.requestAnimationFrame = (callback) => { callbacks.push(callback); return callbacks.length; };
  try {
    reset(0, 1, false);
    assert.equal(observeJournalEpoch('epoch-old'), false);
    applyDaemonEvent(1, { session_id: 'old-session', kind: 'session_created', payload: {}, occurred_at_ms: 10 }, null);
    callbacks[0]!(0);
    assert.deepEqual(sessionJournalEvents().map((item) => item.sessionId), ['old-session']);
    assert.equal(observeJournalEpoch('epoch-new'), true);
    assert.equal(sessionJournalEvents().length, 0);
    applyDaemonEvent(1, { session_id: 'new-session', kind: 'session_created', payload: {}, occurred_at_ms: 20 }, null);
    callbacks.at(-1)!(0);
    assert.deepEqual(sessionJournalEvents().map((item) => item.sessionId), ['new-session']);
    assert.equal(observeJournalEpoch('epoch-new'), false);
  } finally {
    globalThis.requestAnimationFrame = previous;
    reset(0, 1, false);
  }
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

test('session summaries skip the nil create audit and the new-session button follows the allowlist', () => {
  const events = [
    event(1, NIL_SESSION_ID, 'session_create_audit', { outcome: 'denied', reason: 'scope' }),
    event(2, 'session-a', 'session_created', { session_snapshot: snapshot('open', 'Idle', 'Exited') }, 10),
    event(3, 'session-a', 'session_create_audit', { outcome: 'allowed' }, 11),
  ];
  assert.deepEqual(sessionSummaries(events).map((item) => item.id), ['session-a']);
  assert.equal(newSessionButton(null), 'hidden');
  assert.equal(newSessionButton([]), 'disabled');
  assert.equal(newSessionButton(['/tmp/work']), 'ready');
});

test('command rejection stays on the prompt and an error follows the approval flight', () => {
  assert.equal(controlFailureTarget('command_rejected', 1), 'prompt');
  assert.equal(controlFailureTarget('command_rejected', 0), 'prompt');
  assert.equal(controlFailureTarget('error', 1), 'approval');
  assert.equal(controlFailureTarget('error', 0), 'prompt');
  assert.equal(controlFailureTarget('approval_resolved', 1), null);
});

test('a prompt clears only when its own turn command is accepted on that session', () => {
  assert.equal(clearPromptOnAccept(null, 'command-1'), false);
  assert.equal(clearPromptOnAccept('command-1', 'command-2'), false);
  assert.equal(clearPromptOnAccept('command-1', 'command-1'), true);
  assert.equal(promptClearApplies(null, 'session-a'), false);
  assert.equal(promptClearApplies('session-a', 'session-b'), false);
  assert.equal(promptClearApplies('session-a', 'session-a'), true);
});
