import { useRef, useState } from 'react';
import { Button, ListBox, ListBoxItem } from 'react-aria-components';
import { useVirtualizer } from '@tanstack/react-virtual';
import { agentText, approvalsForSession, eventsForSession, sessionSummaries, type JournalEvent } from './session-model';
import { connectionStatusLabel, type ConnectionPhase } from './browser-session';
import { useSessionJournalEvents } from './store';
import { UntrustedMarkdown } from './UntrustedMarkdown';

export function MainScreen({ phase, keyStorage, approvalInFlight, approvalError, onReconnect, onResolveApproval }: {
  phase: ConnectionPhase;
  keyStorage?: 'webcrypto' | 'noble-fallback';
  approvalInFlight: ReadonlySet<string>;
  approvalError: string | null;
  onReconnect: () => void;
  onResolveApproval: (approvalId: string, optionId: string) => void;
}) {
  const events = useSessionJournalEvents();
  const sessions = sessionSummaries(events);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const selected = sessions.find((session) => session.id === selectedId) ?? null;
  const visible = selected ? eventsForSession(events, selected.id) : [];
  const approvals = selected ? approvalsForSession(events, selected.id) : [];
  return <main className="main-screen">
    <header>
      <div><h1>Shardrail</h1><p>Local sessions from the daemon journal</p></div>
      <div className="controls">
        <p role={phase === 'key-changed' ? 'alert' : 'status'}>{connectionStatusLabel(phase)}</p>
        <Button onPress={onReconnect}>Reconnect</Button>
      </div>
    </header>
    {keyStorage === 'noble-fallback' ? <p role="note">This browser stores an extractable private key seed in IndexedDB because non-extractable WebCrypto is unavailable. Same-origin script can read it.</p> : null}
    <div className="workspace">
      <section className="session-column" aria-labelledby="sessions-heading">
        <h2 id="sessions-heading">Sessions</h2>
        {sessions.length === 0 ? <p>No sessions yet.</p> : <ListBox className="session-list" aria-label="Sessions" selectionMode="single" selectedKeys={selected ? [selected.id] : []} onSelectionChange={(keys) => { if (keys === 'all') return; const next = [...keys][0]; setSelectedId(next == null ? null : String(next)); }}>
          {sessions.map((session) => <ListBoxItem key={session.id} id={session.id} textValue={`${session.id} ${session.agentKind ?? 'agent unknown'} ${session.status}`} onFocus={() => setSelectedId(session.id)}>
            <span className="session-id">{session.id}</span>
            <span>{session.agentKind ?? 'agent unknown'}</span>
            <span>{session.status}</span>
            <ActivityTime ms={session.lastActivityMs}/>
          </ListBoxItem>)}
        </ListBox>}
      </section>
      <section className="session-detail" aria-labelledby="events-heading">
        <h2 id="events-heading">{selected ? `Events for ${selected.id}` : 'Events'}</h2>
        {selected ? <SessionTranscript events={visible}/> : <p>Select a session.</p>}
        {selected ? <Approvals cards={approvals} inFlight={approvalInFlight} error={approvalError} onResolve={onResolveApproval}/> : null}
      </section>
    </div>
  </main>;
}

function ActivityTime({ ms }: { ms: number }) {
  if (ms <= 0) return <time>no activity yet</time>;
  const iso = new Date(ms).toISOString();
  return <time dateTime={iso}>{new Date(ms).toLocaleString()}</time>;
}

function SessionTranscript({ events }: { events: readonly JournalEvent[] }) {
  const parent = useRef<HTMLDivElement>(null);
  // TanStack Virtual returns imperative methods that React Compiler cannot safely memoize.
  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({ count: events.length, getScrollElement: () => parent.current, estimateSize: () => 96, overscan: 8 });
  const items = virtualizer.getVirtualItems();
  // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- scrollable transcript needs keyboard focus
  return <div ref={parent} className="transcript" role="region" aria-label="Session events" tabIndex={0}>
    {events.length === 0 ? <p>No events yet.</p> : <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>{items.map((item) => {
      const event = events[item.index];
      return event ? <div key={event.cursor} data-index={item.index} style={{ position: 'absolute', top: 0, left: 0, width: '100%', height: 96, transform: `translateY(${item.start}px)` }}><SessionEvent event={event}/></div> : null;
    })}</div>}
  </div>;
}

function SessionEvent({ event }: { event: JournalEvent }) {
  const text = agentText(event);
  return <article className="event">
    <div className="meta"><b>{event.kind}</b><span>#{event.cursor}</span>{event.occurredAtMs > 0 ? <time dateTime={new Date(event.occurredAtMs).toISOString()}>{new Date(event.occurredAtMs).toLocaleTimeString()}</time> : null}</div>
    {text !== null ? <div className="body"><UntrustedMarkdown source={text}/></div> : null}
  </article>;
}

function Approvals({ cards, inFlight, error, onResolve }: {
  cards: ReturnType<typeof approvalsForSession>;
  inFlight: ReadonlySet<string>;
  error: string | null;
  onResolve: (approvalId: string, optionId: string) => void;
}) {
  return <section className="approvals" aria-labelledby="approvals-heading">
    <h2 id="approvals-heading">Approvals</h2>
    {error ? <p role="alert">{error}</p> : null}
    {cards.length === 0 ? <p>No approvals for this session.</p> : null}
    {cards.map((card) => <article key={card.approvalId} aria-label={`Approval ${card.approvalId}`}>
      <p>{card.state}{card.decision ? ` · ${card.decision}` : ''}</p>
      <p>Tool {card.toolKind}</p>
      {card.toolCallId ? <pre>{card.toolCallId}</pre> : null}
      {card.agentTitle || card.agentDescription ? <div className="untrusted"><b>Agent’s description · untrusted</b><UntrustedMarkdown source={[card.agentTitle, card.agentDescription].filter(Boolean).join('\n\n')}/></div> : null}
      {card.state === 'pending' ? <div className="controls">
        <Button isDisabled={inFlight.has(card.approvalId) || !card.denyOnceOptionId} onPress={() => { if (card.denyOnceOptionId) onResolve(card.approvalId, card.denyOnceOptionId); }}>Deny</Button>
        <Button isDisabled={inFlight.has(card.approvalId) || !card.allowOnceOptionId} onPress={() => { if (card.allowOnceOptionId) onResolve(card.approvalId, card.allowOnceOptionId); }}>Allow once</Button>
      </div> : null}
    </article>)}
  </section>;
}
