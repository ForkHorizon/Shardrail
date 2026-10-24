# Daemon logs

`shardraild` writes `shardraild.log` under `~/Library/Logs/Shardrail` on macOS. On Linux it uses `$XDG_STATE_HOME/shardrail/logs`, or `~/.local/state/shardrail/logs` when `XDG_STATE_HOME` is unset. Set `SHARDRAIL_LOG_DIR` or pass `--log-dir DIR` to override the directory. `--no-file-log` disables file logging. Linux processes with `JOURNAL_STREAM` or `INVOCATION_ID` use stderr for systemd journal collection.

The log directory is mode `0700` and files are mode `0600` on Unix. The active file is limited to 1 MiB; rotation keeps four numbered files plus the active file, for a 5 MiB total cap. Rotation and writes are serialized within the daemon process. If setup or writing fails, file logging is disabled and a single failure is reported to stderr; operational lines continue to stderr.

Log lines use UTC RFC3339 timestamps, a level, and a component. Messages are kept short and omit arbitrary protocol/provider error bodies. A best-effort scrubber removes lines containing sensitive field names or known credential prefixes and masks high-entropy token-like runs. It can still miss unusual or low-entropy secrets; logs should be treated as sensitive.

Linux's file-log destination is the proposed default from W7-b and remains pending owner confirmation, because planning 02 §7 currently leaves it undecided. systemd detection is best effort through the standard journal environment variables.
