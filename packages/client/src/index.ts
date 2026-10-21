import { connect as openSocket, type Socket } from "node:net";
import { randomBytes, createHash } from "node:crypto";
import * as ed from "@noble/ed25519";
import type { ClientFrame, DaemonFrame, ServerHello } from "@shardrail/contracts";

const MAX_QUEUED_FRAMES = 128;

export type {
  ApprovalResolutionRequest,
  ClientFrame,
  ClientHello,
  CommandRequest,
  DaemonFrame,
  DurableEvent,
  ServerHello,
} from "@shardrail/contracts";

export type ClientOptions = {
  socketPath: string;
  clientId: string;
  privateKey: Uint8Array;
  hostPublicKey: string;
  cursor?: number;
  journalEpoch?: string;
};

export type DaemonEvent = Extract<DaemonFrame, { kind: "durable_event" }>;
export type Client = {
  host: ServerHello;
  onFrame(listener: (frame: DaemonFrame) => void): () => void;
  onError(listener: (error: Error) => void): () => void;
  reconnect(): Promise<Client>;
  request(request: Extract<ClientFrame, { kind: "request" }>["request"]): void;
  resolveApproval(request: Extract<ClientFrame, { kind: "approval_resolution" }>["request"]): void;
  resume(cursor: number): void;
  close(): void;
};

const hex = (bytes: Uint8Array) => Buffer.from(bytes).toString("hex");
const unhex = (value: string) => {
  if (!/^(?:[0-9a-f]{2})+$/i.test(value)) throw new Error("invalid hex in daemon handshake");
  return Buffer.from(value, "hex");
};

function transcript(versions: number[], fields: string[]) {
  const hash = createHash("sha256").update("Shardrail mutual client handshake v1\0");
  for (const version of versions) {
    const encoded = Buffer.alloc(4);
    encoded.writeUInt32BE(version);
    hash.update(encoded);
  }
  for (const field of fields) {
    const bytes = unhex(field);
    const length = Buffer.alloc(4);
    length.writeUInt32BE(bytes.length);
    hash.update(length).update(bytes);
  }
  return new Uint8Array(hash.digest());
}

function frameReader(socket: Socket) {
  let buffer = Buffer.alloc(0);
  const waiting: { resolve: (line: string) => void; reject: (error: Error) => void }[] = [];
  const queued: string[] = [];
  let failure: Error | undefined;
  const failureListeners = new Set<(error: Error) => void>();
  const fail = (error: Error) => {
    if (failure) return;
    failure ??= error;
    queued.length = 0;
    waiting.splice(0).forEach(({ reject }) => reject(error));
    failureListeners.forEach((listener) => listener(error));
  };
  socket.on("data", (data: Buffer) => {
    buffer = Buffer.concat([buffer, data]);
    let newline;
    while ((newline = buffer.indexOf(10)) >= 0) {
      if (newline > 1_048_576) { const error = new Error("daemon frame exceeds 1 MiB"); fail(error); socket.destroy(error); return; }
      const line = buffer.subarray(0, newline).toString("utf8");
      buffer = buffer.subarray(newline + 1);
      const waiter = waiting.shift();
      if (waiter) waiter.resolve(line);
      else if (queued.length < MAX_QUEUED_FRAMES) queued.push(line);
      else {
        const error = new Error("daemon replay buffer overflow; reconnect to resync from the last delivered cursor");
        fail(error);
        socket.destroy(error);
        return;
      }
    }
    if (buffer.length > 1_048_576) { const error = new Error("daemon frame exceeds 1 MiB"); fail(error); socket.destroy(error); }
  });
  socket.on("error", fail);
  socket.on("close", () => fail(failure ?? new Error("daemon socket closed")));
  return {
    nextFrame: () => new Promise<string>((resolve, reject) => {
      const line = queued.shift();
      if (line !== undefined) resolve(line);
      else if (failure) reject(failure);
      else waiting.push({ resolve, reject });
    }),
    onFailure(listener: (error: Error) => void) {
      failureListeners.add(listener);
      if (failure) listener(failure);
      return () => failureListeners.delete(listener);
    },
  };
}

function writeFrame(socket: Socket, frame: ClientFrame | import("@shardrail/contracts").ClientHello) {
  socket.write(`${JSON.stringify(frame)}\n`);
}

