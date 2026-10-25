export type JournalEvent = {
  cursor: number;
  sessionId: string;
  kind: string;
  occurredAtMs: number;
  payload: unknown;
};

export type SessionSummary = {
  id: string;
  label: string;
  agentKind: string | null;
  lifecycle: string;
  status: string;
  lastActivityMs: number;
  lastCursor: number;
};

export type ApprovalCard = {
  approvalId: string;
  sessionId: string;
  state: string;
  decision: string | null;
  toolKind: string;
  toolCallId: string;
  command: string | null;
  argv: string[] | null;
  rawInput: string | null;
  cwd: string | null;
  locations: string | null;
  changes: string | null;
  network: string | null;
  sandbox: string | null;
  actionDisclosed: boolean;
  agentTitle: string;
  agentDescription: string;
  allowOnceOptionId: string | null;
  denyOnceOptionId: string | null;
};

export type VerificationCheck = {
  id: string;
  label: string;
  status: 'verified' | 'not verified' | 'failed' | 'n/a';
  source: string;
  lastChecked: null;
};

export type TranscriptItem = {
  key: string;
  kind: string;
  cursorLabel: string;
  occurredAtMs: number;
  markdown: string | null;
  plain: string | null;
};

export type ApprovalFlight = { inFlight: ReadonlySet<string>; error: string | null };

export type ApprovalFlightAction =
  | { type: 'sent'; approvalId: string }
  | { type: 'blocked' }
  | { type: 'rejected' }
  | { type: 'disconnect' }
  | { type: 'resolved'; approvalId: string }
  | { type: 'journal'; kind: string; approvalId: string | null };

const NOT_CHECKED = 'not checked in this browser build';

export const VERIFICATION_CHECKS: readonly VerificationCheck[] = [
  { id: 'sandbox', label: 'Sandbox profile applied', status: 'not verified', source: NOT_CHECKED, lastChecked: null },
  { id: 'bypass', label: 'Bypass / yolo modes off', status: 'not verified', source: NOT_CHECKED, lastChecked: null },
  { id: 'routing', label: 'Approval routing pinned to Shardrail', status: 'not verified', source: NOT_CHECKED, lastChecked: null },
  { id: 'config', label: 'Provider approval config unchanged', status: 'not verified', source: NOT_CHECKED, lastChecked: null },
  { id: 'widening', label: 'No unacknowledged permission-widening mode change', status: 'not verified', source: NOT_CHECKED, lastChecked: null },
  { id: 'containment', label: 'Process containment active', status: 'not verified', source: NOT_CHECKED, lastChecked: null },
  { id: 'credentials', label: 'Agent credential sockets dropped', status: 'not verified', source: NOT_CHECKED, lastChecked: null },
  { id: 'writes', label: 'Writes confined to writable roots', status: 'not verified', source: NOT_CHECKED, lastChecked: null },
  { id: 'identity', label: 'Shardrail socket and identity unreachable', status: 'not verified', source: NOT_CHECKED, lastChecked: null },
  { id: 'mcp', label: 'MCP servers inside the sandbox', status: 'not verified', source: NOT_CHECKED, lastChecked: null },
  { id: 'boundary', label: 'No boundary-breaking approval allowed on this host', status: 'not verified', source: NOT_CHECKED, lastChecked: null },
  { id: 'gaps', label: 'Known provider gaps', status: 'not verified', source: NOT_CHECKED, lastChecked: null },
];

const TERMINAL_APPROVAL_STATES = new Set(['allowed', 'denied', 'expired', 'withdrawn', 'delivered_unconfirmed']);
const CONFIRMED_APPROVAL_KINDS = new Set([
  'approval_responding',
  'approval_responded_unacked',
  'approval_delivered_unconfirmed',
  'approval_withdrawn',
  'allowed',
  'denied',
  'expired',
  'withdrawn',
]);
const HIDDEN_RANGES: readonly (readonly [number, number])[] = [[0, 8], [11, 12], [14, 31], [127, 159], [0x200b, 0x200f], [0x202a, 0x202e], [0x2066, 0x2069]];
const HIDDEN_CHARACTERS = new RegExp(`[${HIDDEN_RANGES.map(([start, end]) => `${String.fromCodePoint(start)}-${String.fromCodePoint(end)}`).join('')}${String.fromCodePoint(0xfeff)}]`, 'g');

