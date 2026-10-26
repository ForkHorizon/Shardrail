import { useLayoutEffect, useRef, useState } from 'react';
import { Button, Disclosure, DisclosurePanel, Heading, Link, TextArea } from 'react-aria-components';
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
  return <section className="session-detail" aria-labelledby="events-heading">
    <header className="session-heading">
      <div><p className="eyebrow">Session detail</p><h2 id="events-heading">Events for {sessionId}</h2></div>
      <dl className="session-facts"><div><dt>Provider</dt><dd>{provider ?? 'Unknown provider'}</dd></div><div><dt>Status</dt><dd><span className="status-dot" aria-hidden="true"/>{status}</dd></div></dl>
    </header>
    <SessionTranscript key={sessionId} sessionId={sessionId} items={items} status={status}/>
    <PromptBox key={`prompt-${sessionId}-${promptClear && promptClearApplies(promptClear.sessionId, sessionId) ? promptClear.commandId : ''}`} sessionId={sessionId} error={promptError} connected={connected} onSend={onSendPrompt} onInterrupt={onInterrupt}/>
    <VerificationChecklist checks={checklist.checks}/>
    <Approvals cards={approvals} checklist={checklist} inFlight={approvalInFlight} error={approvalError} confirmingAllowId={confirmingAllowId} onConfirmAllow={onConfirmAllow} onResolve={onResolve}/>
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
  return <div ref={parent} className="transcript" role="region" aria-label="Session events" tabIndex={0} onScroll={() => { const el = parent.current; if (!el) return; followTail.current = isNearLiveTail(el.scrollHeight, el.scrollTop, el.clientHeight); }}>
    <div className="transcript-heading"><h3>Activity</h3>{/running|active|in.progress/i.test(status) ? <span className="streaming-indicator"><span aria-hidden="true"/> Streaming</span> : null}</div>
    {items.length === 0 ? <EmptyState>No events yet.</EmptyState> : <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>{virtualItems.map((item) => {
      const event = items[item.index];
      return event ? <div key={event.key} data-index={item.index} ref={virtualizer.measureElement} style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${item.start}px)` }}><SessionEvent event={event}/></div> : null;
    })}</div>}
  </div>;
}

function SessionEvent({ event }: { event: TranscriptItem }) {
  const iso = activityTimestamp(event.occurredAtMs);
  return <article className="event" data-kind={event.kind}>
    <div className="meta"><b>{event.kind}</b><span>{event.cursorLabel}</span>{iso ? <time dateTime={iso}>{new Date(event.occurredAtMs).toLocaleTimeString()}</time> : null}</div>
    {event.markdown !== null ? <div className="body"><p className="untrusted-label">Agent message · untrusted</p><UntrustedMarkdown source={event.markdown}/></div> : event.presentation === 'approval' ? <div className="body event-summary"><b>Approval requested</b><span>Pending decision</span>{event.approvalId ? <Link href={`#approval-${encodeURIComponent(event.approvalId)}`}>View approval card</Link> : null}</div> : event.presentation === 'tool' ? <div className="body event-summary"><b>Tool activity · untrusted action data</b><code>{event.summary}</code><span>{event.status}</span></div> : event.presentation === 'lifecycle' ? <p className="body lifecycle-summary">{event.summary}</p> : <p className="body event-raw-preview">{event.plain}</p>}
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
    {text ? <details className="draft-preview"><summary>Draft preview</summary><pre>{text}</pre></details> : null}
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

function Approvals({ cards, checklist, inFlight, error, confirmingAllowId, onConfirmAllow, onResolve }: {
  cards: ReturnType<typeof approvalsForSession>;
  checklist: ReturnType<typeof checklistSummary>;
  inFlight: ReadonlySet<string>;
  error: string | null;
  confirmingAllowId: string | null;
  onConfirmAllow: (approvalId: string | null) => void;
  onResolve: (approvalId: string, optionId: string, binding: ApprovalBindingEcho) => void;
}) {
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
        <p>Tool {card.toolKind}</p>
        <p>Sandbox: {card.sandbox ? revealHiddenCharacters(card.sandbox) : 'not disclosed'}</p>
        <p>Approval axis: once</p>
        {card.toolCallId ? <p>Tool call id <span className="session-id">{card.toolCallId}</span></p> : null}
        <p className="approval-meta">{card.state}{card.decision ? ` · ${card.decision}` : ''}</p>
        <p>Checklist: {checklist.failed} failed, {checklist.unverified} not verified.</p>
        <Button onPress={() => document.getElementById('checklist-heading')?.focus()}>Open the full checklist</Button>
        {card.agentTitle || card.agentDescription ? <div className="untrusted"><b>Agent’s description · untrusted</b><UntrustedMarkdown source={[card.agentTitle, card.agentDescription].filter(Boolean).join('\n\n')}/></div> : null}
        {card.state === 'pending' ? <div className="controls approval-actions">
          <Button className="button-danger" isDisabled={busy || !card.binding || !card.denyOnceOptionId} onPress={() => { if (card.binding && card.denyOnceOptionId) onResolve(card.approvalId, card.denyOnceOptionId, card.binding); }}><Icon className="icon" name="deny"/> Deny</Button>
          <Button className="button-allow" isDisabled={busy || !card.binding || !card.allowOnceOptionId} onPress={() => {
            if (!card.binding || !card.allowOnceOptionId) return;
            if (!card.actionDisclosed && !confirming) { onConfirmAllow(card.approvalId); return; }
            onConfirmAllow(null);
            onResolve(card.approvalId, card.allowOnceOptionId, card.binding);
          }}>{confirming ? 'Confirm allow once' : 'Allow once'}</Button>
        </div> : null}
        {confirming ? <p className="allow-confirm-note" role="note"><Icon className="icon" name="warning"/> Allow once sends this response even though the provider did not disclose the action. Choose Confirm allow once to send it, or Deny.</p> : null}
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