/** Opens an authenticated protocol-v1 connection to a local shardraild socket. */
export async function connect(options: ClientOptions): Promise<Client> {
  if (options.privateKey.byteLength !== 32) throw new Error("client private key must be a 32-byte Ed25519 seed");
  const socket = openSocket(options.socketPath);
  const reader = frameReader(socket);
  const { nextFrame } = reader;
  await new Promise<void>((resolve, reject) => {
    socket.once("connect", resolve);
    socket.once("error", reject);
  });
  const publicKey = hex(await ed.getPublicKeyAsync(options.privateKey));
  const clientNonce = hex(randomBytes(32));
  writeFrame(socket, {
    min_version: 1, max_version: 1, client_id: options.clientId,
    public_key: publicKey, nonce: clientNonce, resume_after: options.cursor ?? 0,
    journal_epoch: options.journalEpoch ?? null,
  });
  const host = JSON.parse(await nextFrame()) as ServerHello | { error: string };
  if ("error" in host) { socket.destroy(); throw new Error(String(host.error)); }
  if (host.protocol_version !== 1) { socket.destroy(); throw new Error("unsupported daemon protocol version"); }
  if (host.public_key !== options.hostPublicKey) {
    socket.destroy(); throw new Error("daemon host key does not match the pinned key");
  }
  const message = transcript([1, 1, host.protocol_version], [clientNonce, host.nonce, publicKey, host.public_key]);
  if (!await ed.verifyAsync(unhex(host.signature), message, unhex(host.public_key))) {
    socket.destroy(); throw new Error("daemon handshake signature is invalid");
  }
  writeFrame(socket, { kind: "authenticate", signature: hex(await ed.signAsync(message, options.privateKey)) });
  const auth = JSON.parse(await nextFrame()) as DaemonFrame;
  if (auth.kind !== "authenticated") { socket.destroy(); throw new Error(auth.kind === "error" ? auth.error : "daemon authentication failed"); }

  const listeners = new Set<(frame: DaemonFrame) => void>();
  const errorListeners = new Set<(error: Error) => void>();
  const listenerWaiters: { resolve: () => void; reject: (error: Error) => void }[] = [];
  let connectionError: Error | undefined;
  const reportFailure = (error: Error) => {
    if (connectionError) return;
    connectionError = error;
    listenerWaiters.splice(0).forEach(({ reject }) => reject(error));
    errorListeners.forEach((listener) => listener(error));
  };
  reader.onFailure(reportFailure);
  let cursor = options.cursor ?? 0;
  let resumed = false;
  socket.pause();
  void (async () => {
    while (true) {
      try {
        const line = await nextFrame();
        if (!listeners.size) await new Promise<void>((resolve, reject) => listenerWaiters.push({ resolve, reject }));
        if (line) {
          const frame = JSON.parse(line) as DaemonFrame;
          if (frame.kind === "durable_event" && frame.cursor <= cursor) continue;
          listeners.forEach((listener) => listener(frame));
          if (frame.kind === "durable_event") cursor = Math.max(cursor, frame.cursor);
        }
      } catch (error) {
        const failure = error instanceof Error ? error : new Error(String(error));
        reportFailure(failure);
        break;
      }
    }
  })();
  return {
    host,
    onFrame(listener) {
      const wasPaused = listeners.size === 0;
      listeners.add(listener);
      listenerWaiters.splice(0).forEach(({ resolve }) => resolve());
      if (wasPaused && resumed) writeFrame(socket, { kind: "resume", resume_after: cursor });
      resumed = true;
      socket.resume();
      return () => {
        listeners.delete(listener);
      };
    },
    onError(listener) {
      errorListeners.add(listener);
      if (connectionError) listener(connectionError);
      return () => errorListeners.delete(listener);
    },
    reconnect() { return connect({ ...options, cursor, journalEpoch: host.journal_epoch }); },
    request(request) { writeFrame(socket, { kind: "request", request }); },
    resolveApproval(request) { writeFrame(socket, { kind: "approval_resolution", request }); },
    resume(nextCursor) { cursor = Math.max(0, nextCursor); writeFrame(socket, { kind: "resume", resume_after: cursor }); },
    close() { socket.end(); },
  };
}
