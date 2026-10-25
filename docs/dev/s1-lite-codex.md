# S1-lite Codex app-server

S1-lite connects one real `codex app-server` child per Shardrail session. The fake ACP provider stays the default. Codex is selected only with both `--codex-app-server` and `--codex-workdir`.

The adapter speaks newline JSON-RPC on the child's stdio. It does not pass `capabilities.experimentalApi` (D36). `thread/start` and `turn/start` ask for sandbox `workspace-write`, approval policy `untrusted`, and `approvalsReviewer: "user"`. The first live run used `on-request`: Codex 0.160.0 then applied an in-workspace patch to the test repo without sending any approval request, because `on-request` under `workspace-write` lets the model write inside the workspace and run sandboxed commands without asking. `untrusted` asks before every patch and every command outside Codex's small built-in list of read-only commands (such as `ls` or `cat`), which still run without a card. The daemon journals `approval_routing_rejected` and refuses later turns unless the thread response echoes user routing, the configured cwd, approval policy `untrusted` (an `on-request`, `never`, or granular echo is weaker and fails closed; a `turn/start` response that reports a different policy also fails closed), and a `workspaceWrite` or `readOnly` sandbox whose `networkAccess` is absent or false. A non-boolean `networkAccess` fails closed. `turn/start` in Codex CLI 0.160.0 does not return `approvalsReviewer`; a present non-user value still fails closed, and a missing value keeps the thread confirmation.

Each child is started in its own process group. The mandatory safety `-c` overrides, including `web_search="disabled"`, are appended after any extra config. `--codex-config` accepts only `model` and `model_reasoning_effort`, as a single-line `KEY=VALUE` with a non-empty value. SIGTERM or SIGINT of the daemon kills those process groups. SIGKILL of the daemon does not run that handler. A failed `session.create` stays rejected when the same command id is retried until the child has reached `process_started` or `approval_routing_rejected`.

If a `fileChange` item completes without an approval request for that item, the daemon journals `approval_routing_rejected`, sends `turn/interrupt`, and refuses later turns on that session. This only detects a bypass after the patch is on disk; the approval policy is the prevention.

Command and file-change approval requests become daemon approval cards. A limited client can allow once or deny. Those map to Codex `accept` or `decline`. `acceptForSession` is never sent. Timeout, disconnect, and unknown server requests fail closed. Unknown methods, including user-input, elicitation, and permission grants, get JSON-RPC `-32601` and a `provider_request_refused` journal event.

A limited browser may send `turn.start` and `turn.interrupt` on an existing session. Creating a session still requires a full client. The main screen prompt box is for that limited path. Its draft belongs to the selected session, stays in the field when the command is rejected, and clears when that prompt's `command_accepted` arrives.

## Try it in a normal browser

Build the daemon and the S5 assets, then start `shardraild` yourself. This uses the Codex login already on the machine. Do not pass an API key.

```sh
cd /Users/daliys/Daliys/Shardrail/spikes/wt-s1
nice cargo build -p shardraild
cd apps/s5-prototype
npm ci
npm run build
cd ../..
DATA=$(mktemp -d)
./target/debug/shardraild --data-dir "$DATA" --no-file-log \
  --codex-app-server "$(command -v codex)" \
  --codex-workdir /Users/daliys/Daliys/Shardrail/spikes/s1-testrepo \
  --codex-config 'model_reasoning_effort="low"' \
  --browser-listen 127.0.0.1:9731 \
  --browser-assets-dir apps/s5-prototype/dist
```

In another terminal, authorize one full client and create the session. A browser enrollment is limited and cannot create sessions.

```sh
./target/debug/shardraild --data-dir "$DATA" client add --label me --scope full
./target/debug/shardraild --data-dir "$DATA" identity
```

`client add` prints `client_id`, `public_key`, `scope`, and `host_public_key`, and writes the raw 32-byte key to `$DATA/client-<client_id>.key`. There is no `session create` CLI. From this worktree, with the daemon already running:

