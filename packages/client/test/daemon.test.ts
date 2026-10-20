import assert from "node:assert/strict";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { createServer } from "node:net";
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
after(async () => { for (const child of children) child.kill("SIGKILL"); });

test("rejects a fake daemon that presents its own unpinned identity", async () => {
  const data = await mkdtemp(join(tmpdir(), "shardrail-fake-daemon-"));
  const socketPath = join(data, "fake.sock");
  const server = createServer((socket) => {
    socket.once("data", () => socket.write(`${JSON.stringify({
      protocol_version: 1, public_key: "00".repeat(32), nonce: "00".repeat(32), signature: "00".repeat(64),
    })}\n`));
  });
  try {
    await new Promise<void>((resolveListen, reject) => server.listen(socketPath, resolveListen).once("error", reject));
    await assert.rejects(connect({
      socketPath, clientId: "fake", privateKey: new Uint8Array(32), hostPublicKey: "11".repeat(32),
    }), /does not match the pinned key/);
  } finally {
    await new Promise<void>((resolveClose) => server.close(() => resolveClose()));
    await rm(data, { recursive: true, force: true });
  }
});

test("surfaces daemon authentication errors", async () => {
  const data = await mkdtemp(join(tmpdir(), "shardrail-auth-error-"));
  const socketPath = join(data, "auth-error.sock");
  const server = createServer((socket) => {
    socket.once("data", () => socket.write(`${JSON.stringify({ error: "client is not registered" })}\n`));
  });
  try {
    await new Promise<void>((resolveListen, reject) => server.listen(socketPath, resolveListen).once("error", reject));
    await assert.rejects(connect({
      socketPath, clientId: "unknown", privateKey: new Uint8Array(32), hostPublicKey: "11".repeat(32),
    }), /client is not registered/);
  } finally {
    await new Promise<void>((resolveClose) => server.close(() => resolveClose()));
    await rm(data, { recursive: true, force: true });
  }
});

test("reports bounded replay overflow with an explicit reconnect error", async () => {
  const data = await mkdtemp(join(tmpdir(), "shardrail-overflow-daemon-"));
  const socketPath = join(data, "overflow.sock");
  const hostSeed = new Uint8Array(32).fill(9);
  const hostPublicKey = Buffer.from(await ed.getPublicKeyAsync(hostSeed)).toString("hex");
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
          socket.write(`${JSON.stringify({ kind: "authenticated" })}\n${backlog}`);
        }
      }
    });
  });
  let client: Awaited<ReturnType<typeof connect>> | undefined;
  let timeout: NodeJS.Timeout | undefined;
  try {
    await new Promise<void>((resolveListen, reject) => server.listen(socketPath, resolveListen).once("error", reject));
    client = await connect({ socketPath, clientId: randomUUID(), privateKey: new Uint8Array(32), hostPublicKey });
    const overflow = new Promise<Error>((resolveError, reject) => {
      client!.onError((error) => { clearTimeout(timeout); resolveError(error); });
      timeout = setTimeout(() => reject(new Error("overflow error was not surfaced")), 2_000);
    });
    assert.match((await overflow).message, /replay buffer overflow.*reconnect to resync/);
  } finally {
    clearTimeout(timeout);
    client?.close();
    await new Promise<void>((resolveClose) => server.close(() => resolveClose()));
    await rm(data, { recursive: true, force: true });
  }
});

