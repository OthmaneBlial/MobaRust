# MobaRust

MobaRust is a free, open-source remote workstation for people who live in terminals, bastions, and remote filesystems. It is built Rust-first, with a Tauri desktop shell and a focused operator UI.

> The free, open-source MobaXterm alternative built with Rust.

## Current vertical slice

The first working slice provides:

- a real local PTY owned by Rust and rendered through xterm.js;
- a real `russh` transport crate with restrictive host-key policy, interactive SSH PTY, and streaming SFTP primitives;
- password, private-key, agent, and bounded keyboard-interactive SSH authentication, with credential references resolved only in Rust;
- a native Quick Connect path for a real SSH shell with typed write, resize, and close commands;
- an SFTP directory browser over the live SSH connection with single-file upload/download, bounded concurrency, progress events, explicit overwrite handling, cancellation, and temporary-file commits;
- a native SCP compatibility path in the bounded transfer manager for single-file upload/download, with explicit protocol selection, progress, cancellation, and atomic per-file commits; recursive jobs remain SFTP;
- a native Telnet transport and Quick Connect path with bounded option negotiation, configurable terminal encoding, reconnect/cancel lifecycle, resize support, and a local TCP fixture; Telnet is clearly unencrypted;
- a native serial transport primitive with explicit line parameters, bounded driver I/O, line-ending framing, and recoverable device-loss errors; hardware access is explicit and not exercised by tests;
- an explicit serial-port refresh action and secret-free saved serial profiles that can be reopened with their line parameters;
- a bounded native TCP diagnostic primitive with explicit targets, port ranges, concurrency, timeouts, cancellation, and loopback-only fixtures;
- bounded platform-native ping and traceroute diagnostics with explicit targets, hop/timeout limits, process cancellation, and output truncation;
- explicit unauthenticated SSH host-key fingerprint inspection with a bounded timeout and no credential, agent, or known_hosts access;
- a native credential-vault boundary using platform credential stores without exposing secrets to React;
- an opt-in portable encrypted vault using Argon2id + AES-256-GCM, atomic private-file writes, explicit unlock/lock, and native-only secret lookup;
- a one-shot SSH remote system monitor using a fixed read-only query, bounded output, a six-second timeout, and graceful per-metric capability detection;
- a reproducible local `sshd` integration fixture covering host-key rejection, key authentication, PTY I/O, and streaming transfer;
- a stateful connection/session model with explicit lifecycle transitions;
- a versioned, secret-free saved-session store with typed Tauri list/save/delete commands;
- favorite and tag-aware session organization with secret-free MobaRust catalog import/export;
- explicit SSH session saving after Quick Connect and clickable saved-session reconnect using stored host-trust and credential references;
- real SSH jump-host chaining through native `direct-tcpip` streams, with saved hop descriptors and imported `ProxyJump` alias resolution when matching profiles exist;
- bounded SSH shell reconnection with explicit reconnecting/failed state events and preserved terminal identity;
- native SSH local port forwarding through direct-tcpip channels, with bounded client concurrency, lifecycle events, byte counts, and cooperative cancellation;
- native SSH remote forwarding and a bounded local SOCKS5 `-D` proxy path with typed tunnel-manager commands;
- bounded recursive SFTP upload/download with streaming file bodies, progress, cancellation, symlink refusal, and atomic per-file commits;
- terminal multiline paste is intercepted and confirmed visibly before it is sent to a remote or local shell;
- typed non-secret settings are persisted separately with validation, atomic writes, reset, theme selection, terminal profile controls, reconnect policy, and bounded diagnostic defaults;
- a secret-free snippet library with tags, validated `${variable}` placeholders, rendered preview, and explicit manual clipboard copy (never automatic execution);
- a bounded macro runner and explicit broadcast-input mode with typed actions, target preflight, visible progress, cooperative cancellation, and an `Esc` emergency disable;
- a transfer lifecycle model used by the native SFTP manager;
- a high-signal workspace shell for sessions, tunnels, transfers, diagnostics, and local terminals;
- persistent terminal tabs and two-pane splits for simultaneous local, SSH, Telnet, and serial sessions, with per-tab event routing and lifecycle cleanup;
- a session-scoped SSH workspace that keeps the terminal and SFTP browser on the same native connection, with explicit palette, tab, and return-to-terminal navigation;
- a bounded remote text editor with UTF-8/Windows-1252 encoding selection, local search/replace, SHA-256 conflict detection, mode preservation, and rollback-safe temporary-file promotion;
- an explicit SSH tunnel manager for bounded local forwarding, remote `-R` forwarding, and local SOCKS5 `-D`, with direction-aware lifecycle events and stop controls;
- a local quality command: `cargo xtask check`.
- a documented safe-testing policy that keeps protocol fixtures on loopback
  and temporary paths, without reading personal SSH material.
- a versioned, bounded RDP/VNC helper-process contract with lifecycle and
  redaction tests, plus isolated IronRDP and `vnc-rs` adapter experiments under
  `tools/rdp-helper` and `tools/vnc-helper`; these are not yet packaged or
  production desktop clients.

The native SSH/SFTP/SCP transport and transfer-manager paths, local/remote/dynamic forwarding paths and manager UI, bounded recursive SFTP transfers, jump-host handshake and saved-profile alias resolution, cancellation path, Quick Connect path, Telnet session path, explicit serial refresh/profile flow, secret-free snippets and macros, explicit native vault reference save/delete flow, opt-in portable encrypted vault flow, protocol fixtures, serial configuration/lifecycle tests, bounded TCP diagnostics, bounded native ping/traceroute, explicit DNS/TCP/port-scan diagnostics view, and unauthenticated SSH host-key fingerprint inspection are covered locally. The bounded reconnect worker uses the same native SSH transport but still needs dedicated failure-injection coverage before release. Native file/directory pickers, hardware interoperability, RDP/VNC, and the remaining protocol adapters are intentionally staged behind these primitives. See [the roadmap](ROADMAP.md) and [architecture decisions](docs/adr/0001-rust-first-tauri.md).

## Development

```bash
pnpm install --dir apps/desktop
cargo xtask check
pnpm --dir apps/desktop tauri dev
```

The desktop binary is named `mobarust`.

Reference projects live under `base/` for local research only and are excluded from Git. See [the research log](docs/research/reference-projects.md).

Portable mode is opt-in: a distribution must place an empty `portable.flag`
beside the executable. MobaRust then keeps non-secret application data and the
separate encrypted `portable-data/vault.bin` beside it; it never turns normal
installed or development runs into portable mode automatically.

## License

Apache-2.0. See [LICENSE](LICENSE).
