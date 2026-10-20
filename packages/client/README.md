# @shardrail/client

This package currently re-exports the generated wire types. Its `connect()` is
a typed stub: browser JavaScript cannot open the local Unix socket, and the
Tauri relay/short-lived shell token transport has not been implemented. Do not
pass a daemon credential through browser storage or a URL.

The daemon development CLI manages keys with:

```sh
shardraild client add --label <label> --scope full|limited
shardraild client list
shardraild client remove <client-id>
```

Private keys stay in `0600` files in the `0700` data directory. A pairing UI
and native client transport remain TODOs; the browser stub does not connect to
the daemon.
