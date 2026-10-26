import { useLayoutEffect, useRef, useState } from 'react';
import { Button, Disclosure, DisclosurePanel, Heading, Link, TextArea, ToggleButton } from 'react-aria-components';
import { useVirtualizer } from '@tanstack/react-virtual';
import { activityTimestamp, approvalsForSession, checklistSummary, promptClearApplies, revealHiddenCharacters, type ApprovalBindingEcho, type TranscriptItem, type VerificationCheck } from '../../session-model';
import { isNearLiveTail } from '../../streaming-view';
import { UntrustedMarkdown } from '../../UntrustedMarkdown';
import { InlineAlert } from '../../ui/InlineAlert';
import { EmptyState } from '../../ui/EmptyState';
import { Icon } from '../../ui/Icon';

export function SessionArea({ sessionId, provider, status, connected, items, approvals, checklist, promptError, promptClear, approvalInFlight, approvalError, confirmingAllowId, onConfirmAllow, onResolve, onSendPrompt, onInterrupt }: {
  sessionId: string;
  provider: string | null;
  status: string;
  connected: boolean;
  items: readonly TranscriptItem[];
  approvals: ReturnType<typeof approvalsForSession>;
  checklist: ReturnType<typeof checklistSummary>;
  promptError: string | null;
  promptClear: { commandId: string; sessionId: string } | null;
  approvalInFlight: ReadonlySet<string>;
  approvalError: string | null;
  confirmingAllowId: string | null;
  onConfirmAllow: (approvalId: string | null) => void;
  onResolve: (approvalId: string, optionId: string, binding: ApprovalBindingEcho) => void;
  onSendPrompt: (sessionId: string, prompt: string) => void;
  onInterrupt: (sessionId: string) => void;
}) {
  const pendingApproval = approvals.some((card) => card.state === 'pending');
  return <section className="session-detail" aria-labelledby="events-heading">
    <header className="session-heading">
      <div><p className="eyebrow">Session detail</p><h2 id="events-heading" title={sessionId}>Events for {sessionId.slice(0, 8)}</h2></div>
      <dl className="session-facts"><div><dt>Provider</dt><dd>{provider ?? 'Unknown provider'}</dd></div><div><dt>Status</dt><dd><span className="status-dot" aria-hidden="true"/>{status}</dd></div></dl>
    </header>
    {pendingApproval ? <Approvals cards={approvals} checklist={checklist} connected={connected} inFlight={approvalInFlight} error={approvalError} confirmingAllowId={confirmingAllowId} onConfirmAllow={onConfirmAllow} onResolve={onResolve}/> : null}
    <SessionTranscript key={sessionId} sessionId={sessionId} items={items} status={status}/>
    <PromptBox key={`prompt-${sessionId}-${promptClear && promptClearApplies(promptClear.sessionId, sessionId) ? promptClear.commandId : ''}`} sessionId={sessionId} error={promptError} connected={connected} onSend={onSendPrompt} onInterrupt={onInterrupt}/>
    <VerificationChecklist checks={checklist.checks}/>
    {!pendingApproval ? <Approvals cards={approvals} checklist={checklist} connected={connected} inFlight={approvalInFlight} error={approvalError} confirmingAllowId={confirmingAllowId} onConfirmAllow={onConfirmAllow} onResolve={onResolve}/> : null}
  </section>;
}

