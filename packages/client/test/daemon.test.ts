import assert from "node:assert/strict";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { createServer } from "node:net";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { once } from "node:events";
import { after, test } from "node:test";
import { createHash, randomUUID } from "node:crypto";
import * as ed from "@noble/ed25519";
import { connect } from "../src/index.ts";

const root = resolve(import.meta.dirname, "../../..");
const daemonBin = process.env.SHARDRAILD_BIN ?? join(root, "target/debug/shardraild");
const fakeBin = process.env.SHARDRAIL_FAKE_ACP_BIN ?? join(root, "target/debug/shardrail-fake-acp");
const children: ChildProcess[] = [];
const withDeadline = async <T>(promise: Promise<T>, ms: number, label: string): Promise<T> => {
  let timer: NodeJS.Timeout | undefined;
  try {
    return await Promise.race([promise, new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error(`${label} exceeded ${ms}ms`)), ms);
    })]);
  } finally {
    clearTimeout(timer);
  }
};

after(async () => {
  for (const child of children) {
    if (child.exitCode !== null || child.signalCode !== null) continue;
    try { process.kill(-child.pid!, "SIGKILL"); } catch { child.kill("SIGKILL"); }
    await withDeadline(once(child, "exit"), 5_000, "daemon shutdown");
  }
});

test("rejects a fake daemon that presents its own unpinned identity", async () => {
  const data = await withDeadline(mkdtemp(join(tmpdir(), "shardrail-fake-daemon-")), 10_000, "fake daemon temp dir");
  const socketPath = join(data, "fake.sock");
  const server = createServer((socket) => {
    socket.once("data", () => socket.write(`${JSON.stringify({
      protocol_version: 1, public_key: "00".repeat(32), nonce: "00".repeat(32), signature: "00".repeat(64),
    })}\n`));
  });
  try {
    await withDeadline(new Promise<void>((resolveListen, reject) => server.listen(socketPath, resolveListen).once("error", reject)), 5_000, "fake daemon listen");
    await withDeadline(assert.rejects(connect({
      socketPath, clientId: "fake", privateKey: new Uint8Array(32), hostPublicKey: "11".repeat(32),
    }), /does not match the pinned key/), 5_000, "fake daemon authentication rejection");
  } finally {
    await withDeadline(new Promise<void>((resolveClose) => server.close(() => resolveClose())), 5_000, "fake daemon server close");
    await withDeadline(rm(data, { recursive: true, force: true }), 5_000, "fake daemon temp cleanup");
  }
});

test("surfaces daemon authentication errors", async () => {
  const data = await withDeadline(mkdtemp(join(tmpdir(), "shardrail-auth-error-")), 10_000, "auth error temp dir");
  const socketPath = join(data, "auth-error.sock");
  const server = createServer((socket) => {
    socket.once("data", () => socket.write(`${JSON.stringify({ error: "client is not registered" })}\n`));
  });
  try {
    await withDeadline(new Promise<void>((resolveListen, reject) => server.listen(socketPath, resolveListen).once("error", reject)), 5_000, "auth error server listen");
    await withDeadline(assert.rejects(connect({
      socketPath, clientId: "unknown", privateKey: new Uint8Array(32), hostPublicKey: "11".repeat(32),
    }), /client is not registered/), 5_000, "auth error rejection");
  } finally {
    await withDeadline(new Promise<void>((resolveClose) => server.close(() => resolveClose())), 5_000, "auth error server close");
    await withDeadline(rm(data, { recursive: true, force: true }), 5_000, "auth error temp cleanup");
  }
});