test("authenticates, sends a command, resumes and reconnects to the real daemon", async () => {
  const data = await mkdtemp(join(tmpdir(), "shardrail-client-"));
  let client: Awaited<ReturnType<typeof connect>> | undefined;
  try {
    const registration = spawnSync(daemonBin, ["--data-dir", data, "client", "add", "--label", "client-test", "--scope", "full"], { encoding: "utf8" });
    assert.equal(registration.status, 0, registration.stderr);
    const values = Object.fromEntries(registration.stdout.trim().split(/\s+/).map((part) => part.split("=")));
    const clientId = values.client_id;
    assert.ok(clientId);
    const privateKey = new Uint8Array(await readFile(join(data, `client-${clientId}.key`)));
    const daemon = spawn(daemonBin, ["--data-dir", data, "--fake-acp-path", fakeBin], { stdio: ["ignore", "ignore", "pipe"] });
    children.push(daemon);
    await once(daemon.stderr!, "data");

    const options = { socketPath: join(data, "shardraild.sock"), clientId, privateKey, hostPublicKey: values.host_public_key! };
    await assert.rejects(connect({ ...options, hostPublicKey: "00".repeat(32) }), /does not match the pinned key/);
    client = await connect(options);
    assert.equal(client.host.scope, "full");
    const firstEvent = new Promise<Extract<import("@shardrail/contracts").DaemonFrame, { kind: "durable_event" }>>((resolveEvent) => {
      const off = client!.onFrame((frame) => { if (frame.kind === "durable_event") { off(); resolveEvent(frame); } });
    });
    const commandId = randomUUID();
    client.request({ command_id: commandId, kind: "session.create", expected_state: null, expected_turn_id: null, payload: {} });
    const event = await firstEvent;
    assert.equal(event.event.kind, "session_created");
    assert.equal(typeof event.receipt_wall_clock_ms, "number");

    const approval = new Promise<{ approvalId: string; rejectOptionId: string }>((resolveApproval, reject) => {
      const deadline = setTimeout(() => { offApproval(); reject(new Error("fake agent did not request approval")); }, 10_000);
      const offApproval = client!.onFrame((frame) => {
        if (frame.kind !== "durable_event" || frame.event.kind !== "approval_requested") return;
        const payload = frame.event.payload as { approval_id: string; options: { option_id: string; decision: string }[] };
        const rejectOption = payload.options.find((option) => option.decision === "deny");
        if (!rejectOption) { clearTimeout(deadline); offApproval(); reject(new Error("fake agent approval has no deny option")); return; }
        clearTimeout(deadline);
        offApproval();
        resolveApproval({ approvalId: payload.approval_id, rejectOptionId: rejectOption.option_id });
      });
    });
    const { approvalId, rejectOptionId } = await approval;
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
    client.resolveApproval({ approval_id: approvalId, option_id: rejectOptionId });
    await resolved;

    client.resume(0);
    const replay = new Promise<void>((resolveReplay) => {
      const off = client!.onFrame((frame) => { if (frame.kind === "durable_event" && frame.cursor === event.cursor) { off(); resolveReplay(); } });
    });
    await replay;
    const journalEpoch = client.host.journal_epoch;
    client.close();
    client = await connect({ socketPath: join(data, "shardraild.sock"), clientId, privateKey, hostPublicKey: values.host_public_key, cursor: event.cursor - 1, journalEpoch });
    const reconnected = new Promise<void>((resolveReplay) => {
      const off = client!.onFrame((frame) => { if (frame.kind === "durable_event" && frame.cursor === event.cursor) { off(); resolveReplay(); } });
    });
    await reconnected;
    const resumed = await client.reconnect();
    assert.equal(resumed.host.journal_epoch, journalEpoch);
    resumed.close();

    let created = 0;
    const offCreated = client.onFrame((frame) => { if (frame.kind === "durable_event" && frame.event.kind === "session_created") created++; });
    for (let i = 0; i < 40; i++) {
      client.request({ command_id: randomUUID(), kind: "session.create", expected_state: null, expected_turn_id: null, payload: {} });
    }
    await new Promise<void>((resolveCreated, reject) => {
      const deadline = setTimeout(() => reject(new Error(`created ${created}/40 sessions`)), 10_000);
      let checkTimer: NodeJS.Timeout;
      const check = () => {
        if (created === 40) { clearTimeout(deadline); clearTimeout(checkTimer); resolveCreated(); }
        else checkTimer = setTimeout(check, 10);
      };
      check();
    });
    offCreated();
    client.close();
    client = await connect({ ...options, cursor: 0, journalEpoch });
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
    const [firstListenerCursors, secondListenerCursors] = await replayed;
    assert.deepEqual(firstListenerCursors, [...firstListenerCursors].sort((a, b) => a - b));
    assert.deepEqual(secondListenerCursors, firstListenerCursors);
    const connectionError = new Promise<Error>((resolveError) => {
      const off = client!.onError((error) => { off(); resolveError(error); });
    });
    daemon.kill("SIGKILL");
    await once(daemon, "exit");
    assert.match((await connectionError).message, /socket closed|ECONNRESET/);
    client.close();
  } finally {
    client?.close();
    await rm(data, { recursive: true, force: true });
  }
});
