import { describe, expect, it } from 'vitest';
import 'fake-indexeddb/auto';
import { createBrowserIdentity, hashBrowserTranscript, openBrowserKeyStore, signBrowserTranscript, type BrowserKeyStore } from '@shardrail/client/browser';
import vector from '../../../test-vectors/client-handshake-v1.json';

describe('browser key storage', () => {
  it('matches the shared Rust and TypeScript transcript vector', async () => {
    const digest = await hashBrowserTranscript(vector.client_nonce, vector.server_nonce, vector.client_public_key, vector.host_public_key);
    expect(Array.from(digest, (byte) => byte.toString(16).padStart(2, '0')).join('')).toBe(vector.transcript_sha256);
  });

  it('stores a non-extractable WebCrypto Ed25519 key when supported', async () => {
    let extractable: boolean | undefined;
    const privateKey = { type: 'private' } as CryptoKey;
    const cryptoProvider = {
      subtle: {
        generateKey: async (_algorithm: unknown, canExtract: boolean) => { extractable = canExtract; return { privateKey, publicKey: {} }; },
        exportKey: async () => Uint8Array.from({ length: 32 }, (_, index) => index).buffer,
      },
      getRandomValues: <T extends ArrayBufferView>(array: T) => array,
    } as unknown as Crypto;
    const store = await openBrowserKeyStore();
    const identity = await createBrowserIdentity('host-webcrypto-key', 'client-1', store, cryptoProvider);
    expect(identity.storage).toBe('webcrypto');
    expect(identity.privateKey).toBe(privateKey);
    expect(extractable).toBe(false);
    expect((await store.get('host-webcrypto-key'))?.privateKey).toEqual(privateKey);
  });

  it('stores and signs with the disclosed Noble seed fallback', async () => {
    const cryptoProvider = {
      subtle: { generateKey: async () => { throw new Error('Ed25519 unavailable'); } },
      getRandomValues: <T extends ArrayBufferView>(array: T) => {
        new Uint8Array(array.buffer, array.byteOffset, array.byteLength).fill(7);
        return array;
      },
    } as unknown as Crypto;
    const store = await openBrowserKeyStore();
    const identity = await createBrowserIdentity('host-noble-key', 'client-2', store, cryptoProvider);
    expect(identity.storage).toBe('noble-fallback');
    expect(identity.seed).toEqual(new Uint8Array(32).fill(7));
    expect((await store.get('host-noble-key'))?.seed).toEqual(identity.seed);
    expect((await signBrowserTranscript(identity, new TextEncoder().encode('protocol transcript'))).length).toBe(64);
  });

  it('falls back when IndexedDB cannot clone a WebCrypto private key', async () => {
    const indexedDbStore = await openBrowserKeyStore();
    const store: BrowserKeyStore = {
      get: (key) => indexedDbStore.get(key),
      list: () => indexedDbStore.list(),
      delete: (key) => indexedDbStore.delete(key),
      put: (record) => record.storage === 'webcrypto' ? Promise.reject(new DOMException('cannot clone CryptoKey', 'DataCloneError')) : indexedDbStore.put(record),
    };
    const cryptoProvider = {
      subtle: {
        generateKey: async () => ({ privateKey: { type: 'private' } as CryptoKey, publicKey: {} }),
        exportKey: async () => Uint8Array.from({ length: 32 }, (_, index) => index + 1).buffer,
      },
      getRandomValues: <T extends ArrayBufferView>(array: T) => { new Uint8Array(array.buffer, array.byteOffset, array.byteLength).fill(8); return array; },
    } as unknown as Crypto;
    const identity = await createBrowserIdentity('host-clone-fallback', 'client-3', store, cryptoProvider);
    expect(identity.storage).toBe('noble-fallback');
    expect((await indexedDbStore.get('host-clone-fallback'))?.seed).toEqual(new Uint8Array(32).fill(8));
  });
});
