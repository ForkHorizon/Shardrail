import React, { Profiler, memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { emit, listen } from '@tauri-apps/api/event';
import { listenAndAnnounceReady } from './tauri-bridge';
import type { DaemonFrame } from '@shardrail/contracts';
import { Button, Input, Label, ListBox, ListBoxItem, Popover, Select, SelectValue, TextField } from 'react-aria-components';
import { useVirtualizer } from '@tanstack/react-virtual';
import { createPortal } from 'react-dom';
import { applyDaemonEvent, daemonAgentChunkCount, daemonClockMapping, daemonReceiptToRafMs, eventCount, getDaemonAgentChunkCount, getRows, markDaemonRowsCommitted, markRowsCommitted, pushToken, reset, setDaemonAgentCount, setDaemonClockMapping, sortedLatency, tokenCount, useRows } from './store';
import { matchedInFirstFrame, measureReadyCommitAfterFrame, nominalFrameIntervalMs, percentile, supportsLongTaskEntries, takeCommittedInput } from './metrics-utils';
import { createFrameSampler } from './frame-sampler';
import { daemonStreamFinished, draftPreview, shouldWaitForDaemonStream } from './bench-utils';
import { UntrustedMarkdown } from './UntrustedMarkdown';
import './style.css';

type Metrics = { firstTranscriptRenderOpportunityMs: number; longTasks: number | null; maxLongTaskMs: number | null; frameP95: number | null; frameMax: number | null; frameSamples: number; commitCount: number; commitMs: number; eventStoreCount: number; generatedTokens: number; heapMB: number | null; mockCreationToMountedRowFollowupP95: number | null; composerP95: number | null; composerMin: number | null; composerMax: number | null; composerSamples: number; matchesByFirstFrame: number; firstFrameSamples: number; firstFrameDeltasMs: number[]; firstFrameRafTimestampDeltasMs: number[]; b4: { count: number; medianMs: number | null; p95Ms: number | null; maxMs: number | null; samplesMs: number[]; clockOffsetMs: number; clockUncertaintyMs: number }; daemonAgentChunkCount: number; userAgent: string };
const started = performance.now();
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

const Markdown = memo(({ source }: { source: string }) => {
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
});
function Row({ row }: { row: ReturnType<typeof getRows>[number] }) {
  return <article data-daemon-event-id={row.id >= 1_000_000_000 ? row.id : undefined} className={`event ${row.kind}`}><div className="meta"><b>Agent {row.agent + 1}</b><span>{row.kind === 'approval' ? 'approval requested' : row.kind === 'tool' ? 'tool call' : 'message'} · #{row.id}</span><time>{new Date().toLocaleTimeString()}</time></div>{row.kind === 'approval' ? <><pre>{row.command}</pre><div className="untrusted"><b>Agent’s description · untrusted</b><Markdown source={row.text}/></div><small>Checklist: sandbox not verified · approval routing not verified · containment not verified</small></> : row.kind === 'tool' ? <><pre>{row.command}</pre><small>{row.cwd}</small></> : <div className="body"><Markdown source={row.text}/></div>}</article>;
}
function Transcript({ onInput, ready }: { onInput: () => void; ready: boolean }) {
  const rows = useRows();
  const parent = useRef<HTMLDivElement>(null);
  const followTail = useRef(true);
  const firstPaintMeasured = useRef(false);
  // TanStack Virtual returns imperative methods that React Compiler cannot safely memoize.
  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({ count: rows.length, getScrollElement: () => parent.current, estimateSize: () => 112, overscan: 8 });
  const virtualItems = virtualizer.getVirtualItems();
  useLayoutEffect(() => {
    markRowsCommitted(virtualItems.map((item) => rows[item.index]!.id));
    markDaemonRowsCommitted(
      virtualItems.map((item) => rows[item.index]!.id),
      requestAnimationFrame,
      () => performance.now(),
      (id) => followTail.current && Boolean(document.querySelector(`[data-daemon-event-id="${id}"]`)),
    );
    if (followTail.current && parent.current) parent.current.scrollTop = parent.current.scrollHeight;
    if (ready && !firstPaintMeasured.current) {
      firstPaintMeasured.current = true;
      measureReadyCommitAfterFrame(true, started, () => performance.now(), requestAnimationFrame, (callback) => window.setTimeout(callback, 0), (elapsed) => { (window as Window & { __firstTranscriptRenderOpportunityMs?: number }).__firstTranscriptRenderOpportunityMs = elapsed; });
    }
  }, [rows, virtualItems, ready]);
  // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- scrollable transcript needs keyboard focus
  return <><div ref={parent} className="transcript" role="region" aria-label="Event transcript" tabIndex={0} onScroll={() => { followTail.current = parent.current !== null && parent.current.scrollHeight - parent.current.scrollTop - parent.current.clientHeight < 24; onInput(); }}><div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>{virtualItems.map((v) => <div key={rows[v.index]!.id} ref={virtualizer.measureElement} data-index={v.index} style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${v.start}px)` }}><Row row={rows[v.index]!}/></div>)}</div></div></>;
}
function App() {
  const query = useMemo(() => new URLSearchParams(location.search), []), count = Math.max(0, Number(query.get('events') ?? 5000)), streams = Number(query.get('streams') ?? 4), durationMs = Number(query.get('duration') ?? 15000), autostart = query.has('events') && query.get('autostart') !== '0', autobench = query.has('autobench'), runId = query.get('run') ?? `${count}-${streams}`, [ready, setReady] = useState(false), [draft, setDraft] = useState(''), [running, setRunning] = useState(false), [benchStatus, setBenchStatus] = useState(''), [transportStatus, setTransportStatus] = useState('Disconnected');
  // Readiness gates the generator until the external store has been initialized.
  // eslint-disable-next-line react-hooks/set-state-in-effect
  useEffect(() => { reset(count, streams); setDaemonAgentCount(streams); setReady(true); }, [count, streams]);
  const runningRef = useRef(false);
  useEffect(() => {
    if (!autobench) return;
    const offsets: number[] = [];
    for (let i = 0; i < 5; i++) { const before = performance.now(), wall = Date.now(), after = performance.now(); offsets.push(wall - (before + after) / 2); }
    setDaemonClockMapping(offsets.reduce((a, b) => a + b, 0) / offsets.length, (Math.max(...offsets) - Math.min(...offsets)) / 2 + 1);
    const unlisten: (() => void)[] = [];
    void listenAndAnnounceReady({ listen, emit }, (event: { payload: DaemonFrame[] }) => {
      for (const frame of event.payload) {
        if (frame.kind === 'durable_event') applyDaemonEvent(frame.cursor, frame.event, frame.receipt_wall_clock_ms);
      }
    }, (event: { payload: { connected: boolean; error?: string } }) => {
      const status = event.payload;
      (window as Window & { __daemonConnected?: boolean }).__daemonConnected = status.connected;
      if (status.connected) (window as Window & { __daemonConnectedOnce?: boolean }).__daemonConnectedOnce = true;
      setTransportStatus(status.connected ? 'Connected · local shell relay' : status.error ? `Connection failed: ${status.error}` : 'Disconnected');
    }).then((remove) => { unlisten.push(remove); setTransportStatus('Frontend listeners ready'); }).catch((error: unknown) => { setTransportStatus(`Connection failed: ${String(error)}`); console.error('S5 event listener setup failed', error); });
    return () => { unlisten.forEach((remove) => remove()); };
  }, [autobench]);
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
    if (!ready || !autobench) return;
    let cancelled = false;
    void (async () => {
      setBenchStatus('Automated run started');
      await new Promise((resolve) => setTimeout(resolve, autostart ? 2200 : 600));
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
      if (autobench && shouldWaitForDaemonStream(query, count)) {
        setBenchStatus('Waiting for shell relay and daemon fake-agent stream');
        const deadline = performance.now() + 55_000;
        while (getDaemonAgentChunkCount() < count && performance.now() < deadline) {
          const relayWindow = window as Window & { __daemonConnected?: boolean; __daemonConnectedOnce?: boolean };
          if (relayWindow.__daemonConnectedOnce && relayWindow.__daemonConnected === false) break;
          await new Promise((resolve) => setTimeout(resolve, 50));
        }
        const connectedOnce = Boolean((window as Window & { __daemonConnectedOnce?: boolean }).__daemonConnectedOnce);
        if (!daemonStreamFinished(connectedOnce, getDaemonAgentChunkCount(), count)) throw new Error(`Relay/stream incomplete: connectedOnce=${connectedOnce}, chunks=${getDaemonAgentChunkCount()}/${count}`);
      }
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
      const samples = metrics();
      const requiredTokens = Math.ceil(streams * 75 * durationMs / 1000 * 0.5);
      if (autostart && samples.generatedTokens < requiredTokens) throw new Error(`Generated stream load ${samples.generatedTokens} < required ${requiredTokens}`);
      const scrollFrames = [...observed.frames];
      const result = { date: new Date().toISOString(), temperature: query.get('temperature') ?? 'unspecified', streams, inputMethod: 'Synthetic InputEvent dispatched to the focused composer', ...samples, scrollSmoothness: { frameP95Ms: percentile(scrollFrames, .95), frameMaxMs: Math.max(...scrollFrames), frames: scrollFrames.length } };
      setBenchStatus('Saving results');
      const response = await fetch('http://127.0.0.1:5174/result', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ filename: runId, result }) });
      if (!response.ok) throw new Error(`Result receiver returned HTTP ${response.status}`);
      setBenchStatus('Results saved');
    })().catch(async (error: unknown) => {
      setBenchStatus(`Run failed: ${String(error)}`);
      console.error('S5 automated run failed', error);
      if (autobench) await fetch('http://127.0.0.1:5174/result', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ filename: runId, result: { error: String(error), daemonAgentChunkCount, b4: { count: daemonReceiptToRafMs.length } } }),
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
  return <main><header><div><h1>Shardrail · S5 UI load</h1><p>Mock content · no provider connections</p></div><div className="controls">{select('Events', String(count), ['0', '5000', '50000'], (value) => updateQuery('events', value))}{select('Streams', String(streams), ['1', '4', '8'], (value) => updateQuery('streams', value))}<Button onPress={() => startStreams(streams, durationMs, runningRef, setRunning)}>Start mock streams</Button></div></header><section className="status"><span>{ready ? `${getRows().length.toLocaleString()} events loaded · ${streams} streams` : 'Loading'}</span><span>{transportStatus} · Metrics: window.__s5Metrics()</span></section><TextField className="composer"><Label>Message</Label><Input aria-label="Message composer" value={draft} placeholder="Type to measure input responsiveness" onChange={handleInput}/><output className="draft-preview" data-draft={draft} aria-live="polite">{draftPreview(draft)}</output></TextField><Profiler id="transcript" onRender={profiler}><Transcript ready={ready} onInput={() => {}}/></Profiler><footer>{benchStatus || 'Mock content only'} · {running ? 'Streams running' : 'Ready'}</footer></main>;
}
createRoot(document.getElementById('root')!).render(<App/>);