const APPROVAL_STATE_BY_KIND: Record<string, string> = {
  approval_responding: 'responding',
  approval_responded_unacked: 'responded_unacked',
  approval_delivered_unconfirmed: 'delivered_unconfirmed',
  approval_withdrawn: 'withdrawn',
  allowed: 'allowed',
  denied: 'denied',
  expired: 'expired',
  withdrawn: 'withdrawn',
};

export function ingestJournalEvents(current: readonly JournalEvent[], incoming: readonly JournalEvent[]) {
  if (!incoming.length) return { events: current, cursor: highWater(current), added: 0 };
  const byCursor = new Map(current.map((event) => [event.cursor, event]));
  let added = 0;
  for (const event of incoming) {
    if (!Number.isSafeInteger(event.cursor) || event.cursor < 0 || !event.sessionId || byCursor.has(event.cursor)) continue;
    byCursor.set(event.cursor, event);
    added += 1;
  }
  if (!added) return { events: current, cursor: highWater(current), added: 0 };
  const events = [...byCursor.values()].sort((left, right) => left.cursor - right.cursor);
  return { events, cursor: events.at(-1)?.cursor ?? 0, added };
}

export function sessionSummaries(events: readonly JournalEvent[]): SessionSummary[] {
  const byId = new Map<string, SessionSummary>();
  for (const event of events) {
    const summary = byId.get(event.sessionId) ?? {
      id: event.sessionId,
      label: event.sessionId,
      agentKind: null,
      lifecycle: 'unknown',
      status: 'unknown',
      lastActivityMs: 0,
      lastCursor: event.cursor,
    };
    if (event.occurredAtMs >= summary.lastActivityMs) summary.lastActivityMs = event.occurredAtMs;
    if (event.cursor >= summary.lastCursor) summary.lastCursor = event.cursor;
    const payload = asRecord(event.payload);
    if (event.kind === 'process_started' && typeof payload?.provider === 'string' && payload.provider.length > 0) {
      summary.agentKind = payload.provider;
    }
    const snapshot = asRecord(payload?.session_snapshot);
    if (snapshot) {
      if (typeof snapshot.lifecycle === 'string') summary.lifecycle = snapshot.lifecycle;
      const turn = typeof snapshot.turn === 'string' ? snapshot.turn : null;
      const process = typeof snapshot.process === 'string' ? snapshot.process : null;
      summary.status = formatStatus(summary.lifecycle, turn, process);
    } else if (event.kind === 'session_created' && summary.lifecycle === 'unknown') {
      summary.lifecycle = 'open';
      summary.status = formatStatus(summary.lifecycle, null, null);
    }
    byId.set(event.sessionId, summary);
  }
  return [...byId.values()].sort((left, right) => right.lastActivityMs - left.lastActivityMs || right.lastCursor - left.lastCursor || left.id.localeCompare(right.id));
}

export function eventsForSession(events: readonly JournalEvent[], sessionId: string) {
  return events.filter((event) => event.sessionId === sessionId);
}

export function agentText(event: JournalEvent) {
  if (event.kind !== 'agent_message_chunk') return null;
  const content = asRecord(asRecord(event.payload)?.content);
  return typeof content?.text === 'string' ? content.text : null;
}

export function plainEventText(event: JournalEvent) {
  try {
    const text = JSON.stringify(event.payload ?? null);
    return typeof text === 'string' ? text : '';
  } catch {
    return '';
  }
}

export function transcriptItems(events: readonly JournalEvent[]): TranscriptItem[] {
  const items: TranscriptItem[] = [];
  for (const event of events) {
    const text = agentText(event);
    const last = items.at(-1);
    if (text !== null && last?.kind === 'agent_message_chunk' && last.markdown !== null) {
      last.markdown += text;
      last.cursorLabel = `${last.cursorLabel.split('–')[0]}–#${event.cursor}`;
      if (Number.isFinite(event.occurredAtMs) && event.occurredAtMs >= last.occurredAtMs) last.occurredAtMs = event.occurredAtMs;
      continue;
    }
    items.push({
      key: `${event.kind}-${event.cursor}`,
      kind: event.kind,
      cursorLabel: `#${event.cursor}`,
      occurredAtMs: event.occurredAtMs,
      markdown: text,
      plain: text === null ? plainEventText(event) : null,
    });
  }
  return items;
}