```sh
export DATA CLIENT_ID HOST_PUBLIC_KEY
node --experimental-strip-types --input-type=module -e '
import { readFileSync } from "node:fs";
import { connect } from "./packages/client/src/index.ts";
const commandId = crypto.randomUUID();
const client = await connect({
  socketPath: process.env.DATA + "/shardraild.sock",
  clientId: process.env.CLIENT_ID,
  privateKey: new Uint8Array(readFileSync(process.env.DATA + "/client-" + process.env.CLIENT_ID + ".key")),
  hostPublicKey: process.env.HOST_PUBLIC_KEY,
});
client.onFrame((frame) => {
  if (frame.kind === "durable_event" && frame.event.kind === "session_created" && frame.event.payload?.command_id === commandId) {
    console.log(frame.event.session_id);
    client.close();
  }
  if (frame.kind === "command_rejected" && frame.command_id === commandId) {
    console.error(frame.error);
    client.close();
  }
});
client.request({ command_id: commandId, kind: "session.create", expected_state: null, expected_turn_id: null, payload: {} });
'
```

Then enroll the browser:

```sh
./target/debug/shardraild --data-dir "$DATA" client enroll --label browser
```

Open `http://127.0.0.1:9731/`, review the fingerprint, enter the enrollment code, select the session, type a prompt, and use **Send prompt**, **Allow once** or **Deny**, and **Interrupt**.

Point `--codex-workdir` only at the throwaway repo `/Users/daliys/Daliys/Shardrail/spikes/s1-testrepo`. Reset it before a run you care about:

```sh
git -C /Users/daliys/Daliys/Shardrail/spikes/s1-testrepo reset --hard 1bb8838
git -C /Users/daliys/Daliys/Shardrail/spikes/s1-testrepo clean -fdx
```

Stop the daemon with SIGTERM so it kills the Codex process group.

## Live smoke

`scripts/s1-lite-live.mjs` is the checked run. It refuses to start unless `SHARDRAIL_S1_LIVE=1`. Before `git reset` or `git clean`, it requires `s1-testrepo` itself to be a directory rather than a symlink, `.git` to be that repo's real git directory rather than a symlink or gitdir file, no remote, and a git directory that is not Shardrail's. `git rev-parse --verify 1bb8838^{commit}` must succeed first. Reset uses that full hash, and HEAD must equal it afterward. A parent directory may be a symlink. The script starts the daemon on `127.0.0.1:0` with a temp data directory and `--no-file-log`, pairs a full socket client for `session.create` and a headless Chromium limited browser for the prompt, denies the first approval, allows the next once if one arrives, interrupts a second turn after the first new journaled `agent_message_chunk`, and writes `s1-lite-result.json`, `s1-lite-result.md`, and `daemon.log` under `S1_RESULT_DIR`. The transcript virtualizes rows, so that journal event is the delta signal. If the model stops after the denial, the run records allow-once as not observed and still succeeds. If an allow-once is recorded, the turn must finish `completed` or `Provider`. `S1_RESULT_DIR` must be outside both Shardrail checkouts. The script's hard timeout is 15 minutes. It was not executed in the implementation session.

```sh
export SHARDRAIL_S1_LIVE=1
export S1_RESULT_DIR=/tmp/s1-lite-result
node /Users/daliys/Daliys/Shardrail/spikes/wt-s1/scripts/s1-lite-live.mjs
```

Headless Chromium must be available as Playwright's `channel: 'chromium'`. The script runs `npm ci` in `apps/s5-prototype` when `playwright-core` is not installed, and `npm ci` in `packages/client` when `@noble/ed25519` is not installed there, then builds assets and `shardraild`. The client import needs those packages beside `packages/client`; `NODE_PATH` does not apply to ESM.

## What the automated tests cover without Codex

`crates/shardraild/tests/codex.rs` drives `tests/fixtures/fake_codex_app_server.py`. The fixture follows the pinned schema in `tests/fixtures/codex-app-server-0.160.0/` (generated by `codex app-server generate-json-schema` for CLI 0.160.0, without experimental methods). The happy-path trace is checked against that schema with an in-repo draft-07 subset validator. The tests check the spawn overrides, initialize without `experimentalApi`, thread and turn parameters, fail-closed reviewer, approval-policy and sandbox echoes, a replayed on-request auto-write (`tests/fixtures/codex-0.160.0-on-request-autowrite.jsonl`) that must interrupt and block the session including `readOnly` with `networkAccess: true`, redaction of a high-entropy secret, deny then allow-once, no `acceptForSession`, unknown-request refusal, interrupt, child crash, approval timeout, disconnect during an approval, an invalid turn `session_id` that rejects without dropping the socket, a refused MCP `--codex-config`, a failed `session.create` retry, and SIGTERM killing the child process group.
