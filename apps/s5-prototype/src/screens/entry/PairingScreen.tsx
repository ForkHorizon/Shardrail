import { Button, Checkbox, Input, Label, TextField } from 'react-aria-components';
import type { PairingState } from '../../pairing';
import { ThemeToggle } from '../../ui/ThemeToggle';
import { Icon } from '../../ui/Icon';

export function PairingScreen({
  state,
  browserMode,
  busy,
  enrollmentCode,
  fallbackAcknowledged,
  onCode,
  onAcknowledge,
  onReview,
  onRetry,
  onPair,
  onCancel,
  onReset,
  isRelay,
  machineId,
  availableMachines,
  onSelectMachine,
}: {
  state: PairingState;
  browserMode: boolean;
  busy: boolean;
  enrollmentCode: string;
  fallbackAcknowledged: boolean;
  onCode: (value: string) => void;
  onAcknowledge: (value: boolean) => void;
  onReview: () => void;
  onRetry: () => void;
  onPair: () => void;
  onCancel: () => void;
  onReset?: () => void;
  isRelay?: boolean;
  machineId?: string | null;
  availableMachines?: { machine_id: string; online: boolean }[];
  onSelectMachine?: (machineId: string) => void;
}) {
  const info = 'info' in state ? state.info : undefined;
  return <main className="pairing-page"><section className="pairing-card" aria-labelledby="pairing-title">
    <div className="pairing-heading"><div className="pairing-brand"><span className="pairing-mark"><Icon name="shardrail"/></span><span>Shardrail <span className="pairing-context">{isRelay ? (machineId ? `REMOTE: ${machineId}` : 'RELAY CONNECTED') : 'LOCAL DAEMON'}</span></span></div><ThemeToggle/></div>
    <div className="pairing-intro"><p className="eyebrow">SECURE ENROLLMENT</p><h1 id="pairing-title">{browserMode ? 'Pair this browser' : 'Pair this desktop'}</h1><p>Verify the daemon identity before granting this client access.</p></div>
    {availableMachines && availableMachines.length > 0 ? (
      <div style={{ marginBottom: '1rem', display: 'flex', gap: '0.5rem', alignItems: 'center', flexWrap: 'wrap' }}>
        <span style={{ fontSize: '0.85rem' }}>{availableMachines.length > 1 ? 'Select Machine:' : 'Target Machine:'}</span>
        {availableMachines.map((m) => (
          <Button
            key={m.machine_id}
            className={m.machine_id === machineId ? 'button-primary' : 'button-secondary'}
            onPress={() => onSelectMachine?.(m.machine_id)}
          >
            {m.machine_id} {m.online ? '●' : '○'}
          </Button>
        ))}
      </div>
    ) : null}
    {state.kind === 'loading' ? <p className="pairing-state" role="status"><span className="state-spinner" aria-hidden="true"/>Checking daemon identity…</p> : null}
    {state.kind === 'error' && state.message.toLowerCase().includes('waiting') ? (
      <div className="pairing-state" role="status">
        <span className="state-spinner" aria-hidden="true"/>
        <div>
          <p><strong>Waiting for machine connection…</strong></p>
          <p style={{ marginTop: '0.25rem', fontSize: '0.85rem' }}>The relay is online. Launch your machine daemon to connect automatically.</p>
        </div>
      </div>
    ) : state.kind === 'error' ? (
      <p className="pairing-state pairing-error" role="alert"><Icon className="icon" name="error"/>{state.message}</p>
    ) : null}
    {state.kind === 'mismatch' ? <div className="pairing-state pairing-error" role="alert"><Icon className="icon" name="error"/><p><strong>The daemon key changed since this browser was paired. Pairing is blocked.</strong><br/>Verify the host, remove the old client with the local shardraild client remove command, then clear this site’s data before pairing again.</p></div> : null}
    {state.kind === 'expired' ? <p className="pairing-state pairing-error" role="alert"><Icon className="icon" name="error"/>Confirmation expired. Review the current fingerprint and try again.</p> : null}
    {info ? <><div className="fingerprint-block"><p className="field-caption">EXPECTED DAEMON FINGERPRINT <span>SHA-256</span></p><output className="fingerprint" aria-label="Daemon SHA-256 fingerprint">{info.fingerprint}</output></div><p className="pairing-scope">{browserMode ? 'Compare this fingerprint with the host shown by the local shardraild identity command. Browser pairing grants limited scope: observe, prompt, and answer approvals with once or deny.' : 'Compare this fingerprint with the daemon identity you expect. Pairing grants this local desktop full client scope.'}</p></> : null}
    {state.kind === 'confirming' ? <>
      <p className="pairing-state" role="status"><Icon className="icon" name="warning"/>Confirm within 60 seconds after checking the fingerprint.</p>
      {browserMode ? <TextField className="enrollment-field"><Label>Access token / Enrollment code (optional)</Label><Input aria-label="Access token or enrollment code" aria-describedby="enrollment-help" autoComplete="off" value={enrollmentCode} onChange={(event) => onCode(event.currentTarget.value.trim())} placeholder="Leave blank for web-enrolled daemon"/><span id="enrollment-help" className="field-help">Optional when the daemon has web enrollment enabled.</span></TextField> : null}
      {browserMode && info?.keyStorage === 'noble-fallback' ? <div className="fallback-consent"><p role="note">This browser must store an extractable private key seed in IndexedDB because non-extractable WebCrypto Ed25519 is unavailable. Same-origin script can read it.</p><Checkbox isSelected={fallbackAcknowledged} onChange={onAcknowledge}>I understand the browser key can be read by same-origin script</Checkbox></div> : null}
      <Button className="button-primary" isDisabled={busy || (browserMode && info?.keyStorage === 'noble-fallback' && !fallbackAcknowledged)} onPress={onPair}>{busy ? 'Pairing…' : 'I verified this fingerprint — pair'}</Button>
      <Button className="button-secondary" isDisabled={busy} onPress={onCancel}>Cancel</Button>
    </> : null}
    {state.kind === 'unpaired' || state.kind === 'mismatch' || state.kind === 'expired' ? <Button onPress={onReview}>{state.kind === 'mismatch' ? 'Review fingerprint and re-pair' : state.kind === 'expired' ? 'Review fingerprint again' : 'Review fingerprint'}</Button> : null}
    {state.kind === 'mismatch' && onReset ? <Button className="button-secondary" onPress={onReset}>Reset pairing for this machine</Button> : null}
    {state.kind === 'error' ? <Button className="button-secondary" onPress={onRetry}>Retry</Button> : null}
  </section></main>;
}