test("reports bounded replay overflow once with an explicit reconnect error", async () => {
  const data = await withDeadline(mkdtemp(join(tmpdir(), "shardrail-overflow-daemon-")), 10_000, "overflow temp dir");
  const socketPath = join(data, "overflow.sock");
  const hostSeed = new Uint8Array(32).fill(9);
  const hostPublicKey = Buffer.from(await withDeadline(ed.getPublicKeyAsync(hostSeed), 5_000, "overflow test key generation")).toString("hex");
  const server = createServer((socket) => {
    socket.on("error", () => {});
    let buffer = "";
    let stage = 0;
    socket.on("data", (chunk) => {
      buffer += chunk.toString("utf8");
      while (buffer.includes("\n")) {
        const newline = buffer.indexOf("\n");
        const frame = JSON.parse(buffer.slice(0, newline)) as Record<string, string>;
        buffer = buffer.slice(newline + 1);
        if (stage++ === 0) {
          const nonce = Buffer.alloc(32, 3);
          const fields = [Buffer.from(frame.nonce!, "hex"), nonce, Buffer.from(frame.public_key!, "hex"), Buffer.from(hostPublicKey, "hex")];
          const digest = createHash("sha256").update("Shardrail mutual client handshake v1\0");
          for (const version of [1, 1, 1]) { const encoded = Buffer.alloc(4); encoded.writeUInt32BE(version); digest.update(encoded); }
          for (const field of fields) { const length = Buffer.alloc(4); length.writeUInt32BE(field.length); digest.update(length).update(field); }
          void ed.signAsync(new Uint8Array(digest.digest()), hostSeed).then((signature) => socket.write(`${JSON.stringify({
            protocol_version: 1, host_id: randomUUID(), journal_epoch: randomUUID(), public_key: hostPublicKey,
            nonce: nonce.toString("hex"), signature: Buffer.from(signature).toString("hex"), scope: "full", cursor: 0, resync_required: false,
          })}\n`));
        } else {
          const backlog = Array.from({ length: 4096 }, (_, cursor) => `${JSON.stringify({ kind: "durable_event", cursor: cursor + 1, event: {} })}\n`).join("");
          socket.write(`${JSON.stringify({ kind: "authenticated" })}\n`);
          setTimeout(() => socket.write(backlog), 100);
        }
      }
    });
  });
  let client: Awaited<ReturnType<typeof connect>> | undefined;
  let timeout: NodeJS.Timeout | undefined;
  try {
    await withDeadline(new Promise<void>((resolveListen, reject) => server.listen(socketPath, resolveListen).once("error", reject)), 5_000, "overflow server listen");
    client = await withDeadline(connect({ socketPath, clientId: randomUUID(), privateKey: new Uint8Array(32), hostPublicKey }), 5_000, "overflow client connect");
    let prematureError: Error | undefined;
    client.onError((error) => { prematureError = error; });
    await withDeadline(new Promise((resolve) => setTimeout(resolve, 20)), 1_000, "overflow handler settle");
    assert.equal(prematureError, undefined, "registering an error handler must not resume frame reads");
    let errorCount = 0;
    const overflow = new Promise<Error>((resolveError, reject) => {
      client!.onError((error) => { clearTimeout(timeout); resolveError(error); });
      timeout = setTimeout(() => reject(new Error("overflow error was not surfaced")), 2_000);
    });
    let unsubscribe = () => {};
    unsubscribe = client.onFrame(() => unsubscribe());
    assert.match((await withDeadline(overflow, 3_000, "overflow notification")).message, /replay buffer overflow.*reconnect to resync/);
    client.onError(() => { errorCount++; });
    await withDeadline(new Promise((resolve) => setTimeout(resolve, 20)), 1_000, "overflow duplicate handler settle");
    assert.equal(errorCount, 1);
  } finally {
    clearTimeout(timeout);
    client?.close();
    await withDeadline(new Promise<void>((resolveClose) => server.close(() => resolveClose())), 5_000, "overflow server close");
    await withDeadline(rm(data, { recursive: true, force: true }), 5_000, "overflow temp cleanup");
  }
});

