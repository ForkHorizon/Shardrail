# Browser listener development

The browser WebSocket listener is disabled by default. Start `shardraild` with `--browser-listen 127.0.0.1:PORT` to enable it; it accepts connections only at `ws://127.0.0.1:PORT/ws` with matching `Host` and `Origin` headers. Issue a one-time browser enrollment code from a local shell:

```sh
shardraild client enroll --label browser
```

The command prints a 256-bit code that expires in five minutes. Enter it in the browser pairing screen; the browser sends it only in a WebSocket protocol frame, alongside its public key. Redemption is serialized with client registry changes, accepts at most five invalid attempts, consumes the code before registration, and creates a `limited` client. Replay fails. Use the local `client add --scope full` command for full-scope clients. Treat the code like a short-lived secret and do not paste it into a URL.

The UI dev server is separate for now:

```sh
npm --prefix apps/s5-prototype run dev
```

Vite binds to `127.0.0.1`. The current prototype still requires Tauri commands for pairing and relay. Browser key storage/transport, the shared browser pairing UI, daemon-served static assets, and browser automation remain pending W5-a work.
