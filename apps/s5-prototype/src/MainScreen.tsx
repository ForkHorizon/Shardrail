import { useLayoutEffect, useMemo, useRef, useState } from 'react';
import { Button, ListBox, ListBoxItem } from 'react-aria-components';
import { useVirtualizer } from '@tanstack/react-virtual';
import { activityTimestamp, approvalsForSession, checklistSummary, eventsForSession, promptClearApplies, revealHiddenCharacters, sessionSummaries, transcriptItems, type TranscriptItem } from './session-model';
import { connectionStatusLabel, type ConnectionPhase } from './browser-session';
import { isNearLiveTail } from './streaming-view';
import { useSessionJournalEvents } from './store';
import { UntrustedMarkdown } from './UntrustedMarkdown';

const checklist = checklistSummary();

export function MainScreen({ phase, keyStorage, approvalInFlight, approvalError, promptError, promptClear, workdirs, createError, createdSessionId, onReconnect, onResolveApproval, onSendPrompt, onInterrupt, onCreateSession }: {
  phase: ConnectionPhase;
  keyStorage?: 'webcrypto' | 'noble-fallback';
  approvalInFlight: ReadonlySet<string>;
  approvalError: string | null;
  promptError: string | null;
  promptClear: { commandId: string; sessionId: string } | null;
  workdirs: readonly string[] | null;
  createError: string | null;
  createdSessionId: string | null;
  onReconnect: () => void;
  onResolveApproval: (approvalId: string, optionId: string) => void;
  onSendPrompt: (sessionId: string, prompt: string) => void;
  onInterrupt: (sessionId: string) => void;
  onCreateSession: (workdir: string) => void;
}) {
  const events = useSessionJournalEvents();
  const sessions = useMemo(() => sessionSummaries(events), [events]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [appliedCreateId, setAppliedCreateId] = useState<string | null>(null);
  if (createdSessionId && createdSessionId !== appliedCreateId) {
    setAppliedCreateId(createdSessionId);
    setSelectedId(createdSessionId);
  }
  const [pickedWorkdir, setPickedWorkdir] = useState('');
  const selectedWorkdir = workdirs?.includes(pickedWorkdir) ? pickedWorkdir : (workdirs?.[0] ?? '');
  const [confirmingAllowId, setConfirmingAllowId] = useState<string | null>(null);
  const selected = sessions.find((session) => session.id === selectedId) ?? null;
  const visible = useMemo(() => selected ? eventsForSession(events, selected.id) : [], [events, selected]);
  const items = useMemo(() => transcriptItems(visible), [visible]);
  const approvals = useMemo(() => selected ? approvalsForSession(events, selected.id) : [], [events, selected]);
  return <main className="main-screen">
    <header>
      <div><h1>Shardrail</h1><p>Local sessions from the daemon journal</p></div>
      <div className="controls">
        <p role={phase === 'key-changed' ? 'alert' : 'status'}>{connectionStatusLabel(phase)}</p>
        <Button onPress={onReconnect}>Reconnect</Button>
      </div>
    </header>
    <p className="host-banner" role="status">approvals not protected. Sandbox, approval routing, containment, and the other session checks are not verified.</p>
    {keyStorage === 'noble-fallback' ? <p role="note">This browser stores an extractable private key seed in IndexedDB because non-extractable WebCrypto is unavailable. Same-origin script can read it.</p> : null}
    <div className="workspace">
      <section className="session-column" aria-labelledby="sessions-heading">
        <h2 id="sessions-heading">Sessions</h2>
        {workdirs !== null ? <section className="new-session" aria-label="New session">
          <label htmlFor="session-workdir">Workdir</label>
          <select id="session-workdir" value={selectedWorkdir} onChange={(event) => setPickedWorkdir(event.currentTarget.value)}>
            {workdirs.map((dir) => <option key={dir} value={dir}>{dir}</option>)}
          </select>
          <Button isDisabled={selectedWorkdir.length === 0} onPress={() => onCreateSession(selectedWorkdir)}>New session</Button>
          {createError ? <p role="alert">{createError}</p> : null}
        </section> : null}
        {sessions.length === 0 ? <p>No sessions yet.</p> : <ListBox className="session-list" aria-label="Sessions" selectionMode="single" selectedKeys={selected ? [selected.id] : []} onSelectionChange={(keys) => { if (keys === 'all') return; const next = [...keys][0]; setSelectedId(next == null ? null : String(next)); setConfirmingAllowId(null); }}>
          {sessions.map((session) => <ListBoxItem key={session.id} id={session.id} textValue={`${session.id} ${session.agentKind ?? 'agent unknown'} ${session.status} ${checklist.unverified} not verified`} onFocus={() => { setSelectedId(session.id); setConfirmingAllowId(null); }}>
            <span className="session-id">{session.id}</span>
            <span>{session.agentKind ?? 'agent unknown'}</span>
            <span>{session.status}</span>
            <span>{checklist.unverified} checks not verified</span>
            <ActivityTime ms={session.lastActivityMs}/>
          </ListBoxItem>)}
        </ListBox>}
      </section>
      <section className="session-detail" aria-labelledby="events-heading">
        <h2 id="events-heading">{selected ? `Events for ${selected.id}` : 'Events'}</h2>
        {selected ? <SessionTranscript key={selected.id} sessionId={selected.id} items={items}/> : <p>Select a session.</p>}
        {/* The prompt key has to differ from the transcript key. Two siblings with the same key leave the previous transcript mounted. */}
        {selected ? <PromptBox key={`prompt-${selected.id}-${promptClear && promptClearApplies(promptClear.sessionId, selected.id) ? promptClear.commandId : ''}`} sessionId={selected.id} error={promptError} onSend={onSendPrompt} onInterrupt={onInterrupt}/> : null}
        {selected ? <VerificationChecklist/> : null}
        {selected ? <Approvals cards={approvals} inFlight={approvalInFlight} error={approvalError} confirmingAllowId={confirmingAllowId} onConfirmAllow={setConfirmingAllowId} onResolve={onResolveApproval}/> : null}
      </section>
    </div>
  </main>;
}

function ActivityTime({ ms }: { ms: number }) {
  const iso = activityTimestamp(ms);
  if (!iso) return <time>no activity yet</time>;
  return <time dateTime={iso}>{new Date(ms).toLocaleString()}</time>;
}

function SessionTranscript({ sessionId, items }: { sessionId: string; items: readonly TranscriptItem[] }) {
  const parent = useRef<HTMLDivElement>(null);
  const followTail = useRef(true);
  // TanStack Virtual returns imperative methods that React Compiler cannot safely memoize.
  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({ count: items.length, getScrollElement: () => parent.current, estimateSize: () => 72, overscan: 8, measureElement: (element) => element.getBoundingClientRect().height });
  const virtualizerRef = useRef(virtualizer);
  virtualizerRef.current = virtualizer;
  const virtualItems = virtualizer.getVirtualItems();
  useLayoutEffect(() => {
    if (followTail.current && items.length > 0) virtualizerRef.current.scrollToIndex(items.length - 1, { align: 'end' });
  }, [sessionId, items.length]);
  // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- scrollable transcript needs keyboard focus
  return <div ref={parent} className="transcript" role="region" aria-label="Session events" tabIndex={0} onScroll={() => { const el = parent.current; if (!el) return; followTail.current = isNearLiveTail(el.scrollHeight, el.scrollTop, el.clientHeight); }}>
    {items.length === 0 ? <p>No events yet.</p> : <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>{virtualItems.map((item) => {
      const event = items[item.index];
      return event ? <div key={event.key} data-index={item.index} ref={virtualizer.measureElement} style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${item.start}px)` }}><SessionEvent event={event}/></div> : null;
    })}</div>}
  </div>;
}

function SessionEvent({ event }: { event: TranscriptItem }) {
  const iso = activityTimestamp(event.occurredAtMs);
  return <article className="event">
    <div className="meta"><b>{event.kind}</b><span>{event.cursorLabel}</span>{iso ? <time dateTime={iso}>{new Date(event.occurredAtMs).toLocaleTimeString()}</time> : null}</div>
    {event.markdown !== null ? <div className="body"><UntrustedMarkdown source={event.markdown}/></div> : <pre className="body">{event.plain}</pre>}
  </article>;
}

function PromptBox({ sessionId, error, onSend, onInterrupt }: {
  sessionId: string;
  error: string | null;
  onSend: (sessionId: string, prompt: string) => void;
  onInterrupt: (sessionId: string) => void;
}) {
  const [text, setText] = useState('');
  return <section className="prompt" aria-labelledby="prompt-heading">
    <h2 id="prompt-heading">Prompt</h2>
    {error ? <p role="alert">{error}</p> : null}
    <label htmlFor="session-prompt">Prompt</label>
    <textarea id="session-prompt" value={text} onChange={(event) => setText(event.currentTarget.value)}/>
    <div className="controls">
      <Button isDisabled={text.trim() === ''} onPress={() => { const prompt = text.trim(); if (!prompt) return; onSend(sessionId, prompt); }}>Send prompt</Button>
      <Button onPress={() => onInterrupt(sessionId)}>Interrupt</Button>
    </div>
  </section>;
}

function VerificationChecklist() {
  return <section className="checklist-section" aria-labelledby="checklist-heading">
    <h2 id="checklist-heading" tabIndex={-1}>Verification checklist</h2>
    <p>{checklist.unverified} not verified. Items are not combined into one verdict.</p>
    <ul className="checklist">
      {checklist.checks.map((item) => <li key={item.id}>
        <span>{item.label}</span>
        <span>{item.status}</span>
        <span>{item.source}. Last checked: never.</span>
      </li>)}
    </ul>
  </section>;
}

function Approvals({ cards, inFlight, error, confirmingAllowId, onConfirmAllow, onResolve }: {
  cards: ReturnType<typeof approvalsForSession>;
  inFlight: ReadonlySet<string>;
  error: string | null;
  confirmingAllowId: string | null;
  onConfirmAllow: (approvalId: string | null) => void;
  onResolve: (approvalId: string, optionId: string) => void;
}) {
  return <section className="approvals" aria-labelledby="approvals-heading">
    <h2 id="approvals-heading">Approvals</h2>
    {error ? <p role="alert">{error}</p> : null}
    {cards.length === 0 ? <p>No approvals for this session.</p> : null}
    {cards.map((card) => {
      const confirming = confirmingAllowId === card.approvalId && card.state === 'pending' && !card.actionDisclosed;
      const busy = inFlight.has(card.approvalId);
      return <article key={card.approvalId} aria-label={`Approval ${card.approvalId}`}>
        {card.actionDisclosed ? <DisclosedAction card={card}/> : <p className="caution">action not disclosed by provider</p>}
        <p>Tool {card.toolKind}</p>
        <p>Sandbox: {card.sandbox ? revealHiddenCharacters(card.sandbox) : 'not disclosed'}</p>
        <p>Approval axis: once</p>
        {card.toolCallId ? <p>Tool call id <span className="session-id">{card.toolCallId}</span></p> : null}
        <p>{card.state}{card.decision ? ` · ${card.decision}` : ''}</p>
        <p>Checklist: {checklist.failed} failed, {checklist.unverified} not verified.</p>
        <Button onPress={() => document.getElementById('checklist-heading')?.focus()}>Open the full checklist</Button>
        {card.agentTitle || card.agentDescription ? <div className="untrusted"><b>Agent’s description · untrusted</b><UntrustedMarkdown source={[card.agentTitle, card.agentDescription].filter(Boolean).join('\n\n')}/></div> : null}
        {card.state === 'pending' ? <div className="controls">
          <Button isDisabled={busy || !card.denyOnceOptionId} onPress={() => { if (card.denyOnceOptionId) onResolve(card.approvalId, card.denyOnceOptionId); }}>Deny</Button>
          <Button isDisabled={busy || !card.allowOnceOptionId} onPress={() => {
            if (!card.allowOnceOptionId) return;
            if (!card.actionDisclosed && !confirming) { onConfirmAllow(card.approvalId); return; }
            onConfirmAllow(null);
            onResolve(card.approvalId, card.allowOnceOptionId);
          }}>{confirming ? 'Confirm allow once' : 'Allow once'}</Button>
        </div> : null}
        {confirming ? <p role="note">Allow once sends this response even though the provider did not disclose the action. Choose Confirm allow once to send it, or Deny.</p> : null}
      </article>;
    })}
  </section>;
}

function DisclosedAction({ card }: { card: ReturnType<typeof approvalsForSession>[number] }) {
  return <>
    {card.command ? <pre>{revealHiddenCharacters(card.command)}</pre> : null}
    {card.argv ? <pre>{card.argv.map((arg) => revealHiddenCharacters(arg)).join('\n')}</pre> : null}
    {card.rawInput ? <pre>{revealHiddenCharacters(card.rawInput)}</pre> : null}
    {card.cwd ? <p>cwd <span className="session-id">{revealHiddenCharacters(card.cwd)}</span></p> : null}
    {card.locations ? <pre>{revealHiddenCharacters(card.locations)}</pre> : null}
    {card.changes ? <pre>{revealHiddenCharacters(card.changes)}</pre> : null}
    {card.network ? <p>Network <span className="session-id">{revealHiddenCharacters(card.network)}</span></p> : null}
  </>;
}
