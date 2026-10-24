import type { BrowserKeyRecord } from '@shardrail/client/browser';
import type { CommandRequest, DaemonFrame } from '@shardrail/contracts';
import type { JournalEvent } from './session-model';

export type ConnectionPhase = 'connecting' | 'connected' | 'reconnecting' | 'offline' | 'key-changed';

export type StoredCursor = { cursor: number; journalEpoch: string };

export interface SessionSocket {
  host: { journal_epoch: string };
  onFrame(listener: (frame: DaemonFrame) => void): () => void;
  onError(listener: (error: Error) => void): () => void;
  resolveApproval(request: { approval_id: string; option_id: string }): void;
  request(request: CommandRequest): void;
  close(): void;
}

export interface BrowserSessionOptions {
  connect(options: { url: string; identity: BrowserKeyRecord; cursor?: number; journalEpoch?: string }): Promise<SessionSocket>;
  url: string;
  identity: BrowserKeyRecord;
  loadCursor(): StoredCursor | undefined;
  saveCursor(cursor: number, journalEpoch: string): void;
  schedule(delayMs: number, run: () => void): () => void;
  online(): boolean;
  onStatus(phase: ConnectionPhase): void;
  onDurableEvent(event: JournalEvent): void;
  onControl(frame: Exclude<DaemonFrame, { kind: 'durable_event' }>): void;
  onEpoch(epoch: string): void;
}

export interface BrowserSession {
  reconnect(): void;
  stop(): void;
  setOnline(online: boolean): void;
  resolveApproval(approvalId: string, optionId: string): 'sent' | 'blocked';
  request(request: CommandRequest): 'sent' | 'blocked';
}

export function reconnectDelayMs(attempt: number) {
  if (!Number.isSafeInteger(attempt) || attempt < 1) return 200;
  return Math.min(8_000, 200 * 2 ** Math.min(attempt - 1, 6));
}

export function connectionStatusLabel(phase: ConnectionPhase) {
  switch (phase) {
    case 'connecting': return 'Connecting · browser WebSocket';
    case 'connected': return 'Connected · browser WebSocket';
    case 'reconnecting': return 'Reconnecting · browser WebSocket';
    case 'offline': return 'Offline · browser WebSocket';
    case 'key-changed': return 'Key changed · pairing is blocked';
  }
}

export function isKeyChangedError(error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  return /key changed|pairing is blocked/i.test(message);
}

export function parseStoredCursor(raw: string | null): StoredCursor | undefined {
  if (!raw) return undefined;
  try {
    const saved = JSON.parse(raw) as { cursor?: unknown; journalEpoch?: unknown };
    if (saved && Number.isSafeInteger(saved.cursor) && (saved.cursor as number) >= 0 && typeof saved.journalEpoch === 'string' && saved.journalEpoch.length > 0) {
      return { cursor: saved.cursor as number, journalEpoch: saved.journalEpoch };
    }
  } catch { /* Ignore corrupt cursor state and replay from the beginning. */ }
  return undefined;
}

export function cursorStorageKey(hostPublicKey: string) {
  return `shardrail-browser-cursor:${hostPublicKey}`;
}