test("authenticates, sends a command, resumes and reconnects to the real daemon", { timeout: 120_000 }, async () => {
  const data = await withDeadline(mkdtemp(join(tmpdir(), "shardrail-client-")), 10_000, "client daemon temp dir");
  let client: Awaited<ReturnType<typeof connect>> | undefined;
  let daemon: ChildProcess | undefined;
  let daemonStderr = "";
  try {
    const registration = spawnSync(daemonBin, ["--data-dir", data, "client", "add", "--label", "client-test", "--scope", "full"], { encoding: "utf8", timeout: 15_000 });
    assert.equal(registration.status, 0, registration.stderr);
    const values = Object.fromEntries(registration.stdout.trim().split(/\s+/).map((part) => part.split("=")));
    const clientId = values.client_id;
    assert.ok(clientId);
    const privateKey = new Uint8Array(await withDeadline(readFile(join(data, `client-${clientId}.key`)), 5_000, "client private key read"));
    daemon = spawn(daemonBin, ["--data-dir", data, "--no-file-log", "--fake-acp-path", fakeBin], { stdio: ["ignore", "ignore", "pipe"], detached: true });
    daemon.stderr?.setEncoding("utf8");
    daemon.stderr?.on("data", (chunk: string) => { daemonStderr += chunk; });
    const daemonExit = once(daemon, "exit");
    children.push(daemon);
    await withDeadline(new Promise<void>((resolveSocket, reject) => {
      const deadline = setTimeout(() => reject(new Error("daemon socket did not become ready")), 10_000);
      const check = () => {
        if (existsSync(join(data, "shardraild.sock"))) { clearTimeout(deadline); resolveSocket(); }
        else if (daemon?.exitCode !== null) { clearTimeout(deadline); reject(new Error(`daemon exited before socket was ready: ${daemon?.exitCode}`)); }
        else setTimeout(check, 25);
      };
      check();
    }), 15_000, "daemon socket startup");

    const options = { socketPath: join(data, "shardraild.sock"), clientId, privateKey, hostPublicKey: values.host_public_key! };
    await withDeadline(assert.rejects(connect({ ...options, hostPublicKey: "00".repeat(32) }), /does not match the pinned key/), 15_000, "wrong host key rejection");
    await withDeadline(assert.rejects(connect({ ...options, privateKey: new Uint8Array(32).fill(7) }), /client key is not authorized/), 15_000, "wrong client key rejection");
    client = await withDeadline(connect(options), 15_000, "client connect");
    assert.equal(client.host.scope, "full");
    const firstEvent = new Promise<Extract<import("@shardrail/contracts").DaemonFrame, { kind: "durable_event" }>>((resolveEvent) => {
      const off = client!.onFrame((frame) => { if (frame.kind === "durable_event") { off(); resolveEvent(frame); } });
    });
    const commandId = randomUUID();
    client.request({ command_id: commandId, kind: "session.create", expected_state: null, expected_turn_id: null, payload: {} });
    const event = await withDeadline(firstEvent, 15_000, "first durable event");
    assert.equal(event.event.kind, "session_created");
    assert.equal(typeof event.receipt_wall_clock_ms, "number");

    const approval = new Promise<{ approvalId: string; rejectOptionId: string; binding: { content_hash: string; session_id: string; turn_id: string; provider_request_id: string } }>((resolveApproval, reject) => {
      const deadline = setTimeout(() => { offApproval(); reject(new Error("fake agent did not request approval")); }, 10_000);
      const offApproval = client!.onFrame((frame) => {
        if (frame.kind !== "durable_event" || frame.event.kind !== "approval_requested") return;
        const payload = frame.event.payload as { approval_id: string; options: { option_id: string; decision: string }[]; binding: { content_hash: string; session_id: string; turn_id: string; provider_request_id: string } };
        const rejectOption = payload.options.find((option) => option.decision === "deny");
        if (!rejectOption || !payload.binding?.content_hash) { clearTimeout(deadline); offApproval(); reject(new Error("fake agent approval has no deny option")); return; }
        clearTimeout(deadline);
        offApproval();
        resolveApproval({ approvalId: payload.approval_id, rejectOptionId: rejectOption.option_id, binding: payload.binding });
      });
    });
    const { approvalId, rejectOptionId, binding } = await withDeadline(approval, 15_000, "approval request");
    const resolved = new Promise<void>((resolveApproval, reject) => {
      const deadline = setTimeout(() => { offResolved(); reject(new Error("approval resolution was not acknowledged")); }, 10_000);
      const offResolved = client!.onFrame((frame) => {
        if (frame.kind === "approval_resolved" && frame.approval_id === approvalId) {
          clearTimeout(deadline);
          offResolved();
          resolveApproval();
        }
      });
    });
    client.resolveApproval({ approval_id: approvalId, option_id: rejectOptionId, content_hash: binding.content_hash, session_id: binding.session_id, turn_id: binding.turn_id, provider_request_id: binding.provider_request_id });
    await withDeadline(resolved, 15_000, "approval resolution");

    const replay = new Promise<void>((resolveReplay, rejectReplay) => {
      const off = client!.onFrame((frame) => {
        if (frame.kind === "durable_event" && frame.cursor > event.cursor) { clearTimeout(timer); off(); offError(); resolveReplay(); }
      });
      const offError = client!.onError((error) => { clearTimeout(timer); off(); offError(); rejectReplay(error); });
      const timer = setTimeout(() => { off(); offError(); rejectReplay(new Error(`resume did not deliver an event after cursor ${event.cursor}`)); }, 5_000);
    });
    client.resume(event.cursor);
    await withDeadline(replay, 15_000, "journal resume replay");
    const journalEpoch = client.host.journal_epoch;
    client.close();
    client = await withDeadline(connect({ socketPath: join(data, "shardraild.sock"), clientId, privateKey, hostPublicKey: values.host_public_key, cursor: event.cursor - 1, journalEpoch }), 15_000, "reconnect with cursor");
    const reconnected = new Promise<void>((resolveReplay) => {
      const off = client!.onFrame((frame) => { if (frame.kind === "durable_event" && frame.cursor === event.cursor) { off(); resolveReplay(); } });
    });
    await withDeadline(reconnected, 15_000, "reconnect replay");
    const resumed = await withDeadline(client.reconnect(), 15_000, "client reconnect handshake");
    assert.equal(resumed.host.journal_epoch, journalEpoch);
    resumed.close();

    let created = 0;
    const offCreated = client.onFrame((frame) => { if (frame.kind === "durable_event" && frame.event.kind === "session_created") created++; });
    for (let i = 0; i < 40; i++) {
      client.request({ command_id: randomUUID(), kind: "session.create", expected_state: null, expected_turn_id: null, payload: {} });
    }
    await withDeadline(new Promise<void>((resolveCreated, reject) => {
      const deadline = setTimeout(() => reject(new Error(`created ${created}/40 sessions`)), 10_000);
      let checkTimer: NodeJS.Timeout;
      const check = () => {
        if (created === 40) { clearTimeout(deadline); clearTimeout(checkTimer); resolveCreated(); }
        else checkTimer = setTimeout(check, 10);
      };
      check();
    }), 15_000, "40 session creation events");
    offCreated();
    client.close();
    client = await withDeadline(connect({ ...options, cursor: 0, journalEpoch }), 15_000, "full journal replay connect");
    const replayed = new Promise<[number[], number[]]>((resolveReplay, reject) => {
      const cursors: number[][] = [[], []];
      const deadline = setTimeout(() => { off(); reject(new Error(`replayed ${cursors[0]?.length ?? 0}/41 durable events`)); }, 10_000);
      const listeners = cursors.map((received) => client!.onFrame((frame) => {
        if (frame.kind === "durable_event") received.push(frame.cursor);
        if (cursors.every((list) => list.length === 41)) {
          clearTimeout(deadline);
          listeners.forEach((off) => off());
          resolveReplay(cursors as [number[], number[]]);
        }
      }));
      const off = () => listeners.forEach((unsubscribe) => unsubscribe());
    });
    const [firstListenerCursors, secondListenerCursors] = await withDeadline(replayed, 15_000, "full journal replay");
    assert.deepEqual(firstListenerCursors, Array.from({ length: 41 }, (_, index) => index + 1));
    assert.deepEqual(secondListenerCursors, firstListenerCursors);
    const allowedApproval = new Promise<{ approvalId: string; optionId: string; binding: { content_hash: string; session_id: string; turn_id: string; provider_request_id: string } }>((resolveApproval, reject) => {
      const deadline = setTimeout(() => { offAllowed(); reject(new Error("fake agent did not request the second approval")); }, 10_000);
      const offAllowed = client!.onFrame((frame) => {
        if (frame.kind !== "durable_event" || frame.event.kind !== "approval_requested") return;
        const payload = frame.event.payload as { approval_id: string; options: { option_id: string; decision: string }[]; binding: { content_hash: string; session_id: string; turn_id: string; provider_request_id: string } };
        const allowOption = payload.options.find((option) => option.decision === "allow");
        if (!allowOption || !payload.binding?.content_hash) { clearTimeout(deadline); offAllowed(); reject(new Error("fake agent approval has no allow option")); return; }
        clearTimeout(deadline);
        offAllowed();
        resolveApproval({ approvalId: payload.approval_id, optionId: allowOption.option_id, binding: payload.binding });
      });
    });
    client.request({ command_id: randomUUID(), kind: "session.create", expected_state: null, expected_turn_id: null, payload: {} });
    const { approvalId: allowedId, optionId, binding: allowBinding } = await withDeadline(allowedApproval, 15_000, "allow approval request");
    const allowed = new Promise<void>((resolveApproval, reject) => {
      const deadline = setTimeout(() => { offAllowed(); reject(new Error("allow approval resolution was not acknowledged")); }, 10_000);
      const offAllowed = client!.onFrame((frame) => {
        if (frame.kind === "approval_resolved" && frame.approval_id === allowedId) {
          clearTimeout(deadline);
          offAllowed();
          resolveApproval();
        }
      });
    });
    client.resolveApproval({ approval_id: allowedId, option_id: optionId, content_hash: allowBinding.content_hash, session_id: allowBinding.session_id, turn_id: allowBinding.turn_id, provider_request_id: allowBinding.provider_request_id });
    await withDeadline(allowed, 15_000, "allow resolution");
    let disconnectCount = 0;
    const connectionError = new Promise<Error>((resolveError) => {
      client!.onError((error) => { disconnectCount++; resolveError(error); });
    });
    try { process.kill(-daemon.pid!, "SIGKILL"); } catch { daemon.kill("SIGKILL"); }
    await withDeadline(daemonExit, 5_000, "daemon exit after disconnect");
    assert.match((await withDeadline(connectionError, 5_000, "client disconnect notification")).message, /socket closed|ECONNRESET/);
    await withDeadline(new Promise((resolve) => setTimeout(resolve, 20)), 1_000, "disconnect handler settle");
    assert.equal(disconnectCount, 1);
    client.close();
  } catch (error) {
    throw new Error(`${error instanceof Error ? error.message : String(error)}\ndaemon stderr:\n${daemonStderr || "(empty)"}`, { cause: error });
  } finally {
    client?.close();
    if (daemon && daemon.exitCode === null && daemon.signalCode === null) {
      try { process.kill(-daemon.pid!, "SIGKILL"); } catch { daemon.kill("SIGKILL"); }
      await withDeadline(once(daemon, "exit"), 5_000, "daemon cleanup exit");
    }
    await withDeadline(rm(data, { recursive: true, force: true }), 5_000, "client daemon temp cleanup");
  }
});
