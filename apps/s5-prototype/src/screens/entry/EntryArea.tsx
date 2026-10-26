import { Button, Label, ListBox, ListBoxItem, Popover, Select, SelectValue } from 'react-aria-components';
import { activityTimestamp, checklistSummary, sessionChecks, type JournalEvent, type SessionSummary } from '../../session-model';
import { EmptyState } from '../../ui/EmptyState';
import { InlineAlert } from '../../ui/InlineAlert';
import { Icon } from '../../ui/Icon';

export function EntryArea({ sessions, events, selectedId, workdirs, pickedWorkdir, createError, onSelect, onPickWorkdir, onCreateSession }: {
  sessions: readonly SessionSummary[];
  events: readonly JournalEvent[];
  selectedId: string | null;
  workdirs: readonly string[] | null;
  pickedWorkdir: string;
  createError: string | null;
  onSelect: (sessionId: string | null) => void;
  onPickWorkdir: (workdir: string) => void;
  onCreateSession: (workdir: string) => void;
}) {
  const selectedWorkdir = workdirs?.includes(pickedWorkdir) ? pickedWorkdir : (workdirs?.[0] ?? '');
  return <section className="session-column" aria-labelledby="sessions-heading">
    <h2 id="sessions-heading"><Icon className="icon" name="sessions"/> Sessions</h2>
    {workdirs !== null ? <section className="new-session" aria-label="New session">
      <Select aria-label="Workdir" selectedKey={selectedWorkdir || null} onSelectionChange={(key) => onPickWorkdir(String(key ?? ''))} isDisabled={workdirs.length === 0}>
        <Label>Workdir</Label><Button className="field-control"><SelectValue/><span aria-hidden="true">⌄</span></Button>
        <Popover><ListBox>{workdirs.map((dir) => <ListBoxItem key={dir} id={dir} textValue={dir}>{dir}</ListBoxItem>)}</ListBox></Popover>
      </Select>
      <Button isDisabled={selectedWorkdir.length === 0} onPress={() => onCreateSession(selectedWorkdir)}>New session</Button>
      {createError ? <InlineAlert>{createError}</InlineAlert> : null}
    </section> : null}
    {sessions.length === 0 ? <EmptyState>No sessions yet.</EmptyState> : <ListBox className="session-list" aria-label="Sessions" selectionMode="single" selectedKeys={selectedId ? [selectedId] : []} onSelectionChange={(keys) => { if (keys === 'all') return; onSelect([...keys][0] == null ? null : String([...keys][0])); }}>
      {sessions.map((session) => {
        const row = checklistSummary(sessionChecks(events, session.id));
        const activity = activityTimestamp(session.lastActivityMs);
        return <ListBoxItem key={session.id} id={session.id} textValue={`${session.id} ${session.agentKind ?? 'agent unknown'} ${session.status} ${row.unverified} not verified`} onFocus={() => onSelect(session.id)}>
          <span className="session-id">{session.label || session.id}</span><span>{session.agentKind ?? 'agent unknown'}</span><span>{session.status}</span>
          <span>{row.unverified} checks not verified</span>
          {activity ? <time dateTime={activity}>{new Date(session.lastActivityMs).toLocaleString()}</time> : <time>no activity yet</time>}
        </ListBoxItem>;
      })}
    </ListBox>}
  </section>;
}
