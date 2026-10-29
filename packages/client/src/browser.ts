import * as ed from "@noble/ed25519";
import type { ClientFrame, DaemonFrame, ServerHello } from "@shardrail/contracts";

const DB_NAME = "shardrail-browser-keys";
const DB_STORE = "clients";
const MAX_QUEUED_FRAMES = 128;

export type BrowserKeyRecord = {
  hostPublicKey: string;
  clientId: string;
  publicKey: string;
  storage: "webcrypto" | "noble-fallback";
  privateKey?: CryptoKey;
  seed?: Uint8Array;
};

export interface BrowserKeyStore {
  get(hostPublicKey: string): Promise<BrowserKeyRecord | undefined>;
  list(): Promise<BrowserKeyRecord[]>;
  put(record: BrowserKeyRecord): Promise<void>;
  delete(hostPublicKey: string): Promise<void>;
}

function request<T>(value: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    value.onsuccess = () => resolve(value.result);
    value.onerror = () => reject(value.error ?? new Error("IndexedDB request failed"));
  });
}

export async function openBrowserKeyStore(): Promise<BrowserKeyStore> {
  const opening = indexedDB.open(DB_NAME, 1);
  opening.onupgradeneeded = () => opening.result.createObjectStore(DB_STORE, { keyPath: "hostPublicKey" });
  const db = await request(opening);
  const transact = (mode: IDBTransactionMode, action: (store: IDBObjectStore) => IDBRequest) =>
    request(action(db.transaction(DB_STORE, mode).objectStore(DB_STORE)));
  return {
    get: (hostPublicKey) => transact("readonly", (store) => store.get(hostPublicKey)) as Promise<BrowserKeyRecord | undefined>,
    list: () => transact("readonly", (store) => store.getAll()) as Promise<BrowserKeyRecord[]>,
    put: async (record) => { await transact("readwrite", (store) => store.put(record)); },
    delete: async (hostPublicKey) => { await transact("readwrite", (store) => store.delete(hostPublicKey)); },
  };
}

const hex = (bytes: Uint8Array) => Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
const unhex = (value: string) => {
  if (!/^(?:[0-9a-f]{2})+$/i.test(value)) throw new Error("invalid hex in daemon handshake");
  return Uint8Array.from(value.match(/../g)!, (pair) => Number.parseInt(pair, 16));
};

function transcript(versions: number[], fields: string[]) {
  const chunks = [new TextEncoder().encode("Shardrail mutual client handshake v1\0")];
  for (const version of versions) chunks.push(Uint8Array.of(version >>> 24, version >>> 16, version >>> 8, version));
  for (const field of fields) {
    const bytes = unhex(field);
    chunks.push(Uint8Array.of(bytes.length >>> 24, bytes.length >>> 16, bytes.length >>> 8, bytes.length), bytes);
  }
  const input = new Uint8Array(chunks.reduce((length, chunk) => length + chunk.length, 0));
  let offset = 0;
  for (const chunk of chunks) { input.set(chunk, offset); offset += chunk.length; }
  return input;
}

export async function hashBrowserTranscript(clientNonce: string, serverNonce: string, clientPublicKey: string, hostPublicKey: string, cryptoProvider: Crypto = crypto) {
  return new Uint8Array(await cryptoProvider.subtle.digest("SHA-256", Uint8Array.from(transcript([1, 1, 1], [clientNonce, serverNonce, clientPublicKey, hostPublicKey]))));
}

export async function createBrowserIdentity(
  hostPublicKey: string,
  clientId: string,
  store?: BrowserKeyStore,
  cryptoProvider: Crypto = crypto,
): Promise<BrowserKeyRecord> {
  const keyStore = store ?? await openBrowserKeyStore();
  const existing = await keyStore.get(hostPublicKey);
  if (existing) return existing;
  let record: BrowserKeyRecord;
  try {
    const pair = await cryptoProvider.subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"]) as CryptoKeyPair;
    record = { hostPublicKey, clientId, publicKey: hex(new Uint8Array(await cryptoProvider.subtle.exportKey("raw", pair.publicKey))), storage: "webcrypto", privateKey: pair.privateKey };
  } catch {
    const seed = cryptoProvider.getRandomValues(new Uint8Array(32));
    record = { hostPublicKey, clientId, publicKey: hex(await ed.getPublicKeyAsync(seed)), storage: "noble-fallback", seed };
  }
  try {
    await keyStore.put(record);
  } catch (error) {
    if (record.storage !== "webcrypto" || !(error && typeof error === "object" && "name" in error && error.name === "DataCloneError")) throw error;
    const seed = cryptoProvider.getRandomValues(new Uint8Array(32));
    record = { hostPublicKey, clientId, publicKey: hex(await ed.getPublicKeyAsync(seed)), storage: "noble-fallback", seed };
    await keyStore.put(record);
  }
  return record;
}

