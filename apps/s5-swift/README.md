# Shardrail Swift AppKit spike

A macOS 14+ SwiftPM experiment with separate `ShardrailCore` library and `shardrail-swift` AppKit executable targets. It authenticates to the same local `shardraild` Unix socket using the v1 transcript-bound Ed25519 handshake, stores all received rows in memory, and presents them in a fixed-height reused `NSTableView`. Updates are coalesced on display frames. B4 samples daemon agent chunks at the second display-frame callback after a visible row commit; it requires a visible window, all requested chunks, and `max(20, ceil(0.5% of events))` distinct row samples (capped at the event count).

The five-sample wall/monotonic mapping uses the mean offset and half the observed offset range plus 1 ms uncertainty, matching the web prototype. B4 is a paint-opportunity proxy and does not prove completed paint or scanout. Full event history has no retention cap in this spike; memory retention policy remains an open question. B5 input measurements are deferred.

## Build and headless tests

```sh
cd apps/s5-swift
DEVELOPER_DIR=/Applications/Xcode-beta.app/Contents/Developer swift build -c release
DEVELOPER_DIR=/Applications/Xcode-beta.app/Contents/Developer swift test
```

Tests run library logic only. They do not create windows, run an `NSApplication` loop, or start display links.

## B4 night run

Build the daemon and fake agent first with `cargo build --release -p shardraild -p shardrail-fake-acp`; build this package with `swift build -c release`. Validate configuration without launching processes:

```sh
bash apps/s5-swift/scripts/b4-bench-swift.sh --check
```

Run the full 60-run matrix (5k/50k × 1/4/8 streams × cold/warm × 5 repetitions, 300 seconds per run):

```sh
bash apps/s5-swift/scripts/b4-bench-swift.sh
```

One visible-window smoke run:

```sh
bash apps/s5-swift/scripts/b4-bench-swift.sh --smoke
```

Results and logs go to `apps/s5-prototype/bench-results/w4k2-swift/`, alongside the existing B4 run artifacts. The runner tracks only its app and daemon PIDs and terminates their descendant process trees. `--check` checks the configured executable paths and timeout, then exits without creating directories or starting anything.

## Licensing

The app source is proprietary (`LicenseRef-Proprietary`, matching the current repository scope in planning/09). It has no third-party dependencies; CryptoKit, AppKit, CoreVideo, Foundation, and Darwin are Apple system frameworks and remain governed by Apple's terms.
