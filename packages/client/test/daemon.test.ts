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

    client = await connect({ socketPath: join(data, "shardraild.sock"), clientId, privateKey, hostPublicKey: values.host_public_key });
    assert.equal(client.host.scope, "full");
    const firstEvent = new Promise<Extract<import("@shardrail/contracts").DaemonFrame, { kind: "durable_event" }>>((resolveEvent) => {
      const off = client!.onFrame((frame) => { if (frame.kind === "durable_event") { off(); resolveEvent(frame); } });
    });
    const commandId = randomUUID();
    client.request({ command_id: commandId, kind: "session.create", expected_state: null, expected_turn_id: null, payload: {} });
    const event = await firstEvent;
    assert.equal(event.event.kind, "session_created");
    assert.equal(typeof event.receipt_wall_clock_ms, "number");

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
    client.close();
  } finally {
    client?.close();
    await rm(data, { recursive: true, force: true });
  }
});