export function createBrowserSession(options: BrowserSessionOptions): BrowserSession {
  let generation = 0;
  let stopped = false;
  let attempt = 0;
  let phase: ConnectionPhase = 'connecting';
  let active: SessionSocket | undefined;
  let cursor = options.loadCursor();
  let highWater = cursor?.cursor ?? -1;
  const seen = new Set<number>();
  let cancelTimer: (() => void) | undefined;

  const setPhase = (next: ConnectionPhase) => {
    phase = next;
    options.onStatus(next);
  };
  const clearTimer = () => {
    cancelTimer?.();
    cancelTimer = undefined;
  };
  const remember = (epoch: string) => {
    const changed = cursor !== undefined && cursor.journalEpoch !== epoch;
    if (!cursor || changed) {
      seen.clear();
      highWater = -1;
    }
    if (changed) cursor = undefined;
    options.onEpoch(epoch);
  };

  async function open(retrying: boolean) {
    if (stopped) return;
    clearTimer();
    const token = ++generation;
    const previous = active;
    active = undefined;
    previous?.close();
    if (!options.online()) {
      setPhase('offline');
      return;
    }
    setPhase(retrying ? 'reconnecting' : 'connecting');
    try {
      const saved = cursor;
      const client = await options.connect({
        url: options.url,
        identity: options.identity,
        ...(saved ? { cursor: saved.cursor, journalEpoch: saved.journalEpoch } : {}),
      });
      if (stopped || token !== generation) {
        client.close();
        return;
      }
      remember(client.host.journal_epoch);
      active = client;
      attempt = 0;
      setPhase('connected');
      client.onFrame((frame) => {
        if (stopped || token !== generation) return;
        if (frame.kind !== 'durable_event') {
          options.onControl(frame);
          return;
        }
        const event = journalEvent(frame);
        if (!event || seen.has(event.cursor)) return;
        seen.add(event.cursor);
        if (event.cursor > highWater) {
          highWater = event.cursor;
          cursor = { cursor: highWater, journalEpoch: client.host.journal_epoch };
          options.saveCursor(highWater, client.host.journal_epoch);
        }
        options.onDurableEvent(event);
      });
      client.onError((error) => {
        if (stopped || token !== generation) return;
        generation += 1;
        active = undefined;
        if (isKeyChangedError(error)) {
          setPhase('key-changed');
          return;
        }
        scheduleRetry();
      });
    } catch (error) {
      if (stopped || token !== generation) return;
      if (isKeyChangedError(error)) {
        setPhase('key-changed');
        return;
      }
      scheduleRetry();
    }
  }

  function scheduleRetry() {
    if (stopped || phase === 'key-changed') return;
    if (!options.online()) {
      setPhase('offline');
      return;
    }
    attempt += 1;
    setPhase('reconnecting');
    cancelTimer = options.schedule(reconnectDelayMs(attempt), () => { void open(true); });
  }

  void open(false);
  return {
    reconnect() {
      if (stopped) return;
      attempt = 0;
      clearTimer();
      void open(phase === 'connected' || phase === 'reconnecting' || phase === 'offline');
    },
    stop() {
      stopped = true;
      clearTimer();
      generation += 1;
      const previous = active;
      active = undefined;
      previous?.close();
    },
    setOnline(online) {
      if (stopped || phase === 'key-changed') return;
      if (!online) {
        clearTimer();
        generation += 1;
        const previous = active;
        active = undefined;
        previous?.close();
        setPhase('offline');
        return;
      }
      if (phase === 'offline') void open(true);
    },
    resolveApproval(approvalId, optionId) {
      if (stopped || phase !== 'connected' || !active || !approvalId || !optionId) return 'blocked';
      try {
        active.resolveApproval({ approval_id: approvalId, option_id: optionId });
        return 'sent';
      } catch {
        return 'blocked';
      }
    },
    request(request) {
      if (stopped || phase !== 'connected' || !active) return 'blocked';
      try {
        active.request(request);
        return 'sent';
      } catch {
        return 'blocked';
      }
    },
  };
}

function journalEvent(frame: Extract<DaemonFrame, { kind: 'durable_event' }>): JournalEvent | null {
  const cursor = frame.cursor;
  const sessionId = frame.event.session_id;
  if (!Number.isSafeInteger(cursor) || cursor < 0 || typeof sessionId !== 'string' || sessionId.length === 0) return null;
  return {
    cursor,
    sessionId,
    kind: frame.event.kind,
    occurredAtMs: typeof frame.event.occurred_at_ms === 'number' ? frame.event.occurred_at_ms : 0,
    payload: frame.event.payload,
  };
}
