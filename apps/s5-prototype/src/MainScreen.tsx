import { useMemo, useState } from 'react';
import { Button } from 'react-aria-components';
import { approvalsForSession, checklistSummary, eventsForSession, hostBanner, sessionChecks, sessionSummaries, transcriptItems, type ApprovalBindingEcho } from './session-model';
import { connectionStatusLabel, type ConnectionPhase } from './browser-session';
import { useSessionJournalEvents } from './store';
import { EntryArea } from './screens/entry/EntryArea';
import { SessionArea } from './screens/session/SessionArea';
import { ThemeToggle } from './ui/ThemeToggle';
import { Icon } from './ui/Icon';
import { StatusPill } from './ui/StatusPill';

export function MainScreen({ phase, keyStorage, pairedFingerprint, approvalInFlight, approvalError, promptError, promptClear, workdirs, createError, createPending, createdSessionId, dismissedSessionId, onReconnect, onResolveApproval, onSendPrompt, onInterrupt, onCreateSession }: {
  phase: ConnectionPhase;
  keyStorage?: 'webcrypto' | 'noble-fallback';
  pairedFingerprint?: string;
  approvalInFlight: ReadonlySet<string>;
  approvalError: string | null;
  promptError: string | null;
  promptClear: { commandId: string; sessionId: string } | null;
  workdirs: readonly string[] | null;
  createError: string | null;
  createPending: boolean;
  createdSessionId: string | null;
  dismissedSessionId: string | null;
  onReconnect: () => void;
  onResolveApproval: (approvalId: string, optionId: string, binding: ApprovalBindingEcho) => void;
  onSendPrompt: (sessionId: string, prompt: string) => void;
  onInterrupt: (sessionId: string) => void;
  onCreateSession: (workdir: string) => void;
}) {
  const events = useSessionJournalEvents();
  const sessions = useMemo(() => sessionSummaries(events), [events]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [appliedCreateId, setAppliedCreateId] = useState<string | null>(null);
  const [appliedDismissId, setAppliedDismissId] = useState<string | null>(null);
  let nextSelected = selectedId;
  if (createdSessionId && createdSessionId !== appliedCreateId) { setAppliedCreateId(createdSessionId); nextSelected = createdSessionId; }
  if (dismissedSessionId && dismissedSessionId !== appliedDismissId) {
    setAppliedDismissId(dismissedSessionId);
    if (nextSelected === dismissedSessionId) nextSelected = null;
  }
  if (nextSelected !== selectedId) setSelectedId(nextSelected);
  const [pickedWorkdir, setPickedWorkdir] = useState('');
  const [confirmingAllowId, setConfirmingAllowId] = useState<string | null>(null);
  const [mobileDetail, setMobileDetail] = useState(false);
  const selected = sessions.find((session) => session.id === selectedId) ?? null;
  const visible = useMemo(() => selected ? eventsForSession(events, selected.id) : [], [events, selected]);
  const items = useMemo(() => transcriptItems(visible), [visible]);
  const approvals = useMemo(() => selected ? approvalsForSession(events, selected.id) : [], [events, selected]);
  const checks = useMemo(() => sessionChecks(events, selected?.id ?? ''), [events, selected]);
  const checklist = useMemo(() => checklistSummary(checks), [checks]);
  const selectSession = (id: string | null) => { setSelectedId(id); setConfirmingAllowId(null); if (id) setMobileDetail(true); };
  return <main className="main-screen">
    <header className="app-header">
      <div className="brand"><Icon name="shardrail"/><div><h1>Shardrail</h1><p>Local sessions from the daemon journal</p></div></div>
      <div className="header-tools">
        <div className="controls header-controls">
          <StatusPill tone={phase === 'connected' ? 'success' : phase === 'key-changed' ? 'danger' : 'warning'}><Icon className="icon" name={phase === 'connected' ? 'connection' : phase === 'key-changed' ? 'error' : 'reconnect'}/><span className={`connection-state connection-${phase}`} role={phase === 'key-changed' ? 'alert' : 'status'} aria-label={connectionStatusLabel(phase)}><span className="connection-full">{connectionStatusLabel(phase)}</span><span className="connection-short">{phase === 'connected' ? 'Online' : phase === 'key-changed' ? 'Key changed' : 'Reconnecting'}</span></span></StatusPill>
          <ThemeToggle/>
        </div>
        <div className="header-secondary">
          {pairedFingerprint ? <span className="paired-fingerprint" aria-label="Paired daemon fingerprint" title={pairedFingerprint}>{pairedFingerprint}</span> : null}
          <Button className="button-secondary" onPress={onReconnect}>Reconnect</Button>
        </div>
      </div>
    </header>
    <p className="host-banner" role="status">{hostBanner(checks, selected !== null)}</p>
    {keyStorage === 'noble-fallback' ? <p className="inline-note" role="note">This browser stores an extractable private key seed in IndexedDB because non-extractable WebCrypto is unavailable. Same-origin script can read it.</p> : null}
    <div className="workspace">
      <div className={`entry-pane${mobileDetail ? ' mobile-hidden' : ''}`}>
        <EntryArea sessions={sessions} events={events} selectedId={selected?.id ?? null} workdirs={workdirs} pickedWorkdir={pickedWorkdir} createError={createError} createPending={createPending} phase={phase} onSelect={selectSession} onPickWorkdir={setPickedWorkdir} onCreateSession={onCreateSession}/>
      </div>
      <div className={`detail-pane${mobileDetail ? ' mobile-visible' : ''}`}>
        {selected ? <>
          <Button className="button-quiet mobile-back" onPress={() => setMobileDetail(false)}><Icon className="icon" name="back"/> Back to sessions</Button>
          <SessionArea sessionId={selected.id} provider={selected.agentKind} status={selected.status} connected={phase === 'connected'} items={items} approvals={approvals} checklist={checklist} promptError={promptError} promptClear={promptClear} approvalInFlight={approvalInFlight} approvalError={approvalError} confirmingAllowId={confirmingAllowId} onConfirmAllow={setConfirmingAllowId} onResolve={onResolveApproval} onSendPrompt={onSendPrompt} onInterrupt={onInterrupt}/>
        </> : <section className="session-detail empty-detail" aria-labelledby="events-heading"><h2 id="events-heading">Events</h2><p>Select a session to view its events.</p></section>}
      </div>
    </div>
  </main>;
}
