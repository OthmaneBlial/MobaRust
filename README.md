# MobaRust — a free, open-source MobaXterm alternative for SSH and remote operations

<p align="center">
  <img src="apps/desktop/src-tauri/icons/icon.svg" alt="MobaRust logo" width="112" />
</p>

<p align="center">
  <strong>One focused desktop workspace for every machine you operate.</strong><br />
  SSH, SFTP, SCP, terminals, tunnels, diagnostics, and remote operations — built with Rust and Tauri.
</p>

<p align="center">
  <a href="https://othmaneblial.github.io/MobaRust/">Website</a> ·
  <a href="https://othmaneblial.github.io/MobaRust/docs.html">Documentation</a> ·
  <a href="ROADMAP.md">Roadmap</a> ·
  <a href="https://github.com/OthmaneBlial/MobaRust/issues">Feedback</a>
</p>

<p align="center">
  <a href="https://github.com/OthmaneBlial/MobaRust/blob/main/LICENSE"><img src="https://img.shields.io/github/license/OthmaneBlial/MobaRust?style=flat-square&label=license" alt="Apache 2.0 license" /></a>
  <a href="https://github.com/OthmaneBlial/MobaRust/stargazers"><img src="https://img.shields.io/github/stars/OthmaneBlial/MobaRust?style=flat-square&label=stars" alt="GitHub stars" /></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/built%20with-Rust-orange?style=flat-square&logo=rust" alt="Built with Rust" /></a>
  <a href="https://tauri.app/"><img src="https://img.shields.io/badge/desktop-Tauri-24c8db?style=flat-square" alt="Tauri desktop application" /></a>
</p>

> MobaRust is a free, open-source MobaXterm alternative for developers, DevOps engineers, system administrators, infrastructure teams, and homelab users who want one transparent desktop app for everyday remote work — without a mandatory subscription or cloud account.

MobaRust is an independent project and is not affiliated with Mobatek or MobaXterm. MobaXterm is a trademark of its respective owner.

MobaRust is a real desktop application, not a website pretending to be a terminal. React and TypeScript render the interface inside Tauri; Rust owns the native networking, PTY, filesystem, process, and credential boundaries.

## Why choose MobaRust?

Remote work should not require one app for SSH, another for file transfer, another for tunnels, and a fourth place for session notes. MobaRust brings the daily operator loop into one keyboard-friendly desktop application:

**Connect. Inspect. Transfer. Automate carefully. Disconnect cleanly.**

- **Free and open source** — Apache-2.0 licensed, inspectable, forkable, and built in public.
- **One operator workspace** — Keep terminals, remote files, tunnels, diagnostics, snippets, and session context together.
- **Rust at the native boundary** — Networking, PTYs, transfers, persistence, cancellation, and sensitive operations stay native where possible.
- **Credentials treated as secrets** — Sessions reference vault entries instead of duplicating passwords or private-key material throughout the app.
- **No account required for the local app** — The project is designed around local state, not a mandatory cloud account or subscription.
- **Honest capability labels** — Experimental RDP/VNC work is clearly separated from the mature SSH foundation and release evidence still in progress.

## What is MobaRust?

MobaRust is a Rust/Tauri desktop SSH client and remote-operations workspace with a React and TypeScript interface. It is designed as an open-source alternative to MobaXterm, while also taking inspiration from the best parts of terminal workspaces, SFTP clients, and lightweight administration tools.

MobaXterm made the all-in-one remote toolbox familiar. MobaRust follows that useful idea with a free, open-source implementation that can be inspected, improved, and adapted in public. If you are comparing MobaRust with MobaXterm, PuTTY, Remmina, Tabby, or separate SSH/SFTP tools, these are the workflows MobaRust is designed to bring together:

- cross-platform SSH client foundations for Windows, macOS, and Linux
- explicit local shell targets for PowerShell, cmd, bash, zsh, fish, and WSL where the platform supports them
- SFTP and SCP file transfers
- local terminals and native PTY sessions
- SSH tunnels and bounded SOCKS5 diagnostics
- session profiles, folders, tags, favorites, and fast search
- snippets, visible macros, and explicit multi-terminal broadcast
- Telnet and serial support for legacy equipment
- experimental RDP and VNC integration paths
- remote file editing, network diagnostics, and optional system monitoring

## Capability map

