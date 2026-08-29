# MobaRust

MobaRust is a free, open-source remote workstation for people who live in terminals, bastions, and remote filesystems. It is built Rust-first, with a Tauri desktop shell and a focused operator UI.

> The free, open-source MobaXterm alternative built with Rust.

## Current vertical slice

The first working slice provides:

- a real local PTY owned by Rust and rendered through xterm.js;
- a real `russh` transport crate with restrictive host-key policy, interactive SSH PTY, and streaming SFTP primitives;
- a native Quick Connect path for a real SSH shell with typed write, resize, and close commands;
- an SFTP directory browser over the live SSH connection with single-file upload/download, bounded concurrency, progress events, explicit overwrite handling, cancellation, and temporary-file commits;
- a native SCP compatibility primitive for streaming single-file upload/download, verified against the isolated local SSH fixture;
- a native Telnet transport and Quick Connect path with bounded option negotiation, configurable terminal encoding, reconnect/cancel lifecycle, resize support, and a local TCP fixture; Telnet is clearly unencrypted;
- a native serial transport primitive with explicit line parameters, bounded driver I/O, line-ending framing, and recoverable device-loss errors; hardware access is explicit and not exercised by tests;
- a bounded native TCP diagnostic primitive with explicit targets, port ranges, concurrency, timeouts, cancellation, and loopback-only fixtures;
- a native credential-vault boundary using platform credential stores without exposing secrets to React;
- a reproducible local `sshd` integration fixture covering host-key rejection, key authentication, PTY I/O, and streaming transfer;
- a stateful connection/session model with explicit lifecycle transitions;
- a versioned, secret-free saved-session store with typed Tauri list/save/delete commands;
- favorite and tag-aware session organization with secret-free MobaRust catalog import/export;
- explicit SSH session saving after Quick Connect and clickable saved-session reconnect using stored host-trust and credential references;
- real SSH jump-host chaining through native `direct-tcpip` streams, with an optional agent-backed hop in Quick Connect;
- bounded SSH shell reconnection with explicit reconnecting/failed state events and preserved terminal identity;
- native SSH local port forwarding through direct-tcpip channels, with bounded client concurrency, lifecycle events, byte counts, and cooperative cancellation;
- native SSH remote forwarding and a bounded local SOCKS5 `-D` proxy path with typed tunnel-manager commands;
- a transfer lifecycle model used by the native SFTP manager;
- a high-signal workspace shell for sessions, tunnels, transfers, diagnostics, and local terminals;
- an explicit SSH tunnel manager for bounded local forwarding, remote `-R` forwarding, and local SOCKS5 `-D`, with direction-aware lifecycle events and stop controls;
- a local quality command: `cargo xtask check`.
- a versioned, bounded RDP/VNC helper-process contract with lifecycle and
  redaction tests; this is not yet a real RDP/VNC client.

The native SSH/SFTP/SCP transport, local/remote/dynamic forwarding paths and manager UI, jump-host handshake, cancellation path, Quick Connect path, Telnet session path, protocol fixture, serial session path, serial configuration/lifecycle tests, bounded TCP diagnostics, and explicit DNS/TCP/port-scan diagnostics view are covered locally. The bounded reconnect worker uses the same native SSH transport but still needs dedicated failure-injection coverage before release. Keyboard-interactive auth and vault-backed CRUD are still in progress. Native file/directory pickers, recursive transfers, SCP transfer-manager wiring, serial device refresh, OpenSSH `ProxyJump` profile resolution, RDP/VNC, and the remaining protocol adapters are intentionally staged behind these primitives. See [the roadmap](ROADMAP.md) and [architecture decisions](docs/adr/0001-rust-first-tauri.md).

## Development

```bash
pnpm install --dir apps/desktop
cargo xtask check
pnpm --dir apps/desktop tauri dev
```

The desktop binary is named `mobarust`.

Reference projects live under `base/` for local research only and are excluded from Git. See [the research log](docs/research/reference-projects.md).

## License

Apache-2.0. See [LICENSE](LICENSE).
