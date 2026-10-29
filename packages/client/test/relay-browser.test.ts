import assert from "node:assert/strict";
import { test } from "node:test";
import { createHash } from "node:crypto";
import * as ed from "@noble/ed25519";
import { connectBrowserRelay, type BrowserKeyRecord } from "../src/browser.ts";

const hex = (bytes: Uint8Array) => Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
const unhex = (value: string) => Uint8Array.from(value.match(/../g)!, (pair) => Number.parseInt(pair, 16));

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

class MockWebSocket {
  static instances: MockWebSocket[] = [];
  url: string;
  onopen: (() => void) | null = null;
  onerror: ((err: unknown) => void) | null = null;
  onclose: (() => void) | null = null;
  listeners: Record<string, ((event: { data: string }) => void)[]> = {};
  sent: string[] = [];
  readyState = 1; // OPEN

  constructor(url: string) {
    this.url = url;
    MockWebSocket.instances.push(this);
    queueMicrotask(() => {
      this.onopen?.();
    });
  }

  addEventListener(event: string, handler: (e: any) => void) {
    this.listeners[event] = this.listeners[event] || [];
    this.listeners[event].push(handler);
  }

  send(data: string) {
    this.sent.push(data);
  }

  close() {
    this.readyState = 3; // CLOSED
    this.onclose?.();
    const closeHandlers = this.listeners["close"] || [];
    for (const h of closeHandlers) h({} as any);
  }

  emitMessage(data: unknown) {
    const text = typeof data === "string" ? data : JSON.stringify(data);
    const msgHandlers = this.listeners["message"] || [];
    for (const h of msgHandlers) h({ data: text });
  }
}

test("connectBrowserRelay rejects when remote machine is offline", async () => {
  const hostSeed = new Uint8Array(32).fill(1);
  const hostPublicKey = hex(await ed.getPublicKeyAsync(hostSeed));
  const clientSeed = new Uint8Array(32).fill(2);
  const clientPublicKey = hex(await ed.getPublicKeyAsync(clientSeed));

  const identity: BrowserKeyRecord = {
    hostPublicKey,
    clientId: "c1",
    publicKey: clientPublicKey,
    storage: "noble-fallback",
    seed: clientSeed,
  };

  const connectPromise = connectBrowserRelay({
    relayUrl: "ws://mock-relay/ws",
    machineId: "mach-1",
    identity,
    WebSocketImpl: MockWebSocket as any,
  });

  // Wait for mock socket open & hello frame
  await new Promise((r) => setTimeout(r, 10));
  const socket = MockWebSocket.instances[MockWebSocket.instances.length - 1]!;
  assert.equal(socket.sent.length, 1);
  assert.deepEqual(JSON.parse(socket.sent[0]!), {
    type: "hello",
    role: "browser",
    machine_id: "mach-1",
    client_id: "c1",
  });

  // Relay responds that machine is offline
  socket.emitMessage({
    type: "status",
    machine_id: "mach-1",
    online: false,
  });

  await assert.rejects(connectPromise, /remote machine is offline/);
});

