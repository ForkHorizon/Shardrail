# S1-lite Codex app-server

S1-lite connects one real `codex app-server` child per Shardrail session. The fake ACP provider stays the default. Codex is selected only with both `--codex-app-server` and `--codex-workdir`.

The adapter speaks newline JSON-RPC on the child's stdio. It does not pass `capabilities.experimentalApi` (D36). `thread/start` asks for sandbox `workspace-write`, approval policy `on-request`, and `approvalsReviewer: "user"`. The daemon journals `approval_routing_rejected` and refuses later turns unless the thread response echoes user routing, the configured cwd, an asking approval policy, and a `workspaceWrite` or stricter `readOnly` sandbox. `turn/start` in Codex CLI 0.160.0 does not return `approvalsReviewer`; a present non-user value still fails closed, and a missing value keeps the thread confirmation.

Each child is started in its own process group with the safety `-c` overrides last (last-wins), including `web_search="disabled"`. Extra `--codex-config KEY=VALUE` values are applied first. SIGTERM or SIGINT of the daemon kills those process groups. SIGKILL of the daemon does not run that handler.

Command and file-change approval requests become daemon approval cards. A limited client can allow once or deny. Those map to Codex `accept` or `decline`. `acceptForSession` is never sent. Timeout, disconnect, and unknown server requests fail closed. Unknown methods, including user-input, elicitation, and permission grants, get JSON-RPC `-32601` and a `provider_request_refused` journal event.

A limited browser may send `turn.start` and `turn.interrupt` on an existing session. Creating a session still requires a full client. The main screen prompt box is for that limited path.

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

Use the printed host key with a full local client, or the Node client in `packages/client`, to send `session.create`. Then enroll the browser:

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

`scripts/s1-lite-live.mjs` is the checked run. It refuses to start unless `SHARDRAIL_S1_LIVE=1`. It resets only `s1-testrepo` (no remote, not the Shardrail git directory, HEAD `1bb8838`), starts the daemon on `127.0.0.1:0` with a temp data directory and `--no-file-log`, pairs a full socket client for `session.create` and a headless Chromium limited browser for the prompt, denies the first approval, allows the next once, interrupts a second turn after the first new journaled `agent_message_chunk`, and writes `s1-lite-result.json`, `s1-lite-result.md`, and `daemon.log` under `S1_RESULT_DIR`. The transcript virtualizes rows, so that journal event is the delta signal. If the model asks for only one approval, the script records the deny and exits non-zero because allow-once was not observed. `S1_RESULT_DIR` must be outside both Shardrail checkouts. The script's hard timeout is 15 minutes. It was not executed in the implementation session.

```sh
export SHARDRAIL_S1_LIVE=1
export S1_RESULT_DIR=/tmp/s1-lite-result
node /Users/daliys/Daliys/Shardrail/spikes/wt-s1/scripts/s1-lite-live.mjs
```

Headless Chromium must be available as Playwright's `channel: 'chromium'`. The script runs `npm ci` in `apps/s5-prototype` when `playwright-core` is not installed, and `npm ci` in `packages/client` when `@noble/ed25519` is not installed there, then builds assets and `shardraild`. The client import needs those packages beside `packages/client`; `NODE_PATH` does not apply to ESM.

## What the automated tests cover without Codex

`crates/shardraild/tests/codex.rs` drives `tests/fixtures/fake_codex_app_server.py`. The fixture follows the pinned schema in `tests/fixtures/codex-app-server-0.160.0/` (generated by `codex app-server generate-json-schema` for CLI 0.160.0, without experimental methods). The tests check the spawn overrides, initialize without `experimentalApi`, thread and turn parameters, fail-closed reviewer and sandbox echoes, redaction of a high-entropy secret, deny then allow-once, no `acceptForSession`, unknown-request refusal, interrupt, child crash, approval timeout, disconnect during an approval, and SIGTERM killing the child process group.
