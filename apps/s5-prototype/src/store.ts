import { useSyncExternalStore } from 'react';
import { measureAfterPaint } from './metrics-utils';

export type EventRow = { id: number; agent: number; kind: 'message' | 'tool' | 'approval'; text: string; command: string; cwd: string; at: number };
let rows: EventRow[] = [];
let rowIndexById = new Map<number, number>();
const listeners = new Set<() => void>();
let pending = new Map<number, EventRow>();
let raf = 0;
const receivedAt = new Map<number, number>();
const scheduledLatency = new Set<number>();
const daemonReceipts = new Map<number, number>();
const daemonScheduled = new Set<number>();
const daemonAgents = new Map<string, number>();
let daemonAgentCount = 1;
let daemonAgentNext = 0;
let pageClockOffsetMs = 0;
let pageClockUncertaintyMs = 0;
export const daemonReceiptToRafMs: number[] = [];
export let daemonAgentChunkCount = 0;
const activeMessageByAgent = new Map<number, number>();
export const mockCreationToMountedRowFollowupLatency: number[] = [];
let generatedTokens = 0;
export const subscribe = (fn: () => void) => { listeners.add(fn); return () => listeners.delete(fn); };
export const getRows = () => rows;
export const useRows = () => useSyncExternalStore(subscribe, getRows, getRows);
export function applyPendingUpdates(current: EventRow[], indexById: Map<number, number>, updates: EventRow[]) {
  if (!updates.length) return current;
  const next = [...current];
  for (const update of updates) {
    const index = indexById.get(update.id);
    if (index === undefined) {
      indexById.set(update.id, next.length);
      next.push(update);
    } else next[index] = update;
  }
  return next;
}
export function reset(count: number, streams: number) {
  rows = Array.from({ length: Math.max(0, count) }, (_, id) => makeRow(id, streams));
  rowIndexById = new Map(rows.map((row, index) => [row.id, index]));
  pending = new Map();
  receivedAt.clear();
  scheduledLatency.clear();
  activeMessageByAgent.clear();
  daemonReceipts.clear(); daemonScheduled.clear(); daemonAgents.clear(); daemonAgentNext = 0;
  daemonReceiptToRafMs.length = 0;
  daemonAgentChunkCount = 0;
  mockCreationToMountedRowFollowupLatency.length = 0;
  generatedTokens = 0;
  listeners.forEach((fn) => fn());
}
export function setDaemonClockMapping(offsetMs: number, uncertaintyMs: number) { pageClockOffsetMs = offsetMs; pageClockUncertaintyMs = uncertaintyMs; }
export function setDaemonAgentCount(count: number) { daemonAgentCount = Math.max(1, count); }
export function daemonClockMapping() { return { offsetMs: pageClockOffsetMs, uncertaintyMs: pageClockUncertaintyMs }; }
export function applyDaemonEvent(cursor: number, event: { session_id: string; kind: string; payload: unknown }, receiptWallClockMs: number | null | undefined) {
  if (event.kind === 'agent_message_chunk') daemonAgentChunkCount++;
  const agent = daemonAgents.get(event.session_id) ?? daemonAgentNext++ % Math.max(1, daemonAgentCount);
  daemonAgents.set(event.session_id, agent);
  const payload = event.payload as { content?: { text?: string }; toolCallId?: string };
  const id = 1_000_000_000 + cursor;
  const kind = event.kind === 'approval_requested' ? 'approval' : event.kind.includes('tool') ? 'tool' : 'message';
  const text = payload?.content?.text ?? JSON.stringify(event.payload);
  const row = { id, agent, kind, text, command: payload?.toolCallId ?? '', cwd: '', at: performance.now() } as EventRow;
  if (event.kind === 'agent_message_chunk' && receiptWallClockMs !== undefined && receiptWallClockMs !== null) daemonReceipts.set(id, receiptWallClockMs);
  pending.set(id, row);
  if (!raf) raf = requestAnimationFrame(() => {
    rows = applyPendingUpdates(rows, rowIndexById, [...pending.values()]);
    pending = new Map(); raf = 0; listeners.forEach((fn) => fn());
  });
  return id;
}
export function markDaemonRowsCommitted(ids: number[], requestFrame: (callback: FrameRequestCallback) => number = requestAnimationFrame, now: () => number = () => performance.now(), isCommitted: (id: number) => boolean = (id) => Boolean(document.querySelector(`[data-daemon-event-id="${id}"]`))) {
  for (const id of ids) {
    const receipt = daemonReceipts.get(id);
    if (receipt === undefined || daemonScheduled.has(id) || !isCommitted(id)) continue;
    daemonScheduled.add(id);
    requestFrame(() => requestFrame(() => {
      daemonReceiptToRafMs.push(now() - (receipt - pageClockOffsetMs));
      daemonReceipts.delete(id); daemonScheduled.delete(id);
    }));
  }
}
function makeRow(id: number, streams: number): EventRow {
  const kind = id % 97 === 0 ? 'approval' : id % 11 === 0 ? 'tool' : 'message';
  const agent = id % Math.max(streams, 1);
  return { id, agent, kind, text: `Agent ${agent + 1} update ${id}: **Streaming note** with a safe [reference](https://example.com).\n\n- deterministic fixture\n- event timestamp ${Date.now()}`, command: `git status --short --branch # fixture ${id}`, cwd: `/tmp/s5/worktree-${agent}`, at: performance.now() };
}
export function pushToken(agent: number, seq: number) {
  generatedTokens++;
  let id = activeMessageByAgent.get(agent);
  let row = id === undefined ? undefined : pending.get(id) ?? rows[rowIndexById.get(id) ?? -1];
  if (!row) {
    const at = performance.now();
    row = makeRow(rows.length + pending.size, Math.max(agent + 1, 1));
    row.agent = agent;
    row.kind = 'message';
    row.text = `Agent ${agent + 1} streaming message`;
    row.at = at;
    id = row.id;
    activeMessageByAgent.set(agent, id);
    receivedAt.set(id, at);
    pending.set(row.id, row);
  }
  row.text += ` token ${seq}`;
  row.at = performance.now();
  if (!pending.has(row.id)) pending.set(row.id, row);
  if (!raf) raf = requestAnimationFrame(() => {
    rows = applyPendingUpdates(rows, rowIndexById, [...pending.values()]);
    pending = new Map();
    raf = 0;
    listeners.forEach((fn) => fn());
  });
}
export function markRowsCommitted(
  ids: number[],
  requestFrame: (callback: FrameRequestCallback) => number = requestAnimationFrame,
  afterFrame: (callback: () => void) => unknown = (callback) => window.setTimeout(callback, 0),
  now: () => number = () => performance.now(),
) {
  for (const id of ids) {
    const at = receivedAt.get(id);
    if (at === undefined || scheduledLatency.has(id)) continue;
    scheduledLatency.add(id);
    measureAfterPaint(at, now, (callback) => requestFrame(callback), afterFrame, (elapsed) => {
      mockCreationToMountedRowFollowupLatency.push(elapsed);
      receivedAt.delete(id);
      scheduledLatency.delete(id);
      if (mockCreationToMountedRowFollowupLatency.length > 5000) mockCreationToMountedRowFollowupLatency.splice(0, mockCreationToMountedRowFollowupLatency.length - 5000);
    });
  }
}
export function eventCount() { return rows.length + [...pending.keys()].filter((id) => !rowIndexById.has(id)).length; }
export function tokenCount() { return generatedTokens; }
export function sortedLatency() { return [...mockCreationToMountedRowFollowupLatency].sort((a, b) => a - b); }
