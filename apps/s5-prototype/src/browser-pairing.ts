import { createBrowserIdentity, daemonFingerprint, enrollBrowser, inspectBrowserIdentity, openBrowserKeyStore } from '@shardrail/client/browser';
import type { PairingApi, PairingInfo } from './pairing';

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
    const daemonPublicKey = await inspectBrowserIdentity(browserWebSocketUrl());
    const store = await openBrowserKeyStore();
    const record = await store.get(daemonPublicKey);
    const previous = (await store.list())[0];
    const persistedPin = record?.clientId !== 'pending' ? record?.hostPublicKey ?? previous?.hostPublicKey ?? null : previous?.hostPublicKey ?? null;
    const pinnedPublicKey = localStorage.getItem(pinnedKeyName) ?? persistedPin;
    return {
      daemonPublicKey,
      fingerprint: await daemonFingerprint(daemonPublicKey),
      clientId: record?.clientId === 'pending' ? null : record?.clientId ?? null,
      pinnedPublicKey,
      ...(record ? { keyStorage: record.storage } : {}),
    };
  },
  async pair(daemonPublicKey, enrollmentCode) {
    if (!enrollmentCode) throw new Error('enter the one-time enrollment code from the local daemon');
    const store = await openBrowserKeyStore();
    const identity = await createBrowserIdentity(daemonPublicKey, 'pending', store);
    const enrolled = await enrollBrowser(browserWebSocketUrl(), enrollmentCode, identity.publicKey);
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
