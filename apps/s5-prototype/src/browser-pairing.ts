import {
  createBrowserIdentity,
  daemonFingerprint,
  discoverRelayMachines,
  enrollBrowser,
  enrollBrowserRelay,
  inspectBrowserIdentity,
  inspectBrowserIdentityRelay,
  openBrowserKeyStore,
  type RelayMachineInfo,
} from '@shardrail/client/browser';
import type { PairingApi, PairingInfo } from './pairing';

export function isRelayEnvironment(): boolean {
  if (typeof location === 'undefined') return false;
  const query = new URLSearchParams(location.search);
  if (query.has('relay')) return true;
  if (query.has('local')) return false;
  return location.hostname !== 'localhost' && location.hostname !== '127.0.0.1';
}

export function defaultRelayUrl(): string {
  if (typeof location === 'undefined') return 'ws://127.0.0.1:4000/ws';
  const query = new URLSearchParams(location.search);
  const param = query.get('relay');
  if (param) return param;
  const url = new URL(location.href);
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
  url.pathname = '/ws';
  url.search = '';
  url.hash = '';
  return url.toString();
}

export function activeMachineId(): string | null {
  if (typeof location === 'undefined') return null;
  const query = new URLSearchParams(location.search);
  const param = query.get('machine');
  if (param) return param;
  return localStorage.getItem('shardrail-active-machine') ?? null;
}

export function setActiveMachineId(machineId: string) {
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem('shardrail-active-machine', machineId);
  }
}

export async function resolveMachineId(relayUrl: string = defaultRelayUrl()): Promise<string> {
  const current = activeMachineId();
  if (current) return current;
  const machines = await discoverRelayMachines(relayUrl);
  const onlineMachine = machines.find((m) => m.online);
  if (onlineMachine) {
    setActiveMachineId(onlineMachine.machine_id);
    return onlineMachine.machine_id;
  }
  if (machines.length > 0 && machines[0]?.machine_id) {
    setActiveMachineId(machines[0].machine_id);
    return machines[0].machine_id;
  }
  throw new Error('Waiting for machine daemon to connect to relay...');
}

export function browserWebSocketUrl() {
  const url = new URL(location.href);
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
  url.pathname = '/ws';
  url.search = '';
  url.hash = '';
  return url.toString();
}

const pinnedKeyName = 'shardrail-daemon-public-key';

export const browserPairingApi: PairingApi = {
  async inspect(): Promise<PairingInfo> {
    let daemonPublicKey: string;
    if (isRelayEnvironment()) {
      const relayUrl = defaultRelayUrl();
      const machineId = await resolveMachineId(relayUrl);
      daemonPublicKey = await inspectBrowserIdentityRelay(relayUrl, machineId);
    } else {
      daemonPublicKey = await inspectBrowserIdentity(browserWebSocketUrl());
    }
    const store = await openBrowserKeyStore();
    let record = await store.get(daemonPublicKey);
    const previous = (await store.list())[0];
    const persistedPin = record?.clientId !== 'pending' ? record?.hostPublicKey ?? previous?.hostPublicKey ?? null : previous?.hostPublicKey ?? null;
    const pinnedPublicKey = localStorage.getItem(pinnedKeyName) ?? persistedPin;
    if (!record && (!pinnedPublicKey || pinnedPublicKey === daemonPublicKey)) {
      record = await createBrowserIdentity(daemonPublicKey, 'pending', store);
    }
    return {
      daemonPublicKey,
      fingerprint: await daemonFingerprint(daemonPublicKey),
      clientId: record?.clientId === 'pending' ? null : record?.clientId ?? null,
      pinnedPublicKey,
      ...(record ? { keyStorage: record.storage } : {}),
    };
  },
  async pair(daemonPublicKey, enrollmentCode, acknowledgeFallback) {
    const store = await openBrowserKeyStore();
    const identity = await createBrowserIdentity(daemonPublicKey, 'pending', store);
    if (identity.storage === 'noble-fallback' && acknowledgeFallback !== true) {
      throw new Error('acknowledge the extractable browser key fallback before pairing');
    }
    const code = (enrollmentCode ?? '').trim();
    let enrolled: { clientId: string; hostPublicKey: string };
    if (isRelayEnvironment()) {
      const relayUrl = defaultRelayUrl();
      const machineId = await resolveMachineId(relayUrl);
      enrolled = await enrollBrowserRelay(relayUrl, machineId, code, identity.publicKey);
    } else {
      enrolled = await enrollBrowser(browserWebSocketUrl(), code, identity.publicKey);
    }
    if (enrolled.hostPublicKey !== daemonPublicKey) {
      await store.delete(daemonPublicKey);
      throw new Error('daemon key changed during enrollment; pairing is blocked');
    }
    await store.put({ ...identity, clientId: enrolled.clientId });
    localStorage.setItem(pinnedKeyName, daemonPublicKey);
    return enrolled.clientId;
  },
  async unpair() {
    throw new Error('browser keys are stored in this browser; remove this client with the local shardraild client remove command before clearing site data');
  },
};
