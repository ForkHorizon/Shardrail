# @shardrail/client

This package currently re-exports the generated wire types. Its `connect()` is
a typed stub: browser JavaScript cannot open the local Unix socket, and the
Tauri relay/short-lived shell token transport has not been implemented. Do not
pass a daemon credential through browser storage or a URL.

The N2 daemon currently accepts local same-UID development connections only.
Asymmetric key proof and pairing remain TODOs until an approved crypto
dependency and key storage flow are selected.