The table below makes the current boundary explicit. “Implemented” means repository code and local validation exist. Platform, real-server, hardware, signing, or interoperability gates are called out instead of being hidden behind a marketing claim.

| Area | Current status |
| --- | --- |
| **SSH terminal** | Native SSH, host-key verification, password/key references, SSH-agent path, keyboard-interactive authentication, PTY resize, keepalives, cancellation, reconnect, and actionable errors |
| **SFTP / SCP** | Remote browser, recursive transfers, bounded concurrency, progress, cancellation, atomic commits, conflict handling, and native SCP compatibility |
| **Session manager** | Saved profiles, folders, tags, favorites, recents, fast search, OpenSSH config import, jump-host chains, typed settings, and secret-free export |
| **SSH tunnels** | Local forwarding, remote forwarding, bounded dynamic SOCKS5, explicit lifecycle state, stop controls, and cancellable clients |
| **Local terminals** | Native PTY, shell lifecycle, resize, output batching, split panes, persistent tabs, child cleanup, explicit PowerShell/cmd/Unix shell targets, and WSL foundations |
| **Operator tools** | Snippets with preview, visible macros, explicit multi-exec targets, network diagnostics, bounded port checks, remote monitoring, and privacy-conscious audit history |
| **Telnet / serial** | Legacy Telnet with clear unencrypted labelling, plus serial configuration, terminal I/O, refresh, reconnect, and device-loss handling |
| **RDP** | Isolated native candidate with framebuffer, protocol-aware keyboard/mouse input, lifecycle work, explicit hostname/IP target metadata, platform certificate validation, a macOS self-signed-certificate rejection fixture, and local process tests; mature-engine integration, real-server interoperability, Windows/Linux evidence, gateway, audio, clipboard, and production packaging remain open |
| **VNC** | Native helper with local RFB fixtures, authentication, framebuffer updates, protocol-aware keyboard/mouse input, clipboard, scaling, quality profiles, reconnect, cancellation, and clean shutdown; broader interoperability remains open |
| **X11** | Explicit SSH forwarding to a configured external display; an integrated cross-platform X server remains a separate research and packaging decision |

This is a serious foundation — not a screenshot, a static placeholder, or a shell command wrapped in a web page.

## The MobaRust difference

### One window for the remote workday

Open an SSH session, browse its files over SFTP, start a tunnel, check a port, run a saved snippet, and keep the connection searchable without rebuilding the same context in several tools.

### Native power with a focused interface

React and TypeScript make the desktop UI fast to iterate and easy to use. Rust and Tauri own the native boundary: sockets, PTYs, helper processes, file operations, vault access, cancellation, and typed IPC. The frontend is an interaction layer — not a credential store and not an unrestricted shell bridge.

### Safety that is visible

MobaRust is designed for operators who cannot afford an accidental production action:

- host-key verification is explicit;
- session configuration is separated from secret material;
- credentials are resolved inside the native boundary when possible;
- helper processes receive secrets through native channels, never command-line arguments;
- passwords, private keys, tokens, and sensitive environment values are redacted from logs;
- broadcast input requires explicit target selection and has an emergency disable path;
- pasted multiline shell commands are not automatically executed;
- network work uses operation-specific timeouts, cancellation, and bounded retries;
- local protocol fixtures use disposable state and loopback networking.

Read the [threat model](docs/security/threat-model.md) and [safe testing policy](docs/security/safe-testing.md) before contributing protocol or credential code.

## Honest project status

The current engineering checklist is **60/67 items evidenced — approximately 89.6%**. This is a measure of verified repository work, not a claim of complete MobaXterm parity or production readiness on every operating system.

The local implementation layer is ahead of the release matrix. SSH, SFTP/SCP, PTY, explicit local shell targets, sessions, tunnels, diagnostics, and the security boundaries have a substantial local test foundation. RDP/VNC helpers, macOS packaging, and cross-platform contracts are being developed incrementally; a macOS-only TLS fixture now proves that an untrusted self-signed certificate is rejected, while real Windows/Linux shell interoperability, cross-platform certificate-store evidence, serial hardware, signed distribution, and broader desktop evidence still require their target environments.

The next gates are visible in the [roadmap](ROADMAP.md):

- mature-engine RDP integration with real Windows interoperability;
- broader VNC interoperability beyond local fixtures;
- Windows, Linux, macOS, WSL, clipboard, serial-hardware, DPI, and multi-monitor evidence;
- a practical integrated or external X-server strategy;
- signed, notarized, portable packages and clean-install verification.