export async function signBrowserTranscript(record: BrowserKeyRecord, message: Uint8Array, cryptoProvider: Crypto = crypto) {
  if (record.storage === "webcrypto" && record.privateKey) {
    return new Uint8Array(await cryptoProvider.subtle.sign({ name: "Ed25519" }, record.privateKey, Uint8Array.from(message)));
  }
  if (record.storage === "noble-fallback" && record.seed) return ed.signAsync(message, record.seed);
  throw new Error("browser client key is unavailable");
}

export type BrowserClientOptions = {
  url: string;
  identity: BrowserKeyRecord;
  cursor?: number | undefined;
  journalEpoch?: string | undefined;
  WebSocketImpl?: typeof WebSocket | undefined;
  cryptoProvider?: Crypto | undefined;
};

export type BrowserClient = {
  host: ServerHello;
  onFrame(listener: (frame: DaemonFrame) => void): () => void;
  onError(listener: (error: Error) => void): () => void;
  reconnect(): Promise<BrowserClient>;
  request(request: Extract<ClientFrame, { kind: "request" }>["request"]): void;
  resolveApproval(request: Extract<ClientFrame, { kind: "approval_resolution" }>["request"]): void;
  resume(cursor: number): void;
  close(): void;
};

export function resumeCursor(requested: number | undefined, host: Pick<ServerHello, "cursor" | "resync_required">) {
  if (host.resync_required || requested === undefined || !Number.isSafeInteger(requested) || requested < 0) return 0;
  return Math.min(requested, host.cursor);
}

function socketFrames(socket: WebSocket) {
  const queued: string[] = [];
  const waiting: { resolve: (value: string) => void; reject: (error: Error) => void }[] = [];
  const listeners = new Set<(error: Error) => void>();
  let failure: Error | undefined;
  const fail = (error: Error) => {
    if (failure) return;
    failure = error;
    waiting.splice(0).forEach(({ reject }) => reject(error));
    listeners.forEach((listener) => listener(error));
    socket.close();
  };
  socket.addEventListener("message", (event) => {
    if (typeof event.data !== "string" || event.data.length > 1_048_576) return fail(new Error("invalid or oversized daemon frame"));
    const waiter = waiting.shift();
    if (waiter) waiter.resolve(event.data);
    else if (queued.length < MAX_QUEUED_FRAMES) queued.push(event.data);
    else fail(new Error("daemon replay buffer overflow; reconnect to resync from the last delivered cursor"));
  });
  socket.addEventListener("error", () => fail(new Error("daemon WebSocket failed")));
  socket.addEventListener("close", () => fail(new Error("daemon WebSocket closed")));
  return {
    next: () => new Promise<string>((resolve, reject) => {
      const value = queued.shift();
      if (value !== undefined) resolve(value);
      else if (failure) reject(failure);
      else waiting.push({ resolve, reject });
    }),
    onError: (listener: (error: Error) => void) => { listeners.add(listener); if (failure) listener(failure); return () => listeners.delete(listener); },
  };
}

interface ProtocolChannel {
  send(frame: unknown): void;
  next(): Promise<string>;
  onError(listener: (error: Error) => void): () => void;
  close(): void;
  reconnect(cursor: number, journalEpoch: string): Promise<BrowserClient>;
}