test("connectBrowserRelay successfully tunnels handshake and streams frames", async () => {
  const hostSeed = new Uint8Array(32).fill(3);
  const hostPublicKey = hex(await ed.getPublicKeyAsync(hostSeed));
  const clientSeed = new Uint8Array(32).fill(4);
  const clientPublicKey = hex(await ed.getPublicKeyAsync(clientSeed));

  const identity: BrowserKeyRecord = {
    hostPublicKey,
    clientId: "c2",
    publicKey: clientPublicKey,
    storage: "noble-fallback",
    seed: clientSeed,
  };

  const connectPromise = connectBrowserRelay({
    relayUrl: "ws://mock-relay/ws",
    machineId: "mach-2",
    identity,
    WebSocketImpl: MockWebSocket as any,
  });

  await new Promise((r) => setTimeout(r, 10));
  const socket = MockWebSocket.instances[MockWebSocket.instances.length - 1]!;

  // Relay reports machine online
  socket.emitMessage({
    type: "status",
    machine_id: "mach-2",
    online: true,
  });

  // Client should now send ClientHello wrapped in relay envelope
  await new Promise((r) => setTimeout(r, 10));
  assert.equal(socket.sent.length, 2);
  const clientHelloEnv = JSON.parse(socket.sent[1]!);
  assert.equal(clientHelloEnv.type, "relay");
  assert.equal(clientHelloEnv.machine_id, "mach-2");
  assert.equal(clientHelloEnv.client_id, "c2");

  const clientHello = clientHelloEnv.payload;
  assert.equal(clientHello.client_id, "c2");
  assert.equal(clientHello.public_key, clientPublicKey);
  const clientNonce = clientHello.nonce;

  // Daemon responds with ServerHello wrapped in relay envelope
  const serverNonce = hex(new Uint8Array(32).fill(5));
  const transcriptBytes = transcript([1, 1, 1], [clientNonce, serverNonce, clientPublicKey, hostPublicKey]);
  const digest = createHash("sha256").update(transcriptBytes).digest();
  const hostSig = hex(await ed.signAsync(digest, hostSeed));

  socket.emitMessage({
    type: "relay",
    machine_id: "mach-2",
    client_id: "c2",
    payload: {
      protocol_version: 1,
      host_id: "00000000-0000-0000-0000-000000000001",
      journal_epoch: "00000000-0000-0000-0000-000000000002",
      public_key: hostPublicKey,
      nonce: serverNonce,
      signature: hostSig,
      scope: "full",
      cursor: 0,
      resync_required: false,
    },
  });

  // Client verifies host signature, generates client signature, and sends authenticate frame
  await new Promise((r) => setTimeout(r, 10));
  assert.equal(socket.sent.length, 3);
  const authEnv = JSON.parse(socket.sent[2]!);
  assert.equal(authEnv.type, "relay");
  assert.equal(authEnv.payload.kind, "authenticate");
  const clientSig = authEnv.payload.signature;
  assert.ok(await ed.verifyAsync(unhex(clientSig), digest, unhex(clientPublicKey)));

  // Daemon confirms authentication
  socket.emitMessage({
    type: "relay",
    machine_id: "mach-2",
    client_id: "c2",
    payload: {
      kind: "authenticated",
    },
  });

  const client = await connectPromise;
  assert.equal(client.host.protocol_version, 1);
  assert.equal(client.host.public_key, hostPublicKey);

  // Test streaming frames
  const receivedFrames: any[] = [];
  client.onFrame((frame) => {
    receivedFrames.push(frame);
  });

  // Client requests a command
  client.request({
    command_id: "00000000-0000-0000-0000-000000000003",
    kind: "session.create",
    expected_state: null,
    expected_turn_id: null,
    payload: {},
  });

  assert.equal(socket.sent.length, 5); // hello, clientHello, auth, resume, request
  const cmdEnv = JSON.parse(socket.sent[4]!);
  assert.equal(cmdEnv.type, "relay");
  assert.equal(cmdEnv.payload.kind, "request");
  assert.equal(cmdEnv.payload.request.kind, "session.create");

  // Daemon responds with durable event
  socket.emitMessage({
    type: "relay",
    machine_id: "mach-2",
    client_id: "c2",
    payload: {
      kind: "durable_event",
      cursor: 1,
      event: {
        kind: "session_created",
        session_id: "00000000-0000-0000-0000-000000000004",
      },
    },
  });

  await new Promise((r) => setTimeout(r, 10));
  assert.equal(receivedFrames.length, 1);
  assert.equal(receivedFrames[0].kind, "durable_event");
  assert.equal(receivedFrames[0].cursor, 1);
  assert.equal(receivedFrames[0].event.kind, "session_created");

  // Test remote machine goes offline during connection
  let errorRaised: Error | null = null;
  client.onError((err) => {
    errorRaised = err;
  });

  socket.emitMessage({
    type: "status",
    machine_id: "mach-2",
    online: false,
  });

  assert.ok(errorRaised);
  assert.match((errorRaised as Error).message, /remote machine went offline/);

  client.close();
});
