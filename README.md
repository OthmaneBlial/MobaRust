# MobaRust

MobaRust is a free, open-source remote workstation for people who live in terminals, bastions, and remote filesystems. It is built Rust-first, with a Tauri desktop shell and a focused operator UI.

> The free, open-source MobaXterm alternative built with Rust.

## Current vertical slice

The first working slice provides:

- a real local PTY owned by Rust and rendered through xterm.js;
- a real `russh` transport crate with restrictive host-key policy, interactive SSH PTY, and streaming SFTP primitives;
- a native Quick Connect path for a real SSH shell with typed write, resize, and close commands;
- an SFTP directory browser over the live SSH connection with single-file upload/download, bounded concurrency, progress events, explicit overwrite handling, cancellation, and temporary-file commits;
- a native credential-vault boundary using platform credential stores without exposing secrets to React;
- a reproducible local `sshd` integration fixture covering host-key rejection, key authentication, PTY I/O, and streaming transfer;
- a stateful connection/session model with explicit lifecycle transitions;
- a versioned, secret-free saved-session store with typed Tauri list/save/delete commands;
- favorite and tag-aware session organization with secret-free MobaRust catalog import/export;
- explicit SSH session saving after Quick Connect and clickable saved-session reconnect using stored host-trust and credential references;
- real SSH jump-host chaining through native `direct-tcpip` streams, with an optional agent-backed hop in Quick Connect;
- bounded SSH shell reconnection with explicit reconnecting/failed state events and preserved terminal identity;
- native SSH local port forwarding through direct-tcpip channels, with bounded client concurrency, lifecycle events, byte counts, and cooperative cancellation;
- a transfer lifecycle model used by the native SFTP manager;
- a high-signal workspace shell for sessions, tunnels, transfers, and local terminals;
- a local quality command: `cargo xtask check`.

The native SSH/SFTP transport, local forwarding channel, jump-host handshake, cancellation path, and Quick Connect path are tested against a local `sshd` fixture. The bounded reconnect worker uses the same native transport but still needs dedicated failure-injection coverage before release. Saved-session editing, keyboard-interactive auth, and vault-backed CRUD are still in progress. Native file/directory pickers, recursive transfers, SCP, remote/dynamic forwarding, OpenSSH `ProxyJump` profile resolution, RDP/VNC, and the remaining protocol adapters are intentionally staged behind these primitives. See [the roadmap](ROADMAP.md) and [architecture decisions](docs/adr/0001-rust-first-tauri.md).

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