async function runBrowserSession(
  channel: ProtocolChannel,
  options: {
    identity: BrowserKeyRecord;
    cursor?: number | undefined;
    journalEpoch?: string | undefined;
    cryptoProvider?: Crypto | undefined;
  },
): Promise<BrowserClient> {
  const send = channel.send;
  const next = channel.next;
  const nonce = crypto.getRandomValues(new Uint8Array(32));
  const clientNonce = hex(nonce);
  send({ min_version: 1, max_version: 1, client_id: options.identity.clientId, public_key: options.identity.publicKey, nonce: clientNonce, resume_after: options.cursor ?? 0, journal_epoch: options.journalEpoch ?? null });
  const host = JSON.parse(await next()) as ServerHello | { error: string };
  if ("error" in host) { channel.close(); throw new Error(String(host.error)); }
  if (host.protocol_version !== 1) { channel.close(); throw new Error("unsupported daemon protocol version"); }
  if (host.public_key !== options.identity.hostPublicKey) { channel.close(); throw new Error("daemon host key changed; pairing is blocked"); }
  const message = transcript([1, 1, host.protocol_version], [clientNonce, host.nonce, options.identity.publicKey, host.public_key]);
  const digest = new Uint8Array(await (options.cryptoProvider ?? crypto).subtle.digest("SHA-256", Uint8Array.from(message)));
  const valid = await ed.verifyAsync(unhex(host.signature), digest, unhex(host.public_key));
  if (!valid) { channel.close(); throw new Error("daemon handshake signature is invalid"); }
  send({ kind: "authenticate", signature: hex(await signBrowserTranscript(options.identity, digest, options.cryptoProvider ?? crypto)) });
  const authenticated = JSON.parse(await next()) as DaemonFrame;
  if (authenticated.kind !== "authenticated") { channel.close(); throw new Error(authenticated.kind === "error" ? authenticated.error : "daemon authentication failed"); }

  const frameListeners = new Set<(frame: DaemonFrame) => void>();
  const errorListeners = new Set<(error: Error) => void>();
  const pendingFrames: DaemonFrame[] = [];
  let cursor = resumeCursor(options.cursor, host);
  let resumed = false;
  channel.onError((error) => errorListeners.forEach((listener) => listener(error)));
  void (async () => {
    while (true) {
      try {
        const frame = JSON.parse(await next()) as DaemonFrame;
        if (frame.kind === "durable_event" && frame.cursor <= cursor) continue;
        if (frameListeners.size) frameListeners.forEach((listener) => listener(frame));
        else if (pendingFrames.length < MAX_QUEUED_FRAMES) pendingFrames.push(frame);
        else throw new Error("daemon replay buffer overflow; reconnect to resync from the last delivered cursor");
        if (frame.kind === "durable_event") cursor = Math.max(cursor, frame.cursor);
      } catch (error) {
        const failure = error instanceof Error ? error : new Error(String(error));
        errorListeners.forEach((listener) => listener(failure));
        channel.close();
        return;
      }
    }
  })();
  const sendFrame = (frame: ClientFrame) => send(frame);
  return {
    host,
    onFrame(listener) { frameListeners.add(listener); pendingFrames.splice(0).forEach(listener); if (!resumed) { resumed = true; send({ kind: "resume", resume_after: cursor }); } return () => frameListeners.delete(listener); },
    onError(listener) { errorListeners.add(listener); return () => errorListeners.delete(listener); },
    reconnect() { return channel.reconnect(cursor, host.journal_epoch); },
    request(request) { sendFrame({ kind: "request", request }); },
    resolveApproval(request) { sendFrame({ kind: "approval_resolution", request }); },
    resume(nextCursor) { cursor = Math.max(0, nextCursor); sendFrame({ kind: "resume", resume_after: cursor }); },
    close() { channel.close(); },
  };
}

/** Opens the existing authenticated protocol-v1 handshake over a same-origin loopback WebSocket. */
export async function connectBrowser(options: BrowserClientOptions): Promise<BrowserClient> {
  const socket = new (options.WebSocketImpl ?? WebSocket)(options.url);
  const frames = socketFrames(socket);
  await new Promise<void>((resolve, reject) => {
    socket.onopen = () => resolve();
    socket.onerror = () => reject(new Error("could not open daemon WebSocket"));
    socket.onclose = () => reject(new Error("daemon WebSocket closed before opening"));
  });
  const channel: ProtocolChannel = {
    send: (frame: unknown) => socket.send(JSON.stringify(frame)),
    next: frames.next,
    onError: frames.onError,
    close: () => socket.close(),
    reconnect: (cursor, journalEpoch) => connectBrowser({ ...options, cursor, journalEpoch }),
  };
  return runBrowserSession(channel, options);
}

