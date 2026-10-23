# Browser listener development

The browser WebSocket listener is disabled by default. Start `shardraild` with `--browser-listen 127.0.0.1:PORT` to enable it; it accepts connections only at `ws://127.0.0.1:PORT/ws` with matching `Host` and `Origin` headers. The UI dev server is separate for now:

```sh
npm --prefix apps/s5-prototype run dev
```

Vite binds to `127.0.0.1`. The current prototype still requires Tauri commands for pairing and relay; browser pairing and daemon-served static assets are pending W5-a work.