RDP and VNC are labelled experiments until those gates are proven. That honesty is part of the product quality bar.

## Quick start

MobaRust is currently source-first. The commands below build and validate the local desktop application on macOS; target-specific runtime and signed-release checks belong in their respective Windows/Linux environments.

### Requirements

- Rust stable (the workspace currently requires Rust 1.88 or newer)
- Node.js and pnpm
- Tauri desktop prerequisites for your operating system

### Build and run

```bash
git clone https://github.com/OthmaneBlial/MobaRust.git
cd MobaRust
pnpm install --dir apps/desktop
cargo xtask check
cargo tauri dev --manifest-path apps/desktop/src-tauri/Cargo.toml
```

### Validate the safe local path

```bash
cargo xtask check
cargo xtask package-check
cargo xtask portable-check
cargo xtask package-layout-check
cargo xtask verify-platform-layout macos target/debug/bundle/macos/MobaRust.app
cargo xtask pre-push-check
```

The validation path uses isolated home/XDG directories, repository-owned or disposable fixture paths, and loopback-only protocol servers. It does **not** need your personal `~/.ssh`, GitHub keys, Keychain, SSH agent, real hosts, or attached hardware.

`package-check` and `portable-check` currently produce unsigned macOS smoke artifacts. They do not claim notarization, cross-platform release support, or RDP/VNC interoperability.

On macOS, the isolated RDP trust fixture can be run explicitly:

```bash
cargo test --locked --manifest-path tools/rdp-helper/Cargo.toml \
  platform_tls_rejects_a_self_signed_loopback_certificate -- --nocapture
```

It creates only a short-lived synthetic certificate and key in a disposable
temporary directory, connects only to `127.0.0.1`, and verifies that the
platform trust verifier rejects the certificate. Windows/Linux certificate
store fixtures and real RDP-server interoperability remain future gates.

The isolated RDP candidate is not staged by the normal build path. For an explicit repository-local development run, use `cargo xtask stage-rdp-helper` first. This does not make RDP production-ready, bypass its dependency audit, or provide Windows/Linux interoperability evidence.

## Architecture at a glance

```text
React + TypeScript + xterm.js
                │ typed Tauri commands and events
Rust desktop boundary
  ├─ SSH / SFTP / SCP / jump hosts / tunnels
  ├─ PTY, local shells, terminal tabs and splits
  ├─ session store, settings, imports, exports and vault references
  ├─ transfers, editor, diagnostics, monitoring and audit events
  └─ isolated RDP / VNC helper experiments
```

Session configuration and secret material are separate by design. A session stores an opaque credential reference; the secret is resolved only inside the native boundary when a protocol needs it. Helpers receive credentials through a dedicated native pipe, never through process arguments.

## Documentation

- [Roadmap](ROADMAP.md)
- [Architecture decisions](docs/adr/)
- [Security threat model](docs/security/threat-model.md)
- [Safe local testing policy](docs/security/safe-testing.md)
- [Dependency audit](docs/security/dependency-audit.md)
- [RDP integration research](docs/research/rdp.md)
- [VNC integration research](docs/research/vnc.md)
- [X11 forwarding strategy](docs/research/x11.md)
- [Remote editor decision](docs/adr/0022-bounded-remote-text-editor.md)
- [Keyboard shortcuts](docs/keyboard-shortcuts.md)
- [Hardware and interoperability matrix](docs/testing/hardware-interoperability.md)
- [Release and packaging notes](docs/release/packaging.md)
- [Reddit launch draft](docs/marketing/reddit-launch.md)

## Contributing

The most useful contributions are concrete and reproducible:

1. Try the SSH, SFTP/SCP, terminal, tunnel, and session workflows.
2. Report the exact operating system, environment, and steps to reproduce.
3. Tell us which MobaXterm, PuTTY, Remmina, Tabby, or terminal workflow you would need before switching.
4. Contribute tests, protocol fixtures, platform evidence, documentation, or focused code.

Please do not include passwords, private keys, host inventories, real connection logs, or personal configuration exports in issues or pull requests.

If you are building an open-source MobaXterm alternative, SSH client, SFTP tool, terminal workspace, or remote administration tool, MobaRust is open to focused collaboration.

MobaRust is released under the [Apache License 2.0](LICENSE).
