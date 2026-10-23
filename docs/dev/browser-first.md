# Browser listener development

The browser WebSocket listener is disabled by default. Start `shardraild` with `--browser-listen 127.0.0.1:PORT` to enable it; it accepts connections only at `ws://127.0.0.1:PORT/ws` with matching `Host` and `Origin` headers. The React prototype now has a browser pairing path and a browser-safe client entry point. The same-origin SPA serving path is not implemented yet, so a Vite page on its default separate port cannot connect under the exact-origin policy. Issue a one-time browser enrollment code from a local shell:

```sh
shardraild client enroll --label browser
```

The command prints a 256-bit code that expires in five minutes. Compare the browser pairing fingerprint with `host_fingerprint` from `shardraild identity`, then enter the code in the browser pairing screen. The browser sends the code only in a WebSocket protocol frame, alongside its public key. Redemption is serialized with client registry changes, accepts at most five invalid attempts, consumes the code before registration, and creates a `limited` client. Replay fails. The browser stores non-extractable WebCrypto Ed25519 keys in IndexedDB when supported; otherwise it discloses that the pinned Noble fallback stores an extractable seed in IndexedDB. A changed pinned daemon key blocks pairing. Use the local `client add --scope full` command for full-scope clients. Treat the code like a short-lived secret and do not paste it into a URL.

The UI dev server is separate for now:

```sh
npm --prefix apps/s5-prototype run dev
```

Vite binds to `127.0.0.1`. It remains useful for UI work, but its page origin must match the daemon listener port before browser transport can work. Daemon-served static assets are deferred until the daemon's build/install flow defines where the versioned Vite artifact lives; adding an implicit source-tree path now would make local builds appear to serve a production SPA while installed daemons have no artifact. The Tauri pairing/relay path remains available. Rust WebSocket acceptance integration tests and headless Chromium/WebKit Playwright flows also remain open.
