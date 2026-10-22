import { useSyncExternalStore } from 'react';
import { measureAfterPaint } from './metrics-utils';
import { hashContent } from './streaming-view';

export type EventRow = { id: number; agent: number; kind: 'message' | 'tool' | 'approval'; text: string; contentHash: string; contentVersion: number; command: string; cwd: string; at: number };
let rows: EventRow[] = [];
let rowStart = 0;
let rowTotal = 0;
let liveTail = true;
let rowIndexById = new Map<number, number>();
const listeners = new Set<() => void>();
let revision = 0;
let pending = new Map<number, EventRow>();
let raf = 0;
const receivedAt = new Map<number, number>();
const scheduledLatency = new Set<number>();
const daemonReceipts = new Map<number, number>();
// shortcut: only the newest 512 unmounted receipts are retained, upgrade if B4 must measure delayed historical virtual rows.
const maxPendingDaemonReceipts = 512;
const daemonScheduled = new Set<number>();
const daemonAgents = new Map<string, number>();
const daemonSessionAgents = new Map<string, number>();
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
export const useRows = () => { useSyncExternalStore(subscribe, () => revision, () => revision); return rows; };
export const getRowStart = () => rowStart;
export const rowCount = () => rowTotal;
export const setLiveTail = (value: boolean) => { liveTail = value; };
export const isLiveTail = () => liveTail;
export function setDaemonTotal(total: number, publish = true) { rowTotal = total; if (publish) notify(); }
export function setDaemonAgentChunkCount(count: number) { daemonAgentChunkCount = count; }
export function setDaemonWindow(start: number, total: number, values: { cursor: number; event: { session_id: string; kind: string; payload: unknown }; receipt_wall_clock_ms?: number | null }[], version: number) {
  rowTotal = total;
  const nextRows = values.map(({ cursor, event, receipt_wall_clock_ms }) => {
    const id = 1_000_000_000 + cursor;
    const payload = event.payload as { content?: { text?: string }; toolCallId?: string };
    const text = payload?.content?.text ?? JSON.stringify(event.payload);
    if (event.kind === 'agent_message_chunk' && receipt_wall_clock_ms != null) {
      daemonReceipts.set(id, receipt_wall_clock_ms);
      while (daemonReceipts.size > maxPendingDaemonReceipts) daemonReceipts.delete(daemonReceipts.keys().next().value!);
    }
    const agent = daemonSessionAgents.get(event.session_id) ?? daemonSessionAgents.size;
    daemonSessionAgents.set(event.session_id, agent);
    return { id, agent, kind: event.kind === 'approval_requested' ? 'approval' : event.kind.includes('tool') ? 'tool' : 'message', text, contentHash: hashContent(text), contentVersion: 0, command: payload?.toolCallId ?? '', cwd: '', at: 0 } as EventRow;
  });
  pendingDaemonWindow = { start, total, rows: nextRows, version };
  if (!daemonWindowRaf) daemonWindowRaf = requestAnimationFrame(() => {
    const next = pendingDaemonWindow;
    pendingDaemonWindow = undefined;
    daemonWindowRaf = 0;
    if (!next) return;
    rows = next.rows;
    rowStart = next.start;
    rowTotal = next.total;
    windowVersion = next.version;
    notify();
  });
}
let windowVersion = 0;
let daemonWindowRaf = 0;
let pendingDaemonWindow: { start: number; total: number; rows: EventRow[]; version: number } | undefined;
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
function notify() { revision++; listeners.forEach((fn) => fn()); }
export function reset(count: number, streams: number, seedRows = true) {
  rows = Array.from({ length: Math.max(0, count) }, (_, id) => makeRow(id, streams));
  if (!seedRows) rows = [];
  rowStart = 0;
  rowTotal = rows.length;
  windowVersion = 0;
  liveTail = true;
  pendingDaemonWindow = undefined;
  if (daemonWindowRaf) cancelAnimationFrame(daemonWindowRaf);
  daemonWindowRaf = 0;
  rowIndexById = new Map(rows.map((row, index) => [row.id, index]));
  pending = new Map();
  receivedAt.clear();
  scheduledLatency.clear();
  activeMessageByAgent.clear();
  daemonReceipts.clear(); daemonScheduled.clear(); daemonAgents.clear(); daemonSessionAgents.clear(); daemonAgentNext = 0;
  daemonReceiptToRafMs.length = 0;
  daemonAgentChunkCount = 0;
  mockCreationToMountedRowFollowupLatency.length = 0;
  generatedTokens = 0;
  notify();
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
  const row = { id, agent, kind, text, contentHash: hashContent(text), contentVersion: 0, command: payload?.toolCallId ?? '', cwd: '', at: performance.now() } as EventRow;
  if (event.kind === 'agent_message_chunk' && receiptWallClockMs !== undefined && receiptWallClockMs !== null) {
    daemonReceipts.set(id, receiptWallClockMs);
    if (daemonReceipts.size > maxPendingDaemonReceipts) {
      const oldest = daemonReceipts.keys().next();
      if (!oldest.done) daemonReceipts.delete(oldest.value);
    }
  }
  pending.set(id, row);
  if (!raf) raf = requestAnimationFrame(() => {
    rows = applyPendingUpdates(rows, rowIndexById, [...pending.values()]);
    rowTotal = rows.length;
    pending = new Map(); raf = 0; notify();
  });
  return id;
}
export function markDaemonRowsCommitted(ids: number[], requestFrame: (callback: FrameRequestCallback) => number = requestAnimationFrame, now: () => number = () => performance.now(), isCommitted: (id: number) => boolean = () => false) {
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
  const text = `Agent ${agent + 1} update ${id}: **Streaming note** with a safe [reference](https://example.com).\n\n- deterministic fixture\n- event timestamp ${Date.now()}`;
  return { id, agent, kind, text, contentHash: hashContent(text), contentVersion: 0, command: `git status --short --branch # fixture ${id}`, cwd: `/tmp/s5/worktree-${agent}`, at: performance.now() };
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
  row.contentVersion++;
  row.at = performance.now();
  if (!pending.has(row.id)) pending.set(row.id, row);
  if (!raf) raf = requestAnimationFrame(() => {
    rows = applyPendingUpdates(rows, rowIndexById, [...pending.values()]);
    rowTotal = rows.length;
    pending = new Map();
    raf = 0;
    notify();
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
export function eventCount() { return windowVersion ? rowTotal : rows.length + [...pending.keys()].filter((id) => !rowIndexById.has(id)).length; }
export function pendingDaemonReceiptCount() { return daemonReceipts.size; }
export function tokenCount() { return generatedTokens; }
export function sortedLatency() { return [...mockCreationToMountedRowFollowupLatency].sort((a, b) => a - b); }