export type BrowserRelayClientOptions = {
  relayUrl: string;
  machineId: string;
  identity: BrowserKeyRecord;
  cursor?: number | undefined;
  journalEpoch?: string | undefined;
  WebSocketImpl?: typeof WebSocket | undefined;
  cryptoProvider?: Crypto | undefined;
};

/** Opens the authenticated protocol-v1 session tunneled through a blind WebSocket relay. */
export async function connectBrowserRelay(options: BrowserRelayClientOptions): Promise<BrowserClient> {
  const socket = new (options.WebSocketImpl ?? WebSocket)(options.relayUrl);
  const queued: string[] = [];
  const waiting: { resolve: (value: string) => void; reject: (error: Error) => void }[] = [];
  const listeners = new Set<(error: Error) => void>();
  let failure: Error | undefined;

  const fail = (error: Error) => {
    if (failure) return;
    failure = error;
    waiting.splice(0).forEach(({ reject }) => reject(error));
    listeners.forEach((listener) => listener(error));
    socket.close();
  };

  let initialStatusResolved = false;
  let statusResolver: (() => void) | undefined;
  let statusRejecter: ((err: Error) => void) | undefined;
  const statusPromise = new Promise<void>((resolve, reject) => {
    statusResolver = resolve;
    statusRejecter = reject;
  });

  socket.addEventListener("message", (event) => {
    if (typeof event.data !== "string" || event.data.length > 1_048_576) {
      return fail(new Error("invalid or oversized relay frame"));
    }
    let msg: Record<string, unknown>;
    try {
      msg = JSON.parse(event.data);
    } catch {
      return fail(new Error("invalid JSON frame from relay"));
    }

    if (msg.type === "status") {
      const online = Boolean(msg.online);
      if (!initialStatusResolved) {
        initialStatusResolved = true;
        if (online) {
          statusResolver?.();
        } else {
          statusRejecter?.(new Error("remote machine is offline"));
          fail(new Error("remote machine is offline"));
        }
      } else if (!online) {
        fail(new Error("remote machine went offline"));
      }
      return;
    }

    if (msg.type === "relay" && msg.machine_id === options.machineId) {
      const payloadStr = typeof msg.payload === "string" ? msg.payload : JSON.stringify(msg.payload);
      const waiter = waiting.shift();
      if (waiter) waiter.resolve(payloadStr);
      else if (queued.length < MAX_QUEUED_FRAMES) queued.push(payloadStr);
      else fail(new Error("daemon replay buffer overflow; reconnect to resync from the last delivered cursor"));
    }
  });

  socket.addEventListener("error", () => {
    const err = new Error("relay WebSocket failed");
    if (!initialStatusResolved) statusRejecter?.(err);
    fail(err);
  });
  socket.addEventListener("close", () => {
    const err = new Error("relay WebSocket closed");
    if (!initialStatusResolved) statusRejecter?.(err);
    fail(err);
  });

  await new Promise<void>((resolve, reject) => {
    socket.onopen = () => resolve();
    socket.onerror = () => reject(new Error("could not open relay WebSocket"));
    socket.onclose = () => reject(new Error("relay WebSocket closed before opening"));
  });

  // Register with relay as browser client
  socket.send(JSON.stringify({
    type: "hello",
    role: "browser",
    machine_id: options.machineId,
    client_id: options.identity.clientId,
  }));

  // Wait for machine status confirmation
  await statusPromise;

  const channel: ProtocolChannel = {
    send: (frame: unknown) => {
      socket.send(JSON.stringify({
        type: "relay",
        machine_id: options.machineId,
        client_id: options.identity.clientId,
        payload: frame,
      }));
    },
    next: () => new Promise<string>((resolve, reject) => {
      const value = queued.shift();
      if (value !== undefined) resolve(value);
      else if (failure) reject(failure);
      else waiting.push({ resolve, reject });
    }),
    onError: (listener) => {
      listeners.add(listener);
      if (failure) listener(failure);
      return () => listeners.delete(listener);
    },
    close: () => socket.close(),
    reconnect: (cursor, journalEpoch) => connectBrowserRelay({ ...options, cursor, journalEpoch }),
  };

  return runBrowserSession(channel, options);
}

