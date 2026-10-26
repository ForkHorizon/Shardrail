import { Button, Checkbox, Input, Label, TextField } from 'react-aria-components';
import type { PairingState } from '../../pairing';
import { ThemeToggle } from '../../ui/ThemeToggle';

export function PairingScreen({ state, browserMode, busy, enrollmentCode, fallbackAcknowledged, onCode, onAcknowledge, onReview, onRetry, onPair, onCancel }: {
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
}) {
  const info = 'info' in state ? state.info : undefined;
  return <main className="pairing-page"><section className="pairing-card" aria-labelledby="pairing-title">
    <div className="pairing-heading"><h1 id="pairing-title">{browserMode ? 'Pair this browser' : 'Pair this desktop'}</h1><ThemeToggle/></div>
    {state.kind === 'loading' ? <p>Checking daemon identity…</p> : null}
    {state.kind === 'error' ? <p role="alert">{state.message}</p> : null}
    {state.kind === 'mismatch' ? <p role="alert">The daemon key changed since this browser was paired. Pairing is blocked. Verify the host, remove the old client with the local shardraild client remove command, then clear this site’s data before pairing again.</p> : null}
    {state.kind === 'expired' ? <p role="alert">Confirmation expired. Review the current fingerprint and try again.</p> : null}
    {info ? <><p>{browserMode ? 'Compare this fingerprint with the host shown by the local shardraild identity command. Browser pairing grants limited scope: observe, prompt, and answer approvals with once or deny.' : 'Compare this fingerprint with the daemon identity you expect. Pairing grants this local desktop full client scope.'}</p><output className="fingerprint" aria-label="Daemon SHA-256 fingerprint">{info.fingerprint}</output></> : null}
    {state.kind === 'confirming' ? <>
      <p role="status">Confirm within 60 seconds after checking the fingerprint.</p>
      {browserMode ? <TextField><Label>One-time enrollment code</Label><Input aria-label="One-time enrollment code" autoComplete="off" value={enrollmentCode} onChange={(event) => onCode(event.currentTarget.value.trim())}/></TextField> : null}
      {browserMode && info?.keyStorage === 'noble-fallback' ? <><p role="note">This browser must store an extractable private key seed in IndexedDB because non-extractable WebCrypto Ed25519 is unavailable. Same-origin script can read it.</p><Checkbox isSelected={fallbackAcknowledged} onChange={onAcknowledge}>I understand the browser key can be read by same-origin script</Checkbox></> : null}
      <Button isDisabled={busy || (browserMode && (!enrollmentCode || (info?.keyStorage === 'noble-fallback' && !fallbackAcknowledged)))} onPress={onPair}>I verified this fingerprint — pair</Button>
      <Button className="button-secondary" isDisabled={busy} onPress={onCancel}>Cancel</Button>
    </> : null}
    {state.kind === 'unpaired' || (!browserMode && state.kind === 'mismatch') || state.kind === 'expired' ? <Button onPress={onReview}>{state.kind === 'mismatch' ? 'Review fingerprint and re-pair' : state.kind === 'expired' ? 'Review fingerprint again' : 'Review fingerprint'}</Button> : null}
    {state.kind === 'error' ? <Button className="button-secondary" onPress={onRetry}>Retry</Button> : null}
  </section></main>;
}
