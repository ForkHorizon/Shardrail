import assert from "node:assert/strict";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { once } from "node:events";
import { after, test } from "node:test";
import { randomUUID } from "node:crypto";
import { connect } from "../src/index.ts";

const root = resolve(import.meta.dirname, "../../..");
const daemonBin = process.env.SHARDRAILD_BIN ?? join(root, "target/debug/shardraild");
const fakeBin = process.env.SHARDRAIL_FAKE_ACP_BIN ?? join(root, "target/debug/shardrail-fake-acp");
const children: ChildProcess[] = [];
after(async () => { for (const child of children) child.kill("SIGKILL"); });

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
    const replayed = new Promise<number[]>((resolveReplay, reject) => {
      const cursors: number[] = [];
      const deadline = setTimeout(() => { off(); reject(new Error(`replayed ${cursors.length}/41 durable events`)); }, 10_000);
      const off = client!.onFrame((frame) => {
        if (frame.kind === "durable_event") cursors.push(frame.cursor);
        if (cursors.length === 41) { clearTimeout(deadline); off(); resolveReplay(cursors); }
      });
    });
    const cursors = await replayed;
    assert.deepEqual(cursors, [...cursors].sort((a, b) => a - b));
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