function SessionTranscript({ sessionId, items, status }: { sessionId: string; items: readonly TranscriptItem[]; status: string }) {
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
  const scrollRegion = <div ref={parent} className="transcript-scroll" role="region" aria-label="Session events" tabIndex={0} onScroll={() => { const el = parent.current; if (!el) return; followTail.current = isNearLiveTail(el.scrollHeight, el.scrollTop, el.clientHeight); }}>
    {items.length === 0 ? <EmptyState>No events yet.</EmptyState> : <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>{virtualItems.map((item) => {
      const event = items[item.index];
      return event ? <div key={event.key} data-index={item.index} ref={virtualizer.measureElement} style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${item.start}px)` }}><SessionEvent event={event}/></div> : null;
    })}</div>}
  </div>;
  return <section className="transcript" aria-labelledby="activity-heading">
    <div className="transcript-heading"><h3 id="activity-heading">Activity</h3>{/running|active|in.progress/i.test(status) ? <span className="streaming-indicator"><span aria-hidden="true"/> Streaming</span> : null}</div>
    {scrollRegion}
  </section>;
}

function SessionEvent({ event }: { event: TranscriptItem }) {
  const iso = activityTimestamp(event.occurredAtMs);
  if (event.presentation === 'lifecycle') return <article className="event event-lifecycle" data-kind={event.kind}>
    <p className="lifecycle-summary">{event.summary}</p>
    <span className="lifecycle-meta">{event.cursorLabel}{iso ? <> · <time dateTime={iso}>{new Date(event.occurredAtMs).toLocaleTimeString()}</time></> : null}</span>
    {event.rawEvents.length > 0 ? <Disclosure className="raw-event"><Heading><Button slot="trigger">Raw</Button></Heading><DisclosurePanel>{event.rawEvents.map((raw, index) => <pre key={index}>{raw}</pre>)}</DisclosurePanel></Disclosure> : null}
  </article>;
  return <article className="event" data-kind={event.kind}>
    <div className="meta"><b>{event.presentation === 'raw' ? 'Unrecognized event' : event.presentation === 'message' ? 'Agent message' : event.presentation === 'approval' ? 'Approval' : event.presentation === 'tool' ? 'Tool activity' : 'Session update'}</b><span>{event.cursorLabel}</span>{iso ? <time dateTime={iso}>{new Date(event.occurredAtMs).toLocaleTimeString()}</time> : null}</div>
    {event.markdown !== null ? <div className="body"><p className="untrusted-label">Agent message · untrusted</p><UntrustedMarkdown source={event.markdown}/></div> : event.presentation === 'approval' ? <div className="body event-summary"><b>Approval requested</b><span>Pending decision</span>{event.approvalId ? <Link href={`#approval-${encodeURIComponent(event.approvalId)}`}>View approval card</Link> : null}</div> : event.presentation === 'tool' ? <div className="body event-summary"><b>Tool activity · untrusted action data</b><code>{event.summary}</code><span>{event.status}</span></div> : <p className="body event-raw-preview">{event.plain}</p>}
    {event.rawEvents.length > 0 ? <Disclosure className="raw-event"><Heading><Button slot="trigger">Raw event{event.rawEvents.length > 1 ? `s (${event.rawEvents.length})` : ''}</Button></Heading><DisclosurePanel>{event.rawEvents.map((raw, index) => <pre key={index}>{raw}</pre>)}</DisclosurePanel></Disclosure> : null}
  </article>;
}

function PromptBox({ sessionId, error, connected, onSend, onInterrupt }: {
  sessionId: string;
  error: string | null;
  connected: boolean;
  onSend: (sessionId: string, prompt: string) => void;
  onInterrupt: (sessionId: string) => void;
}) {
  const [text, setText] = useState('');
  return <section className="prompt" aria-labelledby="prompt-heading">
    <h2 id="prompt-heading">Prompt</h2>
    {error ? <InlineAlert>{error}</InlineAlert> : null}
    <label htmlFor="session-prompt">Message to the agent</label>
    <TextArea id="session-prompt" value={text} onChange={(event) => setText(event.currentTarget.value)} placeholder="Write a prompt…"/>
    {text ? <Disclosure className="draft-preview"><Heading><Button slot="trigger">Draft preview</Button></Heading><DisclosurePanel><pre>{text}</pre></DisclosurePanel></Disclosure> : null}
    <div className="controls">
      <Button isDisabled={!connected || text.trim() === ''} onPress={() => { const prompt = text.trim(); if (!prompt) return; onSend(sessionId, prompt); }}><Icon className="icon" name="send"/> Send prompt</Button>
      <Button className="button-secondary" isDisabled={!connected} onPress={() => onInterrupt(sessionId)}><Icon className="icon" name="interrupt"/> Interrupt</Button>
    </div>
    {!connected ? <p className="blocked-reason" role="status">Prompt and interrupt are unavailable while disconnected. Your draft stays here.</p> : null}
  </section>;
}

function VerificationChecklist({ checks }: { checks: readonly VerificationCheck[] }) {
  const unverified = checks.filter((item) => item.status === 'not verified').length;
  const failed = checks.filter((item) => item.status === 'failed').length;
  const verified = checks.filter((item) => item.status === 'verified').length;
  return <section className="checklist-section" aria-labelledby="checklist-heading">
    <div className="checklist-heading"><div><h2 id="checklist-heading" tabIndex={-1}><Icon className="icon" name="checklist"/> Verification checklist</h2><p>{failed} failed · {unverified} not verified · {verified} verified. Items are not combined into one verdict.</p></div><span className="check-count">{checks.length} checks</span></div>
    <ul className="checklist">
      {checks.map((item) => <li key={item.id} data-check={item.id} data-status={item.status}>
        <span className="check-label">{item.label}</span>
        <span className={`check-status check-${item.status === 'n/a' ? 'not-applicable' : item.status.replaceAll(' ', '-')}`}><Icon className="icon" name={item.status === 'verified' ? 'verified' : item.status === 'failed' ? 'failed' : 'not-verified'}/>{item.status === 'n/a' ? 'not applicable' : item.status}</span>
        <span>{item.source}. {item.detail} Last checked: {item.lastChecked ?? 'never'}.</span>
      </li>)}
    </ul>
  </section>;
}

function Approvals({ cards, checklist, connected, inFlight, error, confirmingAllowId, onConfirmAllow, onResolve }: {
  cards: ReturnType<typeof approvalsForSession>;
  checklist: ReturnType<typeof checklistSummary>;
  connected: boolean;
  inFlight: ReadonlySet<string>;
  error: string | null;
  confirmingAllowId: string | null;
  onConfirmAllow: (approvalId: string | null) => void;
  onResolve: (approvalId: string, optionId: string, binding: ApprovalBindingEcho) => void;
}) {
  const confirmNote = useRef<HTMLParagraphElement>(null);
  const allowEnterPressed = useRef(false);
  useLayoutEffect(() => {
    if (!confirmingAllowId) return;
    confirmNote.current?.scrollIntoView({ block: 'center' });
    confirmNote.current?.focus();
  }, [confirmingAllowId]);
  return <section className="approvals" aria-labelledby="approvals-heading">
    <h2 id="approvals-heading">Approvals</h2>
    {error ? <InlineAlert>{error}</InlineAlert> : null}
    {cards.length === 0 ? <EmptyState>No approvals for this session.</EmptyState> : null}
    {cards.map((card) => {
      const confirming = confirmingAllowId === card.approvalId && card.state === 'pending' && !card.actionDisclosed;
      const busy = inFlight.has(card.approvalId);
      const stateLabel = ({ pending: 'Pending', responding: 'Sending response', responded_unacked: 'Response sent · awaiting acknowledgement', delivered_unconfirmed: 'Outcome unconfirmed', denied: 'Denied', allowed: 'Allowed once', expired: 'Expired', withdrawn: 'Withdrawn' } as Record<string, string>)[card.state] ?? card.state;
      return <article id={`approval-${card.approvalId}`} key={card.approvalId} className={`approval-card approval-${card.state}`} aria-label={`Approval ${card.approvalId}`}>
        <div className="approval-heading"><h3>Approval request</h3><span className={`approval-state state-${card.state}`}><Icon className="icon" name={card.state === 'denied' || card.state === 'expired' || card.state === 'withdrawn' ? 'deny' : card.state === 'allowed' ? 'verified' : 'pending'}/>{stateLabel}</span></div>
        {card.actionDisclosed ? <DisclosedAction card={card}/> : <p className="caution">action not disclosed by provider</p>}
        <dl className="approval-facts">
          <div><dt>Tool</dt><dd>{card.toolKind}</dd></div>
          <div><dt>Sandbox</dt><dd>{card.sandbox ? revealHiddenCharacters(card.sandbox) : 'not disclosed'}</dd></div>
          <div><dt>Approval axis</dt><dd>once</dd></div>
          {card.toolCallId ? <div><dt>Tool call id</dt><dd className="session-id">{card.toolCallId}</dd></div> : null}
        </dl>
        <p className="approval-meta">{card.decision ? `${card.state} · ${card.decision}` : null}</p>
        <div className="approval-checklist"><p>Checklist: {checklist.failed} failed, {checklist.unverified} not verified.</p><Button aria-label="Open full verification checklist" className="button-quiet checklist-link" onPress={() => document.getElementById('checklist-heading')?.scrollIntoView({ block: 'start' })}><Icon className="icon" name="checklist"/></Button></div>
        <AgentDescription title={card.agentTitle} description={card.agentDescription}/>
        {confirming ? <p ref={confirmNote} tabIndex={-1} className="allow-confirm-note" role="note"><Icon className="icon" name="warning"/> Allow once sends this response even though the provider did not disclose the action. Choose Confirm allow once to send it, or Deny.</p> : null}
        {card.state === 'pending' ? <div className="controls approval-actions" onKeyDownCapture={(event) => { if (event.target instanceof HTMLElement && event.target.classList.contains('button-allow') && (event.key === 'Enter' || event.key === 'NumpadEnter')) { allowEnterPressed.current = true; event.preventDefault(); } }}>
          <Button className="button-danger" isDisabled={!connected || busy || !card.binding || !card.denyOnceOptionId} onPress={() => { if (card.binding && card.denyOnceOptionId) onResolve(card.approvalId, card.denyOnceOptionId, card.binding); }}><Icon className="icon" name="deny"/> Deny</Button>
          <Button className="button-allow" isDisabled={!connected || busy || !card.binding || !card.allowOnceOptionId} onPress={(event) => {
            if (event.pointerType === 'keyboard' && allowEnterPressed.current) { allowEnterPressed.current = false; return; }
            allowEnterPressed.current = false;
            if (!card.binding || !card.allowOnceOptionId) return;
            if (!card.actionDisclosed && !confirming) { onConfirmAllow(card.approvalId); return; }
            onConfirmAllow(null);
            onResolve(card.approvalId, card.allowOnceOptionId, card.binding);
          }}>{confirming ? 'Confirm allow once' : 'Allow once'}</Button>
        </div> : null}
      </article>;
    })}
  </section>;
}

export function AgentDescription({ title, description }: { title: string | null; description: string | null }) {
  const source = [title, description].filter(Boolean).join('\n\n');
  const long = source.length > 240;
  const [expanded, setExpanded] = useState(false);
  return <section className="untrusted" aria-label="Agent’s description · untrusted">
    <p className="untrusted-label">Agent’s description · untrusted</p>
    {source ? <div className={`untrusted-content${long && !expanded ? ' clamped' : ''}`}><UntrustedMarkdown source={source}/></div> : <p className="description-empty">No description provided</p>}
    {long ? <ToggleButton className="description-toggle" isSelected={expanded} onChange={setExpanded}>{expanded ? 'Show less' : 'Show more'}</ToggleButton> : null}
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
