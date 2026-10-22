import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { createHash } from "node:crypto";
import { test } from "node:test";
import * as ed from "@noble/ed25519";

test("shared Rust and Swift handshake vector", async () => {
  const vector = JSON.parse(await readFile(resolve(import.meta.dirname, "../../../test-vectors/client-handshake-v1.json"), "utf8"));
  const fields = [vector.client_nonce, vector.server_nonce, vector.client_public_key, vector.host_public_key].map((v: string) => Buffer.from(v, "hex"));
  const hash = createHash("sha256").update("Shardrail mutual client handshake v1\0");
  for (const version of [1, 1, 1]) { const value = Buffer.alloc(4); value.writeUInt32BE(version); hash.update(value); }
  for (const field of fields) { const size = Buffer.alloc(4); size.writeUInt32BE(field.length); hash.update(size).update(field); }
  const message = new Uint8Array(hash.digest());
  assert.equal(Buffer.from(message).toString("hex"), vector.transcript_sha256);
  assert.equal(await ed.verifyAsync(Buffer.from(vector.client_signature, "hex"), message, Buffer.from(vector.client_public_key, "hex")), true);
  assert.equal(Buffer.from(await ed.signAsync(message, Buffer.from(vector.client_seed, "hex"))).toString("hex"), vector.client_signature);
});
