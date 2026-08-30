# ADR 0033: Structured native logging with redaction

## Status

Implemented for the desktop native runtime.

## Decision

MobaRust uses `tracing` with a `tracing-subscriber` formatter initialized by
the Rust entry point. The default filter is `mobarust=warn`; operators may
raise verbosity with the normal `RUST_LOG` filter when diagnosing a local
build. Events use named fields such as `operation`, `target_kind`, and
`platform` instead of interpolated log sentences.

The subscriber writes to stderr only. MobaRust does not create a log file,
capture clipboard content, or export terminal output through this logger.
The separate audit store remains a bounded lifecycle journal with its own
secret-free schema.

## Redaction boundary

Credential lookups emit only a fixed `<redacted>` marker. Passwords,
passphrases, private-key bytes, credential references, agent handles,
environment values, terminal input, and remote output are never event fields.
Dynamic protocol errors remain application responses rather than being copied
into logs automatically, because an underlying dependency may include a path
or remote diagnostic that is not safe to classify generically.

This protects MobaRust's logging behavior, not a compromised operating system:
stderr may still be collected by a parent process, and crash dumps are outside
the logger's control.

## Verification

Unit tests verify the redacted formatter. Workspace Clippy rejects unused or
invalid logging code, while the safe test launcher strips ambient SSH-agent,
askpass, and Git SSH variables from test subprocesses.
