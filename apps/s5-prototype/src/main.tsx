import React, { Profiler, memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { listen } from '@tauri-apps/api/event';
import { Channel, invoke } from '@tauri-apps/api/core';
import { Button, Input, Label, ListBox, ListBoxItem, Popover, Select, SelectValue, TextField } from 'react-aria-components';
import { useVirtualizer } from '@tanstack/react-virtual';
import { createPortal } from 'react-dom';
import { applyDaemonEvent, daemonAgentChunkCount, daemonClockMapping, daemonReceiptToRafMs, eventCount, getRowStart, getRows, isLiveTail, markDaemonRowsCommitted, markRowsCommitted, observeJournalEpoch, pushToken, reset, rowCount, setDaemonAgentChunkCount, setDaemonAgentCount, setDaemonClockMapping, setDaemonTotal, setDaemonWindow, setLiveTail, sortedLatency, tokenCount, useRows } from './store';
import { matchedInFirstFrame, measureReadyCommitAfterFrame, nominalFrameIntervalMs, percentile, supportsLongTaskEntries, takeCommittedInput } from './metrics-utils';
import { createFrameSampler, intersectingVirtualItems } from './frame-sampler';
import { draftPreview, minimumB4Samples, shouldWaitForDaemonStream, waitForB4Samples, waitForDaemonStream } from './bench-utils';
import { UntrustedMarkdown } from './UntrustedMarkdown';
import { coalesceChannelFrame, liveTailWindowSize, rangeForWindow, sameMarkdownVersion, transcriptOverscan } from './streaming-view';
import { beginConfirmation, confirmPairing, stateFromInfo, unpairClient, type PairingApi, type PairingInfo, type PairingState } from './pairing';
import { browserPairingApi, browserWebSocketUrl } from './browser-pairing';
import { connectBrowser, openBrowserKeyStore } from '@shardrail/client/browser';
import { createBrowserSession, createFlightOnControl, createFlightOnSessionCreated, isKeyChangedError, noteCreateSent, sessionWorkdirs, type BrowserSession, type ConnectionPhase, type CreateFlight, type CreateFlightResult } from './browser-session';
import { approvalIdFromPayload, clearPromptOnAccept, controlFailureTarget, reduceApprovalFlight, type ApprovalBindingEcho, type ApprovalFlight, type ApprovalFlightAction } from './session-model';
import { MainScreen } from './MainScreen';
import { PairingScreen } from './screens/entry/PairingScreen';
import './styles/index.css';

type Metrics = { firstTranscriptRenderOpportunityMs: number; longTasks: number | null; maxLongTaskMs: number | null; frameP95: number | null; frameMax: number | null; frameSamples: number; commitCount: number; commitMs: number; eventStoreCount: number; generatedTokens: number; heapMB: number | null; mockCreationToMountedRowFollowupP95: number | null; composerP95: number | null; composerMin: number | null; composerMax: number | null; composerSamples: number; matchesByFirstFrame: number; firstFrameSamples: number; firstFrameDeltasMs: number[]; firstFrameRafTimestampDeltasMs: number[]; b4: { count: number; medianMs: number | null; p95Ms: number | null; maxMs: number | null; samplesMs: number[]; clockOffsetMs: number; clockUncertaintyMs: number }; daemonAgentChunkCount: number; userAgent: string };
const started = performance.now();
const browserMode = !('__TAURI_INTERNALS__' in window);
const b4Enabled = new URLSearchParams(location.search).has('b4');
const browserMain = browserMode && !b4Enabled;
const desktopPairingApi: PairingApi = {
  inspect: () => invoke<PairingInfo>('pairing_info'),
  pair: (daemonPublicKey) => invoke<string>('pair_desktop', { expectedPublicKey: daemonPublicKey }),
  unpair: () => invoke('unpair_desktop'),
};
const pairingApi = browserMode ? browserPairingApi : desktopPairingApi;
function b4Trace(stage: string, data: Record<string, unknown> = {}) {
  if (b4Enabled) void invoke('b4_trace', { stage, data }).catch((error: unknown) => console.error('B4 trace command failed', error));
}
if (b4Enabled) {
  window.addEventListener('error', (event) => b4Trace('window.onerror', { message: event.message, source: event.filename, line: event.lineno, column: event.colno }));
  window.addEventListener('unhandledrejection', (event) => b4Trace('unhandledrejection', { reason: String(event.reason) }));
}
const observed = { longTasks: 0, maxLongTaskMs: 0, frames: [] as number[], commits: 0, commitMs: 0, inputs: [] as number[], firstFrame: [] as boolean[], firstFrameTimestamps: [] as number[], rafTimestampDeltas: [] as number[] };
const pendingInputs: { startedAt: number; draft: string }[] = [];
const supportsLongTasks = typeof PerformanceObserver !== 'undefined' && supportsLongTaskEntries(PerformanceObserver.supportedEntryTypes);
if (supportsLongTasks) new PerformanceObserver((list) => list.getEntries().forEach((e) => { observed.longTasks++; observed.maxLongTaskMs = Math.max(observed.maxLongTaskMs, e.duration); })).observe({ type: 'longtask', buffered: true });
const frameSampler = createFrameSampler(requestAnimationFrame, cancelAnimationFrame, (elapsed) => {
  observed.frames.push(elapsed);
  if (observed.frames.length > 12000) observed.frames.shift();
}, () => performance.now());
function startFrameSampling() { observed.frames.length = 0; frameSampler.start(); }
function stopFrameSampling() { frameSampler.stop(); }
function startStreams(streams: number, durationMs: number, running: { current: boolean }, setRunning: (value: boolean) => void) {
  if (running.current) return;
  running.current = true;
  setRunning(true);
  startFrameSampling();
  for (let agent = 0; agent < streams; agent++) {
    let seq = 0;
    const timer = window.setInterval(() => pushToken(agent, seq++), 1000 / 75);
    window.setTimeout(() => { clearInterval(timer); running.current = false; stopFrameSampling(); setRunning(false); }, durationMs);
  }
}
function metrics(): Metrics {
  const frames = [...observed.frames].sort((a, b) => a - b), ls = sortedLatency();
  const inputs = [...observed.inputs].sort((a, b) => a - b);
  const memory = (performance as Performance & { memory?: { usedJSHeapSize: number } }).memory?.usedJSHeapSize;
  const b4 = [...daemonReceiptToRafMs].sort((a, b) => a - b), clock = daemonClockMapping();
  return { firstTranscriptRenderOpportunityMs: (window as Window & { __firstTranscriptRenderOpportunityMs?: number }).__firstTranscriptRenderOpportunityMs ?? 0, longTasks: supportsLongTasks ? observed.longTasks : null, maxLongTaskMs: supportsLongTasks ? observed.maxLongTaskMs : null, frameP95: percentile(frames, .95), frameMax: frames.at(-1) ?? null, frameSamples: observed.frames.length, commitCount: observed.commits, commitMs: observed.commitMs, eventStoreCount: eventCount(), generatedTokens: tokenCount(), heapMB: memory ? +(memory / 1048576).toFixed(1) : null, mockCreationToMountedRowFollowupP95: percentile(ls, .95), composerP95: percentile(observed.inputs, .95), composerMin: inputs[0] ?? null, composerMax: inputs.at(-1) ?? null, composerSamples: observed.inputs.length, matchesByFirstFrame: observed.firstFrame.filter(Boolean).length, firstFrameSamples: observed.firstFrame.length, firstFrameDeltasMs: [...observed.firstFrameTimestamps], firstFrameRafTimestampDeltasMs: [...observed.rafTimestampDeltas], b4: { count: b4.length, medianMs: percentile(b4, .5), p95Ms: percentile(b4, .95), maxMs: b4.at(-1) ?? null, samplesMs: b4, clockOffsetMs: clock.offsetMs, clockUncertaintyMs: clock.uncertaintyMs }, daemonAgentChunkCount, userAgent: navigator.userAgent };
}
Object.assign(window, { __s5Metrics: metrics, __s5ResetInputTrace: () => { observed.inputs.length = 0; observed.firstFrame.length = 0; observed.firstFrameTimestamps.length = 0; observed.rafTimestampDeltas.length = 0; pendingInputs.length = 0; }, __firstTranscriptRenderOpportunityMs: 0 });

const Markdown = memo(({ source }: { rowId: number; source: string; contentHash: string; contentVersion: number }) => {
  const [target, setTarget] = useState<HTMLElement | null>(null);
  const frameRef = useRef<HTMLIFrameElement>(null);
  useEffect(() => {
    const frame = frameRef.current;
    if (!frame) return;
    const doc = frame.contentDocument;
    if (!doc) return;
    const style = doc.createElement('style');
    style.textContent = 'body{margin:0;background:transparent;color:#e6e8ed;font:inherit}p{margin:4px 0;color:inherit}a{color:#77c5e5}';
    doc.head.append(style);
    const root = doc.createElement('div');
    root.id = 'markdown-root';
    doc.body.append(root);
    setTarget(root);
  }, []);
  const content = <UntrustedMarkdown source={source}/>;
  return <iframe ref={frameRef} title="Agent-authored Markdown" sandbox="allow-same-origin" className="markdown-frame">{target ? createPortal(content, target) : null}</iframe>;
}, sameMarkdownVersion);
function Row({ row, onMount, onUnmount }: { row: ReturnType<typeof getRows>[number]; onMount: (id: number) => void; onUnmount: (id: number) => void }) {
  return <article ref={(node) => { if (row.id >= 1_000_000_000) { if (node) onMount(row.id); else onUnmount(row.id); } }} className={`event ${row.kind}`}><div className="meta"><b>Agent {row.agent + 1}</b><span>{row.kind === 'approval' ? 'approval requested' : row.kind === 'tool' ? 'tool call' : 'message'} · #{row.id}</span><time>{new Date().toLocaleTimeString()}</time></div>{row.kind === 'approval' ? <><pre>{row.command}</pre><div className="untrusted"><b>Agent’s description · untrusted</b><Markdown rowId={row.id} source={row.text} contentHash={row.contentHash} contentVersion={row.contentVersion}/></div><small>Checklist: sandbox not verified · approval routing not verified · containment not verified</small></> : row.kind === 'tool' ? <><pre>{row.command}</pre><small>{row.cwd}</small></> : <div className="body"><Markdown rowId={row.id} source={row.text} contentHash={row.contentHash} contentVersion={row.contentVersion}/></div>}</article>;
}
function Transcript({ onInput, ready, requestRange }: { onInput: () => void; ready: boolean; requestRange: (start: number, limit: number) => void }) {
  const rows = useRows();
  const parent = useRef<HTMLDivElement>(null);
  const followTail = useRef(true);
  const firstPaintMeasured = useRef(false);
  const mountedDaemonRows = useRef(new Set<number>());
  // TanStack Virtual returns imperative methods that React Compiler cannot safely memoize.
  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({ count: rowCount(), getScrollElement: () => parent.current, estimateSize: () => 112, overscan: transcriptOverscan(b4Enabled) });
  const virtualItems = virtualizer.getVirtualItems();
  useLayoutEffect(() => {
    if (followTail.current && parent.current) parent.current.scrollTop = parent.current.scrollHeight;
    const viewport = parent.current;
    const intersectingItems = viewport ? intersectingVirtualItems(virtualItems, viewport.scrollTop, viewport.clientHeight) : [];
    const mountedRows = virtualItems.flatMap((item) => rows[item.index - getRowStart()] ? [rows[item.index - getRowStart()]!] : []);
    const visibleRows = intersectingItems.flatMap((item) => rows[item.index - getRowStart()] ? [rows[item.index - getRowStart()]!] : []);
    markRowsCommitted(mountedRows.map((row) => row.id));
    markDaemonRowsCommitted(
      visibleRows.map((row) => row.id),
      requestAnimationFrame,
      () => performance.now(),
      (id) => mountedDaemonRows.current.has(id),
    );
    if (ready && !firstPaintMeasured.current) {
      firstPaintMeasured.current = true;
      measureReadyCommitAfterFrame(true, started, () => performance.now(), requestAnimationFrame, (callback) => window.setTimeout(callback, 0), (elapsed) => { (window as Window & { __firstTranscriptRenderOpportunityMs?: number }).__firstTranscriptRenderOpportunityMs = elapsed; });
    }
  }, [rows, virtualItems, ready]);
  useEffect(() => {
    const first = virtualItems[0]?.index;
    if (first !== undefined) requestRange(first, Math.max(1, (virtualItems.at(-1)?.index ?? first) - first + 1));
  }, [virtualItems, requestRange]);
  // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- scrollable transcript needs keyboard focus
  return <><div ref={parent} className="transcript" role="region" aria-label="Event transcript" tabIndex={0} onScroll={() => { followTail.current = parent.current !== null && parent.current.scrollHeight - parent.current.scrollTop - parent.current.clientHeight < 24; setLiveTail(followTail.current); onInput(); }}><div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>{virtualItems.map((v) => { const row = rows[v.index - getRowStart()]; return row ? <div key={row.id} data-index={v.index} style={{ position: 'absolute', top: 0, left: 0, width: '100%', height: 112, transform: `translateY(${v.start}px)` }}><Row row={row} onMount={(id) => mountedDaemonRows.current.add(id)} onUnmount={(id) => mountedDaemonRows.current.delete(id)}/></div> : null; })}</div></div></>;
}
function App() {
  const query = useMemo(() => new URLSearchParams(location.search), []), count = Math.max(0, Number(query.get('events') ?? 5000)), streams = Number(query.get('streams') ?? 4), durationMs = Number(query.get('duration') ?? 15000), autostart = query.has('events') && query.get('autostart') !== '0', autobench = query.has('autobench'), runId = query.get('run') ?? `${count}-${streams}`, [ready, setReady] = useState(false), [draft, setDraft] = useState(''), [running, setRunning] = useState(false), [benchStatus, setBenchStatus] = useState(''), [transportStatus, setTransportStatus] = useState('Disconnected'), [pairing, setPairing] = useState<PairingState>({ kind: 'loading' }), [pairingBusy, setPairingBusy] = useState(false), [enrollmentCode, setEnrollmentCode] = useState(''), [fallbackAcknowledged, setFallbackAcknowledged] = useState(false), [reconnect, setReconnect] = useState(0), [connectionPhase, setConnectionPhase] = useState<ConnectionPhase>('connecting'), [approvalInFlight, setApprovalInFlight] = useState<ReadonlySet<string>>(() => new Set()), [approvalError, setApprovalError] = useState<string | null>(null), [promptError, setPromptError] = useState<string | null>(null);
  const browserSession = useRef<BrowserSession | null>(null);
  const approvalFlightRef = useRef<ApprovalFlight>({ inFlight: new Set(), error: null });
  const pendingPromptRef = useRef<{ commandId: string; sessionId: string } | null>(null);
  const createFlightRef = useRef<CreateFlight>({ pendingCommandId: null, acceptedCommandId: null });
  const selectedCreateRef = useRef<string | null>(null);
  const [promptClear, setPromptClear] = useState<{ commandId: string; sessionId: string } | null>(null);
  const [sessionWorkdirsState, setSessionWorkdirs] = useState<readonly string[] | null>(null);
  const [createError, setCreateError] = useState<string | null>(null);
  const [createPending, setCreatePending] = useState(false);
  const [createdSessionId, setCreatedSessionId] = useState<string | null>(null);
  const [dismissedSessionId, setDismissedSessionId] = useState<string | null>(null);
  const publishApprovalFlight = (action: ApprovalFlightAction) => {
    const next = reduceApprovalFlight(approvalFlightRef.current, action);
    if (next === approvalFlightRef.current) return;
    approvalFlightRef.current = next;
    setApprovalInFlight(next.inFlight);
    setApprovalError(next.error);
  };
  // Readiness gates the generator until the external store has been initialized.
  // eslint-disable-next-line react-hooks/set-state-in-effect
  useEffect(() => { reset(browserMain ? 0 : count, streams, !b4Enabled && !browserMain); setDaemonAgentCount(streams); setReady(true); }, [count, streams]);
  useEffect(() => {
    if (b4Enabled) return;
    void pairingApi.inspect().then((info) => setPairing(stateFromInfo(info))).catch(() => setPairing({ kind: 'error', message: 'Could not read daemon identity. Check that the daemon is installed and retry.' }));
  }, []);
  useEffect(() => {
    if (pairing.kind !== 'confirming') return;
    const timer = window.setTimeout(() => setPairing({ kind: 'expired', info: pairing.info }), Math.max(0, pairing.expiresAt - performance.now()));
    return () => window.clearTimeout(timer);
  }, [pairing]);
  useEffect(() => {
    if (!browserMode || pairing.kind !== 'paired' || pairing.info.keyStorage) return;
    void pairingApi.inspect().then((info) => setPairing(stateFromInfo(info))).catch(() => undefined);
  }, [pairing]);
  useEffect(() => {
    if (!browserMain || !ready || pairing.kind !== 'paired') return;
    const info = pairing.info;
    const holder: { current: BrowserSession | null } = { current: null };
    let cancelled = false;
    void (async () => {
      try {
        const identity = await (await openBrowserKeyStore()).get(info.daemonPublicKey);
        if (cancelled) return;
        if (!identity || !info.clientId) throw new Error('browser key is missing; pair this browser again');
        const applyCreateFlight = (result: CreateFlightResult) => {
          createFlightRef.current = result.flight;
          setCreatePending(result.selectSessionId === null && (result.flight.pendingCommandId !== null || result.flight.acceptedCommandId !== null));
          if (result.selectSessionId) {
            selectedCreateRef.current = result.selectSessionId;
            setCreatedSessionId(result.selectSessionId);
          }
          if (result.createError !== undefined) setCreateError(result.createError);
          if (result.dismissSelection) {
            setDismissedSessionId(selectedCreateRef.current);
            selectedCreateRef.current = null;
            setCreatedSessionId(null);
          }
        };
        const session = createBrowserSession({
          connect: connectBrowser,
          url: browserWebSocketUrl(),
          identity,
          // A new page replays the journal so the session list is complete. In-page reconnect uses the high-water cursor.
          loadCursor: () => undefined,
          saveCursor: () => undefined,
          schedule: (delayMs, run) => {
            const id = window.setTimeout(run, delayMs);
            return () => window.clearTimeout(id);
          },
          online: () => navigator.onLine,
          onEpoch: (epoch) => { observeJournalEpoch(epoch); },
          onStatus: (phase) => {
            setConnectionPhase(phase);
            if (phase !== 'connected') publishApprovalFlight({ type: 'disconnect' });
          },
          onDurableEvent: (event) => {
            applyDaemonEvent(event.cursor, { session_id: event.sessionId, kind: event.kind, payload: event.payload, occurred_at_ms: event.occurredAtMs }, null);
            publishApprovalFlight({ type: 'journal', kind: event.kind, approvalId: approvalIdFromPayload(event.payload) });
            if (event.kind !== 'session_created') return;
            const commandId = event.payload && typeof event.payload === 'object' && 'command_id' in event.payload ? (event.payload as { command_id?: unknown }).command_id : null;
            applyCreateFlight(createFlightOnSessionCreated(createFlightRef.current, typeof commandId === 'string' ? commandId : null, event.sessionId));
          },
          onControl: (frame) => {
            const workdirs = sessionWorkdirs(frame);
            if (workdirs) { setSessionWorkdirs(workdirs); return; }
            const reduced = createFlightOnControl(createFlightRef.current, frame);
            applyCreateFlight(reduced);
            if (reduced.createError !== undefined) return;
            if (frame.kind === 'approval_resolved') publishApprovalFlight({ type: 'resolved', approvalId: frame.approval_id });
            if (frame.kind === 'command_accepted') {
              const pending = pendingPromptRef.current;
              if (clearPromptOnAccept(pending?.commandId ?? null, frame.command_id) && pending) {
                setPromptClear(pending);
                pendingPromptRef.current = null;
              }
              setPromptError(null);
            }
            const failure = controlFailureTarget(frame.kind, approvalFlightRef.current.inFlight.size);
            if (failure === 'approval') publishApprovalFlight({ type: 'rejected' });
            if (failure === 'prompt' && (frame.kind === 'error' || frame.kind === 'command_rejected')) {
              if (frame.kind === 'command_rejected' && pendingPromptRef.current?.commandId === frame.command_id) pendingPromptRef.current = null;
              setPromptError(frame.error);
            }
          },
        });
        if (cancelled) { session.stop(); return; }
        holder.current = session;
        browserSession.current = session;
      } catch (error) {
        if (!cancelled) setConnectionPhase(isKeyChangedError(error) ? 'key-changed' : 'reconnecting');
      }
    })();
    const onOnline = () => holder.current?.setOnline(navigator.onLine);
    const onOffline = () => holder.current?.setOnline(false);
    window.addEventListener('online', onOnline);
    window.addEventListener('offline', onOffline);
    return () => {
      cancelled = true;
      holder.current?.stop();
      if (browserSession.current === holder.current) browserSession.current = null;
      window.removeEventListener('online', onOnline);
      window.removeEventListener('offline', onOffline);
    };
  }, [ready, pairing, reconnect]);
  const runningRef = useRef(false);
  const rangeRequest = useRef<{ key: string; id: number; inFlight: boolean; pending?: { start: number; limit: number; noticeVersion?: number } }>({ key: '', id: 0, inFlight: false });
  const requestRange = useCallback((start: number, limit: number, noticeVersion?: number) => {
    if (!autobench) return;
    const state = rangeRequest.current;
    if (state.inFlight) {
      const pendingNoticeVersion = noticeVersion ?? state.pending?.noticeVersion;
      state.pending = pendingNoticeVersion === undefined ? { start, limit } : { start, limit, noticeVersion: pendingNoticeVersion };
      return;
    }
    const issue = (requestedStart: number, requestedLimit: number, requestedNoticeVersion?: number) => {
      const total = rowCount();
      const { start: from, limit: size } = rangeForWindow(total, requestedStart, Math.max(32, requestedLimit), isLiveTail());
      const key = `${from}:${size}:${total}`;
      if (state.key === key) return;
      state.key = key;
      state.inFlight = true;
      const id = ++state.id;
      void invoke<{ start: number; total: number; version: number; agentChunkCount: number; rows: { cursor: number; event: { session_id: string; kind: string; payload: unknown }; receipt_wall_clock_ms?: number | null }[] }>('get_stream_window', { start: from, limit: size }).then((view) => {
        setDaemonAgentChunkCount(view.agentChunkCount);
        if (id === state.id && !state.pending) setDaemonWindow(view.start, view.total, view.rows, view.version);
        else setDaemonTotal(view.total, false);
        return requestedNoticeVersion === undefined ? undefined : invoke('acknowledge_stream_view', { version: requestedNoticeVersion });
      }).catch((error: unknown) => { setTransportStatus(`Stream view failed: ${String(error)}`); b4Trace('window request failed', { error: String(error), start: from, limit: size }); }).finally(() => {
        state.inFlight = false;
        const next = state.pending;
        delete state.pending;
        if (next) { state.key = ''; issue(next.start, next.limit, next.noticeVersion); }
      });
    };
    issue(start, limit, noticeVersion);
  }, [autobench]);
  useEffect(() => {
    if (!ready || (b4Enabled && (!autobench || !query.has('b4'))) || (!b4Enabled && pairing.kind !== 'paired')) return;
    if (browserMain) return;
    if (browserMode) {
      let client: Awaited<ReturnType<typeof connectBrowser>> | undefined;
      let disposed = false;
      void (async () => {
        try {
          const info = await pairingApi.inspect();
          const identity = await (await openBrowserKeyStore()).get(info.daemonPublicKey);
          if (!identity || !info.clientId) throw new Error('browser key is missing; pair this browser again');
          const cursorKey = `shardrail-browser-cursor:${info.daemonPublicKey}`;
          let cursor: number | undefined;
          let journalEpoch: string | undefined;
          try {
            const saved = JSON.parse(localStorage.getItem(cursorKey) ?? 'null') as { cursor?: unknown; journalEpoch?: unknown } | null;
            if (saved && Number.isSafeInteger(saved.cursor) && typeof saved.journalEpoch === 'string') {
              cursor = saved.cursor as number;
              journalEpoch = saved.journalEpoch;
            }
          } catch { /* Ignore corrupt cursor state and replay from the beginning. */ }
          client = await connectBrowser({ url: browserWebSocketUrl(), identity, ...(cursor === undefined || journalEpoch === undefined ? {} : { cursor, journalEpoch }) });
          if (disposed) { client.close(); return; }
          client.onFrame((frame) => {
            if (frame.kind === 'durable_event') {
              applyDaemonEvent(frame.cursor, { session_id: frame.event.session_id, kind: frame.event.kind, payload: frame.event.payload }, frame.receipt_wall_clock_ms);
              try { localStorage.setItem(cursorKey, JSON.stringify({ cursor: frame.cursor, journalEpoch: client?.host.journal_epoch ?? '' })); } catch (error) { console.warn('Could not persist browser event cursor', error); }
            }
          });
          client.onError((error) => setTransportStatus(`Connection failed: ${error.message}`));
          setTransportStatus('Connected · browser WebSocket');
        } catch (error) {
          setTransportStatus(`Connection failed: ${String(error)}`);
          void pairingApi.inspect().then((info) => setPairing(stateFromInfo(info))).catch(() => undefined);
        }
      })();
      return () => { disposed = true; client?.close(); };
    }
    if (b4Enabled) {
      const offsets: number[] = [];
      for (let i = 0; i < 5; i++) { const before = performance.now(), wall = Date.now(), after = performance.now(); offsets.push(wall - (before + after) / 2); }
      setDaemonClockMapping(offsets.reduce((a, b) => a + b, 0) / offsets.length, (Math.max(...offsets) - Math.min(...offsets)) / 2 + 1);
    }
    let unlistenStatus: (() => void) | undefined;
    const channel = new Channel<{ version: number; total: number; agentChunkCount: number }>();
    const removeChannel = coalesceChannelFrame(channel, (notice) => {
      const { total, agentChunkCount } = notice;
      setDaemonTotal(total, false);
      setDaemonAgentChunkCount(agentChunkCount);
      if (isLiveTail()) {
        rangeRequest.current.key = '';
        const tailSize = liveTailWindowSize(b4Enabled);
        requestRange(Math.max(0, total - tailSize), tailSize, notice.version);
      } else {
        setDaemonTotal(total);
        void invoke('acknowledge_stream_view', { version: notice.version }).catch((error: unknown) => setTransportStatus(`Stream acknowledgement failed: ${String(error)}`));
      }
    });
    void listen<{ connected: boolean; error?: string }>('daemon-status', (event) => {
      const status = event.payload;
      b4Trace('daemon status received', status);
      (window as Window & { __daemonConnected?: boolean }).__daemonConnected = status.connected;
      if (status.connected) (window as Window & { __daemonConnectedOnce?: boolean }).__daemonConnectedOnce = true;
      setTransportStatus(status.connected ? 'Connected · local shell relay' : status.error ? `Connection failed: ${status.error}` : 'Disconnected');
    }).then((remove) => { unlistenStatus = remove; setTransportStatus('Frontend channel ready'); b4Trace('ready', { channel: 'daemon-frames', events: count });
      return invoke(b4Enabled ? 'start_b4_relay' : 'start_paired_relay', { onFrames: channel, streams: Math.max(1, Math.min(8, streams)) });
    }).catch((error: unknown) => {
      if (b4Enabled) {
        setTransportStatus(`Connection failed: ${String(error)}`);
        b4Trace('relay setup rejected', { error: String(error) });
        console.error('S5 channel setup failed', error);
      } else {
        setTransportStatus('Connection failed · check daemon and pairing');
        void pairingApi.inspect().then((info) => setPairing(stateFromInfo(info))).catch(() => undefined);
      }
    });
    return () => { unlistenStatus?.(); removeChannel(); };
  }, [autobench, count, pairing.kind, query, ready, reconnect, requestRange, streams]);
  useLayoutEffect(() => {
    const input = takeCommittedInput(pendingInputs, draft);
    if (!input) return;
    requestAnimationFrame((frameTime) => {
      const preview = document.querySelector<HTMLElement>('.draft-preview');
      const callbackTime = performance.now();
      const firstFrame = matchedInFirstFrame(input.startedAt, frameTime, callbackTime, preview?.dataset.draft === input.draft, nominalFrameIntervalMs);
      observed.inputs.push(firstFrame.delay);
      observed.firstFrame.push(firstFrame.matched);
      observed.firstFrameTimestamps.push(firstFrame.delay);
      observed.rafTimestampDeltas.push(firstFrame.frameTimestampDelta);
    });
  }, [draft]);
  useEffect(() => { if (!autostart || !ready) return; const id = setTimeout(() => startStreams(streams, durationMs, runningRef, setRunning), 1000); return () => clearTimeout(id); }, [ready, autostart, streams, durationMs]);
  useEffect(() => {
    b4Trace('automation effect evaluated', { ready, autobench, visibility: document.visibilityState, focused: document.hasFocus() });
    if (!ready || !autobench) return;
    let cancelled = false;
    void (async () => {
      setBenchStatus('Automated run started');
      b4Trace('automation started', { visibility: document.visibilityState, focused: document.hasFocus() });
      if (!query.has('b4')) {
        await new Promise((resolve) => setTimeout(resolve, autostart ? 2200 : 600));
        b4Trace('automation delay complete', { visibility: document.visibilityState, focused: document.hasFocus() });
        const field = document.querySelector<HTMLInputElement>('input[aria-label="Message composer"]');
        if (!field) throw new Error('Composer was not rendered');
        field.focus();
        const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set;
        for (const char of '/s5-paint-responsiveness-trace') {
          setter?.call(field, field.value + char);
          field.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: char }));
          await new Promise((resolve) => setTimeout(resolve, 40));
        }
        await new Promise((resolve) => setTimeout(resolve, 100));
        b4Trace('automation input complete');
      } else {
        b4Trace('B4 automation input skipped', { reason: 'synthetic composer input is outside the daemon-to-row measurement' });
      }
      if (autobench && shouldWaitForDaemonStream(query, count)) {
        setBenchStatus('Waiting for shell relay and daemon fake-agent stream');
        b4Trace('completion wait started', { expectedAgentChunks: count, connectedOnce: Boolean((window as Window & { __daemonConnectedOnce?: boolean }).__daemonConnectedOnce) });
        await waitForDaemonStream(count, () => daemonAgentChunkCount, () => Boolean((window as Window & { __daemonConnectedOnce?: boolean }).__daemonConnectedOnce));
        const minimumSamples = minimumB4Samples(count);
        b4Trace('sample wait started', { expectedSamples: minimumSamples });
        await waitForB4Samples(minimumSamples, () => daemonReceiptToRafMs.length);
      }
      b4Trace('completion condition evaluated', { expectedAgentChunks: count, frontendAgentChunks: daemonAgentChunkCount, expectedSamples: minimumB4Samples(count), samples: daemonReceiptToRafMs.length, connectedOnce: Boolean((window as Window & { __daemonConnectedOnce?: boolean }).__daemonConnectedOnce), rows: eventCount() });
      const el = document.querySelector<HTMLElement>('.transcript')!;
      if (!query.has('b4')) {
        startFrameSampling();
        for (let step = 0; step < 60; step++) {
          el.scrollTop = (el.scrollTop + 14) % Math.max(1, el.scrollHeight - el.clientHeight);
          await new Promise((resolve) => setTimeout(resolve, 16));
        }
      }
      if (autostart) await new Promise<void>((resolve) => {
        const waitForStreams = () => document.querySelector('footer')?.textContent?.includes('Streams running') ? window.setTimeout(waitForStreams, 50) : resolve();
        waitForStreams();
      });
      stopFrameSampling();
      if (cancelled) return;
      b4Trace('rows committed', { rows: eventCount() });
      const samples = metrics();
      b4Trace('samples collected', { count: samples.b4.count, daemonAgentChunkCount: samples.daemonAgentChunkCount });
      const requiredTokens = Math.ceil(streams * 75 * durationMs / 1000 * 0.5);
      if (autostart && samples.generatedTokens < requiredTokens) throw new Error(`Generated stream load ${samples.generatedTokens} < required ${requiredTokens}`);
      const scrollFrames = [...observed.frames];
      const result = { date: new Date().toISOString(), events: count, temperature: query.get('temperature') ?? 'unspecified', streams, pageVisibility: document.visibilityState, pageFocused: document.hasFocus(), inputMethod: query.has('b4') ? 'not used for B4 daemon stream measurement' : 'Synthetic InputEvent dispatched to the focused composer', ...samples, scrollSmoothness: { frameP95Ms: percentile(scrollFrames, .95), frameMaxMs: Math.max(...scrollFrames), frames: scrollFrames.length } };
      setBenchStatus('Saving results');
      b4Trace('report fetch started', { url: 'http://127.0.0.1:5174/result', bodyBytes: JSON.stringify({ filename: runId, result }).length });
      let response: Response;
      try {
        response = await fetch('http://127.0.0.1:5174/result', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ filename: runId, result }) });
        b4Trace('report fetch resolved', { status: response.status, ok: response.ok });
      } catch (error) {
        b4Trace('report fetch rejected', { error: String(error) });
        throw error;
      }
      if (!response.ok) throw new Error(`Result receiver returned HTTP ${response.status}`);
      setBenchStatus('Results saved');
    })().catch(async (error: unknown) => {
      setBenchStatus(`Run failed: ${String(error)}`);
      b4Trace('bench rejected', { error: String(error), frontendAgentChunks: daemonAgentChunkCount, samples: daemonReceiptToRafMs.length });
      console.error('S5 automated run failed', error);
      if (autobench) await fetch('http://127.0.0.1:5174/result', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ filename: runId, result: { error: String(error), events: count, streams, daemonAgentChunkCount, b4: { count: daemonReceiptToRafMs.length } } }),
      }).catch(() => undefined);
    });
    return () => { cancelled = true; };
  }, [ready, autostart, autobench, durationMs, count, streams, runId, query]);
  function profiler(_: string, __: string, duration: number) { observed.commits++; observed.commitMs += duration; }
  function handleInput(event: React.ChangeEvent<HTMLInputElement>) {
    const value = event.currentTarget.value;
    pendingInputs.push({ startedAt: performance.now(), draft: value });
    setDraft(value);
  }
  function updateQuery(key: string, value: string) { const u = new URL(location.href); u.searchParams.set(key, value); window.location.assign(u.toString()); }
  const select = (label: string, value: string, options: string[], onChange: (value: string) => void) => <Select aria-label={label} selectedKey={value} onSelectionChange={(key) => onChange(String(key))}><Label>{label}</Label><Button><SelectValue/><span aria-hidden="true">⌄</span></Button><Popover><ListBox>{options.map((option) => <ListBoxItem key={option} id={option}>{option}</ListBoxItem>)}</ListBox></Popover></Select>;
  if (!b4Enabled && pairing.kind !== 'paired') {
    const info = 'info' in pairing ? pairing.info : undefined;
    const verify = () => { if (info) { setFallbackAcknowledged(false); setPairing(beginConfirmation(info)); } };
    const refresh = () => { setPairing({ kind: 'loading' }); void pairingApi.inspect().then((next) => setPairing(stateFromInfo(next))).catch(() => setPairing({ kind: 'error', message: 'Could not read daemon identity. Check that the daemon is available and retry.' })); };
    return <PairingScreen state={pairing} browserMode={browserMode} busy={pairingBusy} enrollmentCode={enrollmentCode} fallbackAcknowledged={fallbackAcknowledged} onCode={setEnrollmentCode} onAcknowledge={setFallbackAcknowledged} onReview={verify} onRetry={refresh} onPair={() => { setPairingBusy(true); void confirmPairing(pairing, pairingApi, undefined, enrollmentCode, fallbackAcknowledged).then(setPairing).finally(() => setPairingBusy(false)); }} onCancel={() => { setEnrollmentCode(''); setFallbackAcknowledged(false); if (info) setPairing(stateFromInfo(info)); }}/>
  }
  if (browserMain && pairing.kind === 'paired') {
    const resolveApproval = (approvalId: string, optionId: string, binding: ApprovalBindingEcho) => {
      const result = browserSession.current?.resolveApproval(approvalId, optionId, binding) ?? 'blocked';
      publishApprovalFlight(result === 'sent' ? { type: 'sent', approvalId } : { type: 'blocked' });
    };
    const sendCommand = (kind: 'turn.start' | 'turn.interrupt', payload: Record<string, unknown>, blocked: string) => {
      const commandId = crypto.randomUUID();
      const result = browserSession.current?.request({
        command_id: commandId,
        kind,
        payload,
        expected_state: null,
        expected_turn_id: null,
      }) ?? 'blocked';
      if (kind === 'turn.start' && result === 'sent' && typeof payload.session_id === 'string') {
        pendingPromptRef.current = { commandId, sessionId: payload.session_id };
      }
      setPromptError(result === 'sent' ? null : blocked);
    };
    const keyStorage = pairing.info.keyStorage;
    const createSession = (workdir: string) => {
      const commandId = crypto.randomUUID();
      const result = browserSession.current?.request({
        command_id: commandId,
        kind: 'session.create',
        payload: { workdir },
        expected_state: null,
        expected_turn_id: null,
      }) ?? 'blocked';
      if (result !== 'sent') { setCreatePending(false); setCreateError('Not connected. The session was not created.'); return; }
      createFlightRef.current = noteCreateSent(commandId);
      setCreatePending(true);
      setCreateError(null);
    };
    return <MainScreen phase={connectionPhase} pairedFingerprint={pairing.info.fingerprint} {...(keyStorage ? { keyStorage } : {})} approvalInFlight={approvalInFlight} approvalError={approvalError} promptError={promptError} promptClear={promptClear} workdirs={sessionWorkdirsState} createError={createError} createPending={createPending} createdSessionId={createdSessionId} dismissedSessionId={dismissedSessionId} onReconnect={() => { if (browserSession.current) browserSession.current.reconnect(); else setReconnect((current) => current + 1); }} onResolveApproval={resolveApproval} onSendPrompt={(sessionId, prompt) => sendCommand('turn.start', { session_id: sessionId, prompt }, 'Not connected. The prompt was not sent.')} onInterrupt={(sessionId) => sendCommand('turn.interrupt', { session_id: sessionId }, 'Not connected. Interrupt was not sent.')} onCreateSession={createSession}/>;
  }
  return <main><header><div><h1>Shardrail · S5 UI load</h1><p>Mock content · no provider connections</p></div><div className="controls">{pairing.kind === 'paired' ? <><span className="paired-fingerprint" aria-label="Paired daemon fingerprint" title={pairing.info.fingerprint}>{pairing.info.fingerprint}</span><Button onPress={() => setReconnect((current) => current + 1)}>Reconnect</Button>{!browserMode ? <Button onPress={() => { void unpairClient(pairingApi).then(setPairing); }}>Unpair</Button> : null}</> : null}{select('Events', String(count), ['0', '5000', '50000'], (value) => updateQuery('events', value))}{select('Streams', String(streams), ['1', '4', '8'], (value) => updateQuery('streams', value))}<Button onPress={() => startStreams(streams, durationMs, runningRef, setRunning)}>Start mock streams</Button></div></header>{browserMode && pairing.kind === 'paired' && pairing.info.keyStorage === 'noble-fallback' ? <p role="note">This browser stores an extractable private key seed in IndexedDB because non-extractable WebCrypto is unavailable. Same-origin script can read it.</p> : null}<section className="status"><span>{ready ? `${eventCount().toLocaleString()} events loaded · ${streams} streams` : 'Loading'}</span><span>{transportStatus} · Metrics: window.__s5Metrics()</span></section><TextField className="composer"><Label>Message</Label><Input aria-label="Message composer" value={draft} placeholder="Type to measure input responsiveness" onChange={handleInput}/><output className="draft-preview" data-draft={draft} aria-live="polite">{draftPreview(draft)}</output></TextField><Profiler id="transcript" onRender={profiler}><Transcript ready={ready} onInput={() => {}} requestRange={requestRange}/></Profiler><footer>{benchStatus || 'Mock content only'} · {running ? 'Streams running' : 'Ready'}</footer></main>;
}
createRoot(document.getElementById('root')!).render(<App/>);