export async function enrollBrowser(url: string, code: string, publicKey: string, WebSocketImpl: typeof WebSocket = WebSocket) {
  const socket = new WebSocketImpl(url);
  return new Promise<{ clientId: string; hostPublicKey: string }>((resolve, reject) => {
    socket.onerror = () => reject(new Error("daemon WebSocket failed"));
    socket.onclose = () => reject(new Error("daemon WebSocket closed before enrollment response"));
    socket.onopen = () => socket.send(JSON.stringify({ kind: "enroll", code, public_key: publicKey }));
    socket.onmessage = (event) => {
      try {
        const reply = JSON.parse(String(event.data)) as { kind: string; client_id?: string; host_public_key?: string; error?: string };
        socket.close();
        if (reply.kind !== "enrolled" || !reply.client_id || !reply.host_public_key) return reject(new Error(reply.error ?? "enrollment failed"));
        resolve({ clientId: reply.client_id, hostPublicKey: reply.host_public_key });
      } catch (error) { reject(error instanceof Error ? error : new Error(String(error))); }
    };
  });
}

export async function inspectBrowserIdentity(url: string, WebSocketImpl: typeof WebSocket = WebSocket) {
  const socket = new WebSocketImpl(url);
  const frames = socketFrames(socket);
  await new Promise<void>((resolve, reject) => {
    socket.onopen = () => resolve();
    socket.onerror = () => reject(new Error("could not open daemon WebSocket"));
    socket.onclose = () => reject(new Error("daemon WebSocket closed before opening"));
  });
  socket.send('{"kind":"identity"}');
  try {
    const reply = JSON.parse(await frames.next()) as { kind?: string; host_public_key?: string; error?: string };
    if (reply.kind !== "identity" || !reply.host_public_key) throw new Error(reply.error ?? "daemon identity unavailable");
    return reply.host_public_key;
  } finally { socket.close(); }
}

export async function daemonFingerprint(publicKey: string, cryptoProvider: Crypto = crypto) {
  return hex(new Uint8Array(await cryptoProvider.subtle.digest("SHA-256", unhex(publicKey))));
}

export type RelayMachineInfo = {
  machine_id: string;
  online: boolean;
  clients: number;
};

export async function discoverRelayMachines(
  relayUrl: string,
  WebSocketImpl: typeof WebSocket = WebSocket,
  timeoutMs = 5000,
): Promise<RelayMachineInfo[]> {
  const socket = new WebSocketImpl(relayUrl);
  return new Promise<RelayMachineInfo[]>((resolve, reject) => {
    let resolved = false;
    const timer = setTimeout(() => {
      if (resolved) return;
      resolved = true;
      socket.close();
      reject(new Error("discoverRelayMachines timeout"));
    }, timeoutMs);

    const finish = (err?: Error, list?: RelayMachineInfo[]) => {
      if (resolved) return;
      resolved = true;
      clearTimeout(timer);
      socket.close();
      if (err) reject(err);
      else resolve(list ?? []);
    };

    socket.onerror = () => finish(new Error("relay WebSocket failed"));
    socket.onclose = () => finish(new Error("relay WebSocket closed"));
    socket.onopen = () => {
      socket.send(
        JSON.stringify({
          type: "hello",
          role: "browser",
          machine_id: "",
          client_id: "browser-disc-" + Math.random().toString(36).slice(2),
        }),
      );
    };
    socket.onmessage = (event) => {
      try {
        const msg = JSON.parse(String(event.data)) as {
          type?: string;
          machines?: RelayMachineInfo[];
        };
        if (msg.type === "machines" && Array.isArray(msg.machines)) {
          finish(undefined, msg.machines);
        }
      } catch (err) {
        finish(err instanceof Error ? err : new Error(String(err)));
      }
    };
  });
}

