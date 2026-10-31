import { Button, Label, ListBox, ListBoxItem, Popover, Select, SelectValue } from 'react-aria-components';
import { activityTimestamp, checklistSummary, sessionChecks, type JournalEvent, type SessionSummary } from '../../session-model';
import { EmptyState } from '../../ui/EmptyState';
import { InlineAlert } from '../../ui/InlineAlert';
import { Icon } from '../../ui/Icon';
import { StatusPill } from '../../ui/StatusPill';
import type { ConnectionPhase } from '../../browser-session';

export function EntryArea({ sessions, events, selectedId, workdirs, pickedWorkdir, createError, createPending, phase, onSelect, onPickWorkdir, onCreateSession }: {
  sessions: readonly SessionSummary[];
  events: readonly JournalEvent[];
  selectedId: string | null;
  workdirs: readonly string[] | null;
  pickedWorkdir: string;
  createError: string | null;
  createPending: boolean;
  phase: ConnectionPhase;
  onSelect: (sessionId: string | null) => void;
  onPickWorkdir: (workdir: string) => void;
  onCreateSession: (workdir: string) => void;
}) {
  const selectedWorkdir = workdirs?.includes(pickedWorkdir) ? pickedWorkdir : (workdirs?.[0] ?? '');
  return <section className="session-column" aria-labelledby="sessions-heading">
    <div className="entry-heading"><div><p className="eyebrow">WORKSPACES</p><h2 id="sessions-heading"><Icon className="icon" name="sessions"/> Sessions <span className="session-count">{sessions.length}</span></h2></div></div>
    {workdirs !== null ? <section className="new-session" aria-label="New session">
      <Select aria-label="Workdir" selectedKey={selectedWorkdir || null} onSelectionChange={(key) => onPickWorkdir(String(key ?? ''))} isDisabled={workdirs.length === 0}>
        <Label>Workdir</Label><Button className="field-control"><SelectValue>{workdirs.length === 0 ? 'No workdir reported' : undefined}</SelectValue><span aria-hidden="true">⌄</span></Button>
        <Popover><ListBox>{workdirs.map((dir) => <ListBoxItem key={dir} id={dir} textValue={dir}>{dir}</ListBoxItem>)}</ListBox></Popover>
      </Select>
      <Button className="create-session-button" isDisabled={selectedWorkdir.length === 0 || createPending || phase !== 'connected'} onPress={() => onCreateSession(selectedWorkdir)}>{createPending ? <><span className="state-spinner" aria-hidden="true"/>{phase === 'connected' ? 'Creating session…' : 'Checking session creation…'}</> : 'New session'}</Button>
      {createPending ? <p className="create-pending" role="status">{phase === 'connected' ? 'Waiting for the daemon to confirm the new session.' : 'Connection interrupted. Reconnect to learn whether the session was created.'}</p> : null}
      {workdirs.length === 0 ? <p className="field-help">No available workdirs were reported by the daemon.</p> : null}
      {phase !== 'connected' && workdirs.length > 0 ? <p className="field-help">Reconnect to the daemon before creating a session.</p> : null}
      {createError ? <InlineAlert>{createError}</InlineAlert> : null}
    </section> : phase === 'connected' ? <div className="permission-state"><Icon className="icon" name="warning"/><p><strong>Session creation unavailable</strong><span>This browser does not have the <code>session.create</code> permission.</span></p></div> : <div className="session-loading" role="status"><span className="state-spinner" aria-hidden="true"/><span>Loading available workdirs…</span></div>}
    {sessions.length === 0 && phase === 'connecting' ? <div className="session-skeleton" role="status" aria-label="Loading sessions"><span/><span/><span/></div> : sessions.length === 0 ? null : <ListBox className="session-list" aria-label="Sessions" selectionMode="single" selectedKeys={selectedId ? [selectedId] : []} onSelectionChange={(keys) => { if (keys === 'all') return; onSelect([...keys][0] == null ? null : String([...keys][0])); }}>
      {sessions.map((session) => {
        const row = checklistSummary(sessionChecks(events, session.id));
        const activity = activityTimestamp(session.lastActivityMs);
        return <ListBoxItem className="session-option" key={session.id} id={session.id} textValue={`${session.id} ${session.agentKind ?? 'agent unknown'} ${session.status} ${row.unverified} not verified`} onFocus={() => onSelect(session.id)}>
          <span className="session-row-heading"><span className="session-id" title={session.id}>{session.label || session.id}</span><StatusPill tone="neutral">{session.lifecycle}</StatusPill></span>
          <span className="session-provider"><Icon className="icon" name="workspace"/>{session.agentKind ?? 'Agent unknown'}</span>
          <span className="session-meta"><span>{session.status} · {row.unverified} checks not verified</span>{activity ? <time dateTime={activity}>{new Date(session.lastActivityMs).toLocaleString()}</time> : <time>no activity yet</time>}</span>
        </ListBoxItem>;
      })}
    </ListBox>}
    {sessions.length === 0 && phase !== 'connecting' ? <div className="phone-empty-state"><EmptyState><span className="empty-mark"><Icon name="workspace"/></span><strong>No sessions yet.</strong><span>Create a session to start monitoring local agent activity.</span></EmptyState></div> : null}
  </section>;
}