export function activityTimestamp(ms: number) {
  if (!Number.isFinite(ms) || ms <= 0) return null;
  const date = new Date(ms);
  if (Number.isNaN(date.getTime())) return null;
  return date.toISOString();
}

export function revealHiddenCharacters(value: string) {
  return value.replace(HIDDEN_CHARACTERS, (char) => {
    const code = char.codePointAt(0) ?? 0;
    return `\\u${code.toString(16).padStart(4, '0')}`;
  });
}

export function checklistSummary(checks: readonly VerificationCheck[] = VERIFICATION_CHECKS) {
  const ranked = [...checks].sort((left, right) => checkRank(left.status) - checkRank(right.status));
  return {
    checks: ranked,
    failed: ranked.filter((item) => item.status === 'failed').length,
    unverified: ranked.filter((item) => item.status === 'not verified').length,
  };
}

export function approvalIdFromPayload(payload: unknown) {
  const approvalId = asRecord(payload)?.approval_id;
  return typeof approvalId === 'string' && approvalId.length > 0 ? approvalId : null;
}

export function controlFailureTarget(kind: string, approvalInFlightCount: number): 'approval' | 'prompt' | null {
  if (kind === 'command_rejected') return 'prompt';
  if (kind === 'error') return approvalInFlightCount > 0 ? 'approval' : 'prompt';
  return null;
}

export function clearPromptOnAccept(pendingCommandId: string | null, acceptedCommandId: string) {
  return pendingCommandId !== null && pendingCommandId === acceptedCommandId;
}

export function promptClearApplies(acceptedSessionId: string | null, visibleSessionId: string) {
  return acceptedSessionId !== null && acceptedSessionId === visibleSessionId;
}

export function reduceApprovalFlight(state: ApprovalFlight, action: ApprovalFlightAction): ApprovalFlight {
  if (action.type === 'blocked') return { inFlight: state.inFlight, error: 'Could not send the approval response. It was not allowed.' };
  if (action.type === 'sent') {
    if (state.inFlight.has(action.approvalId) && state.error === null) return state;
    const inFlight = new Set(state.inFlight);
    inFlight.add(action.approvalId);
    return { inFlight, error: null };
  }
  if (action.type === 'rejected') {
    if (state.inFlight.size === 0 && state.error === 'The daemon rejected the approval response. It was not allowed.') return state;
    return { inFlight: new Set(), error: 'The daemon rejected the approval response. It was not allowed.' };
  }
  if (action.type === 'disconnect') {
    if (state.inFlight.size === 0) return state;
    return { inFlight: new Set(), error: 'The approval response was not confirmed. It will not be retried.' };
  }
  if (action.type === 'resolved') return clearApprovalFlight(state, action.approvalId);
  if (!action.approvalId || !CONFIRMED_APPROVAL_KINDS.has(action.kind)) return state;
  return clearApprovalFlight(state, action.approvalId);
}

export function approvalsForSession(events: readonly JournalEvent[], sessionId: string) {
  return collectApprovals(events, new Set([sessionId]));
}

