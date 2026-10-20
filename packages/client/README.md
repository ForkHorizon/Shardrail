# @shardrail/client

Node client for the local `shardraild` Unix socket. Frames and event DTOs come only from the generated `@shardrail/contracts` package. The client performs the v1 transcript-bound Ed25519 handshake, requires a pinned daemon host key, sends command IDs, resumes from a cursor, and exposes typed daemon frames. Call `reconnect()` after a dropped connection to resume from the latest delivered cursor; `onError()` reports socket failures.

The handshake uses `@noble/ed25519` **3.1.0**, pinned exactly to D62. Keep the private key seed in the owner-only key file created by `shardraild client add`; do not put it in browser storage or a URL. The browser UI uses the Tauri shell relay because browsers cannot open Unix sockets.

```ts
import { readFile } from "node:fs/promises";
import { connect } from "@shardrail/client";

const client = await connect({
  socketPath: "/path/to/data/shardraild.sock",
  clientId: "registered-client-uuid",
  privateKey: new Uint8Array(await readFile("/path/to/data/client-<id>.key")),
  hostPublicKey: "pinned-host-public-key-hex",
});
client.onFrame((frame) => console.log(frame.kind));
```

`npm run typecheck` runs strict `tsc --noEmit`. `npm test` uses Node's built-in TypeScript stripping and tests against the real daemon and fake ACP binary.
