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

Vite binds to `127.0.0.1`; use the daemon-served build for pairing and transport tests. Enrollment consumption and client registry replacement sync their containing directory. Existing authenticated sockets recheck the client registry before each frame and close after local revocation. Browser event cursors and journal epochs persist per host in local storage so reconnect and reload resume from the last delivered event; corrupt cursor state is ignored and replays from zero.

Run browser acceptance tests only in headless Chromium and WebKit. Install the browsers in the isolated cache, then run the npm script (it builds the Vite SPA and starts a temporary daemon on an OS-assigned loopback port):

```sh
cd apps/s5-prototype
PLAYWRIGHT_BROWSERS_PATH=/private/tmp/shardrail-pw ./node_modules/.bin/playwright install --no-shell chromium webkit
PLAYWRIGHT_BROWSERS_PATH=/private/tmp/shardrail-pw npm run test:e2e
```

The Playwright config forces `headless: true` for both projects. The flow checks local-command enrollment, fingerprint confirmation, WebCrypto and forced Noble fallback, reload persistence, reconnect, event delivery, changed-key blocking and a WebSocket attempt from another origin. No headed browser, UI mode or trace viewer is used.
