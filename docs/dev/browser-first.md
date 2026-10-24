# Browser listener development

The browser listener is disabled by default. Build the SPA and start `shardraild` with `--browser-listen 127.0.0.1:PORT --browser-assets-dir PATH/TO/dist` to serve it and its assets on the same loopback origin as `ws://127.0.0.1:PORT/ws`. The daemon serves only files inside that explicit build directory; static requests require the exact loopback Host and reject a mismatched Origin, while WebSocket upgrades require the exact Host and Origin. The Vite dev server uses a separate origin and is not supported for browser transport. The operator must supply the built artifact path; its packaging, versioning and install location remain a later decision. Issue a one-time browser enrollment code from a local shell:

```sh
shardraild client enroll --label browser
```

The command prints a 256-bit code that expires in five minutes. Compare the browser pairing fingerprint with `host_fingerprint` from `shardraild identity`, then enter the code in the browser pairing screen. The browser sends the code only in a WebSocket protocol frame, alongside its public key. Redemption is serialized with client registry changes, accepts at most five invalid attempts, consumes the code before registration, and creates a `limited` client. Replay fails. The browser stores non-extractable WebCrypto Ed25519 keys in IndexedDB when supported; otherwise it discloses that the pinned Noble fallback stores an extractable seed in IndexedDB and requires acknowledgement before sending the enrollment code. A changed pinned daemon key blocks pairing. Use the local `client add --scope full` command for full-scope clients. Treat the code like a short-lived secret and do not paste it into a URL.

For UI-only work, the Vite dev server remains available:

```sh
npm --prefix apps/s5-prototype run dev
```

Vite binds to `127.0.0.1`; use the daemon-served build for pairing and transport tests. Enrollment consumption and client registry replacement sync their containing directory. Existing authenticated sockets recheck the client registry before each frame and close after local revocation. Each page load replays the journal from cursor 0 so the session list is complete. An in-page reconnect resumes from the highest cursor already applied and drops a repeated cursor. The main screen does not write that cursor to local storage, because a saved cursor is not a session list and reloading from it would hide sessions. A changed journal epoch clears both the reconnect dedupe set and the on-screen session projection, then accepts the new journal’s cursors. Corrupt cursor state on the older load screen is ignored.

## Main screen

After pairing, the normal browser (not the Tauri shell and not a `?b4` load run) shows one local machine:

- Sessions are derived from the durable journal. Each row shows the session id, the agent kind when `process_started` names a provider, a lifecycle/turn/process status from the latest session snapshot, and the last activity time. A later event without a snapshot does not wipe that status.
- Choosing a session shows its events in the existing virtualized list. Consecutive `agent_message_chunk` events in one turn are one transcript item, so Markdown that spans chunks is sanitized once. The list follows the tail until the reader scrolls up, and switching sessions resets the scroll position. Other event kinds are plain JSON text, not HTML. Provider output is never inserted as HTML. An agent link opens only after confirmation, including middle-click, and a non-http(s) link is not given an href.
- The connection line reads connecting, connected, reconnecting, offline, or key-changed. A dropped socket retries with backoff from 200 ms, doubling to 8 s, and a Reconnect button starts that attempt immediately and resets the backoff. Offline (`navigator.onLine === false`) pauses retries. A changed host key stops automatic retries.
- The host banner says “approvals not protected”. Each session shows the verification checklist with every item `not verified`, because this browser build does not run those checks. The list is not collapsed into one verdict.
- Pending approvals for the selected session offer Deny and Allow once only, and only for options the journal marked `once`. The card’s primary area shows structured command, argv, raw input, cwd, locations, changes, and network fields when the journal has them, plus the sandbox axis or “not disclosed”. Agent title and description stay in the untrusted description box. If none of those action fields are present, the card says “action not disclosed by provider” and Allow once requires a second confirmation. Buttons disable after a response is sent. A disconnect, a rejected frame, or a failed send does not display or retry an allow. A confirmed `approval_resolved` frame or a later journal state clears the in-flight marker, so a later disconnect does not claim that confirmed response was lost. The card state comes from later journal events (`responding`, `responded_unacked`, `allowed`, `denied`, `expired`, `withdrawn`), not from the acknowledgement alone.
- The browser client is `limited`. It cannot create sessions. The load-test screen remains the Tauri and `?b4` path. There is no inbox, composer, multi-machine view, or hub. The checklist is honest and unverified; it is not a runtime sandbox or routing check.

The fake agent used by tests exits after a permission reply. `allowed`/`denied` is published to live clients when the approval is acknowledged, and `process_exited` is published when the fixture run finishes. That is a daemon lifecycle fix, not a protocol change. The agent chunk emitted after the permission outcome is still ignored by the daemon's post-response read loop. The fake permission request does not include a command, argv, or cwd, so the approval card takes the undisclosed-action path.

Run browser acceptance tests only in headless Chromium and WebKit. Install the browsers in the isolated cache, then run the npm script (it builds the Vite SPA and starts a temporary daemon on an OS-assigned loopback port):

```sh
cd apps/s5-prototype
PLAYWRIGHT_BROWSERS_PATH=/private/tmp/shardrail-pw ./node_modules/.bin/playwright install --no-shell chromium webkit
PLAYWRIGHT_BROWSERS_PATH=/private/tmp/shardrail-pw npm run test:e2e
```

The Playwright config forces `headless: true` for both projects. Pairing coverage checks local-command enrollment, fingerprint confirmation, WebCrypto and forced Noble fallback, reload persistence, reconnect, event delivery, changed-key blocking and a WebSocket attempt from another origin. Main-screen coverage pairs, lists the session, opens it from the keyboard, streams fake-agent text, denies an approval, confirms Allow once only after the journal says so, refuses an allow while the socket is down, checks the screen with axe, and reconnects after the daemon process is replaced without duplicating events. The test daemon is started with `--fake-acp-path`. No headed browser, UI mode or trace viewer is used.