export async function inspectBrowserIdentityRelay(
  relayUrl: string,
  machineId: string,
  WebSocketImpl: typeof WebSocket = WebSocket,
  timeoutMs = 10000,
): Promise<string> {
  const socket = new WebSocketImpl(relayUrl);
  const clientId = "inspect-" + Math.random().toString(36).slice(2);
  return new Promise<string>((resolve, reject) => {
    let resolved = false;
    const timer = setTimeout(() => {
      if (resolved) return;
      resolved = true;
      socket.close();
      reject(new Error("inspectBrowserIdentityRelay timeout"));
    }, timeoutMs);

    const finish = (err?: Error, hostKey?: string) => {
      if (resolved) return;
      resolved = true;
      clearTimeout(timer);
      socket.close();
      if (err) reject(err);
      else resolve(hostKey!);
    };

    socket.onerror = () => finish(new Error("relay WebSocket failed"));
    socket.onclose = () => finish(new Error("relay WebSocket closed before identity response"));

    socket.onopen = () => {
      socket.send(
        JSON.stringify({
          type: "hello",
          role: "browser",
          machine_id: machineId,
          client_id: clientId,
        }),
      );
    };

    socket.onmessage = (event) => {
      try {
        const msg = JSON.parse(String(event.data)) as {
          type?: string;
          online?: boolean;
          machine_id?: string;
          payload?: unknown;
        };
        if (msg.type === "status") {
          if (!msg.online) {
            return finish(new Error("remote machine is offline"));
          }
          socket.send(
            JSON.stringify({
              type: "relay",
              machine_id: machineId,
              client_id: clientId,
              payload: { kind: "identity" },
            }),
          );
          return;
        }

        if (msg.type === "relay" && msg.machine_id === machineId) {
          const payload = typeof msg.payload === "string" ? JSON.parse(msg.payload) : msg.payload;
          if (payload && typeof payload === "object" && payload.kind === "identity" && payload.host_public_key) {
            finish(undefined, String(payload.host_public_key));
          } else {
            finish(new Error(payload?.error ?? "daemon identity unavailable"));
          }
        }
      } catch (err) {
        finish(err instanceof Error ? err : new Error(String(err)));
      }
    };
  });
}

export async function enrollBrowserRelay(
  relayUrl: string,
  machineId: string,
  code: string,
  publicKey: string,
  WebSocketImpl: typeof WebSocket = WebSocket,
  timeoutMs = 10000,
): Promise<{ clientId: string; hostPublicKey: string }> {
  const socket = new WebSocketImpl(relayUrl);
  const clientId = "enroll-" + Math.random().toString(36).slice(2);
  return new Promise<{ clientId: string; hostPublicKey: string }>((resolve, reject) => {
    let resolved = false;
    const timer = setTimeout(() => {
      if (resolved) return;
      resolved = true;
      socket.close();
      reject(new Error("enrollBrowserRelay timeout"));
    }, timeoutMs);

    const finish = (err?: Error, result?: { clientId: string; hostPublicKey: string }) => {
      if (resolved) return;
      resolved = true;
      clearTimeout(timer);
      socket.close();
      if (err) reject(err);
      else resolve(result!);
    };

    socket.onerror = () => finish(new Error("relay WebSocket failed"));
    socket.onclose = () => finish(new Error("relay WebSocket closed before enrollment response"));

    socket.onopen = () => {
      socket.send(
        JSON.stringify({
          type: "hello",
          role: "browser",
          machine_id: machineId,
          client_id: clientId,
        }),
      );
    };

    socket.onmessage = (event) => {
      try {
        const msg = JSON.parse(String(event.data)) as {
          type?: string;
          online?: boolean;
          machine_id?: string;
          payload?: unknown;
        };
        if (msg.type === "status") {
          if (!msg.online) {
            return finish(new Error("remote machine is offline"));
          }
          socket.send(
            JSON.stringify({
              type: "relay",
              machine_id: machineId,
              client_id: clientId,
              payload: { kind: "enroll", code, public_key: publicKey },
            }),
          );
          return;
        }

        if (msg.type === "relay" && msg.machine_id === machineId) {
          const payload = typeof msg.payload === "string" ? JSON.parse(msg.payload) : msg.payload;
          if (payload && typeof payload === "object" && payload.kind === "enrolled" && payload.client_id && payload.host_public_key) {
            finish(undefined, { clientId: String(payload.client_id), hostPublicKey: String(payload.host_public_key) });
          } else {
            finish(new Error(payload?.error ?? "enrollment failed"));
          }
        }
      } catch (err) {
        finish(err instanceof Error ? err : new Error(String(err)));
      }
    };
  });
}
