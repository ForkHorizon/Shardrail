export type PairingInfo = { daemonPublicKey: string; fingerprint: string; clientId: string | null; pinnedPublicKey: string | null; keyStorage?: 'webcrypto' | 'noble-fallback' };
export type PairingState =
  | { kind: 'loading' }
  | { kind: 'unpaired'; info: PairingInfo }
  | { kind: 'confirming'; info: PairingInfo; expiresAt: number }
  | { kind: 'paired'; info: PairingInfo }
  | { kind: 'mismatch'; info: PairingInfo }
  | { kind: 'expired'; info: PairingInfo }
  | { kind: 'error'; message: string; info?: PairingInfo };

export interface PairingApi {
  inspect(): Promise<PairingInfo>;
  pair(daemonPublicKey: string, enrollmentCode?: string): Promise<string>;
  unpair(): Promise<void>;
}

export function stateFromInfo(info: PairingInfo): PairingState {
  if (info.pinnedPublicKey && info.pinnedPublicKey !== info.daemonPublicKey) return { kind: 'mismatch', info };
  if (info.clientId && info.pinnedPublicKey) return { kind: 'paired', info };
  return { kind: 'unpaired', info };
}

export function beginConfirmation(info: PairingInfo, now = performance.now()): PairingState {
  return { kind: 'confirming', info, expiresAt: now + 60_000 };
}

export async function confirmPairing(state: PairingState, api: PairingApi, now: number | (() => number) = () => performance.now(), enrollmentCode?: string): Promise<PairingState> {
  if (state.kind !== 'confirming') return state;
  const currentTime = () => typeof now === 'function' ? now() : now;
  if (currentTime() >= state.expiresAt) return { kind: 'expired', info: state.info };
  try {
    const current = await api.inspect();
    if (current.daemonPublicKey !== state.info.daemonPublicKey) return { kind: 'mismatch', info: current };
    if (currentTime() >= state.expiresAt) return { kind: 'expired', info: state.info };
    const clientId = await api.pair(current.daemonPublicKey, enrollmentCode);
    return stateFromInfo({ ...current, clientId, pinnedPublicKey: current.daemonPublicKey });
  } catch {
    return { kind: 'error', message: 'Pairing failed. Check that the daemon is available, then retry.', info: state.info };
  }
}

export async function unpairClient(api: PairingApi): Promise<PairingState> {
  try {
    await api.unpair();
  } catch {
    return { kind: 'error', message: 'Unpairing failed. The existing client remains registered.' };
  }
  try {
    return stateFromInfo(await api.inspect());
  } catch {
    return { kind: 'error', message: 'Client removed, but daemon status could not be refreshed.' };
  }
}