function collectApprovals(events: readonly JournalEvent[], ids: ReadonlySet<string>) {
  const cards = new Map<string, ApprovalCard>();
  for (const event of events) {
    if (!ids.has(event.sessionId)) continue;
    const payload = asRecord(event.payload);
    const approvalId = typeof payload?.approval_id === 'string' ? payload.approval_id : null;
    if (!approvalId) continue;
    if (event.kind === 'approval_requested') {
      if (TERMINAL_APPROVAL_STATES.has(cards.get(approvalId)?.state ?? '')) continue;
      const options = Array.isArray(payload?.options) ? payload.options : [];
      const description = asRecord(payload?.agent_description);
      const action = disclosedAction(payload);
      cards.set(approvalId, {
        approvalId,
        sessionId: event.sessionId,
        state: 'pending',
        decision: null,
        toolKind: typeof payload?.approval_kind === 'string' ? payload.approval_kind : 'unknown',
        toolCallId: typeof payload?.tool_call_id === 'string' ? payload.tool_call_id : '',
        ...action,
        agentTitle: typeof description?.title === 'string' ? description.title : '',
        agentDescription: typeof description?.description === 'string' ? description.description : '',
        allowOnceOptionId: onceOption(options, 'allow'),
        denyOnceOptionId: onceOption(options, 'deny'),
      });
      continue;
    }
    const nextState = APPROVAL_STATE_BY_KIND[event.kind];
    if (!nextState) continue;
    const card = cards.get(approvalId) ?? {
      approvalId,
      sessionId: event.sessionId,
      state: 'pending',
      decision: null,
      toolKind: 'unknown',
      toolCallId: '',
      ...emptyAction(),
      agentTitle: '',
      agentDescription: '',
      allowOnceOptionId: null,
      denyOnceOptionId: null,
    };
    if (!TERMINAL_APPROVAL_STATES.has(card.state)) card.state = nextState;
    if (typeof payload?.decision === 'string') card.decision = payload.decision;
    cards.set(approvalId, card);
  }
  return [...cards.values()];
}

export function approvalState(events: readonly JournalEvent[], approvalId: string) {
  const card = collectApprovals(events, new Set(events.map((event) => event.sessionId))).find((item) => item.approvalId === approvalId);
  return card?.state ?? null;
}

function formatStatus(lifecycle: string, turn: string | null, process: string | null) {
  const parts = [lifecycle];
  if (turn) parts.push(turn.toLowerCase());
  if (process) parts.push(process.toLowerCase());
  return parts.join(' · ');
}

function onceOption(options: unknown[], decision: 'allow' | 'deny') {
  for (const option of options) {
    const record = asRecord(option);
    if (record?.class === 'once' && record.decision === decision && typeof record.option_id === 'string' && record.option_id.length > 0) return record.option_id;
  }
  return null;
}

function highWater(events: readonly JournalEvent[]) {
  return events.reduce((cursor, event) => Math.max(cursor, event.cursor), 0);
}

function clearApprovalFlight(state: ApprovalFlight, approvalId: string): ApprovalFlight {
  if (!state.inFlight.has(approvalId)) return state;
  const inFlight = new Set(state.inFlight);
  inFlight.delete(approvalId);
  return { inFlight, error: state.error };
}

function asRecord(value: unknown) {
  return value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null;
}

function checkRank(status: VerificationCheck['status']) {
  if (status === 'failed' || status === 'not verified') return 0;
  return 1;
}

function emptyAction() {
  return { command: null, argv: null, rawInput: null, cwd: null, locations: null, changes: null, network: null, sandbox: null, actionDisclosed: false };
}

function disclosedAction(payload: Record<string, unknown> | null) {
  const command = textValue(payload?.command);
  const argv = argvValue(payload?.argv);
  const rawInput = jsonValue(payload?.raw_input);
  const cwd = textValue(payload?.cwd);
  const locations = jsonValue(payload?.locations);
  const changes = jsonValue(payload?.changes);
  const network = textValue(payload?.network);
  const sandbox = textValue(payload?.sandbox);
  return {
    command,
    argv,
    rawInput,
    cwd,
    locations,
    changes,
    network,
    sandbox,
    actionDisclosed: Boolean(command || argv || rawInput || locations || changes || network),
  };
}

function textValue(value: unknown) {
  return typeof value === 'string' && value.length > 0 ? value : null;
}

function argvValue(value: unknown) {
  if (!Array.isArray(value) || value.length === 0 || value.some((item) => typeof item !== 'string' || item.length === 0)) return null;
  return value as string[];
}

function jsonValue(value: unknown) {
  if (value == null) return null;
  if (typeof value === 'string') return value.length > 0 ? value : null;
  try {
    const text = JSON.stringify(value);
    return text && text !== '{}' && text !== '[]' && text !== 'null' ? text : null;
  } catch {
    return null;
  }
}
