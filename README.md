# MobaRust — the free, open-source MobaXterm alternative

<p align="center">
  <img src="apps/desktop/src-tauri/icons/icon.svg" alt="MobaRust logo" width="120" />
</p>

<p align="center">
  <strong>Your terminal, file transfer, and remote operations in one calm desktop workspace.</strong><br />
  A free SSH client and SFTP workspace for developers, DevOps, sysadmins, and homelabs.
</p>

<p align="center">
  <a href="https://othmaneblial.github.io/MobaRust/">Website</a> ·
  <a href="https://othmaneblial.github.io/MobaRust/docs.html">Documentation</a> ·
  <a href="ROADMAP.md">Roadmap</a> ·
  <a href="https://github.com/OthmaneBlial/MobaRust/issues">Feedback</a>
</p>

<p align="center">
  <a href="https://github.com/OthmaneBlial/MobaRust/blob/main/LICENSE"><img src="https://img.shields.io/badge/License-Apache%202.0-2ea44f?style=flat-square" alt="Apache 2.0 license" /></a>
  <a href="https://github.com/OthmaneBlial/MobaRust/stargazers"><img src="https://img.shields.io/github/stars/OthmaneBlial/MobaRust?style=flat-square&label=stars" alt="GitHub stars" /></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/built%20with-Rust-orange?style=flat-square&logo=rust" alt="Built with Rust" /></a>
  <a href="https://tauri.app/"><img src="https://img.shields.io/badge/desktop-Tauri-24c8db?style=flat-square" alt="Tauri desktop application" /></a>
</p>

> Looking for a free and open-source MobaXterm alternative? MobaRust is a local-first desktop SSH client, SFTP client, terminal emulator, and remote-operations workspace for developers, DevOps engineers, system administrators, infrastructure teams, and homelab users.

MobaRust is independent and is not affiliated with Mobatek or MobaXterm. MobaXterm is a trademark of its respective owner.

MobaRust is a real desktop application, not a terminal-shaped website. React and TypeScript render the interface inside Tauri; Rust owns the native networking, PTY, filesystem, process, persistence, cancellation, and credential boundaries.

## Stop juggling five tools for one server

Remote work should not mean switching between a terminal, an SFTP client, a tunnel tool, a notes app, and a session list every few minutes. MobaRust puts the operator loop in one focused window:

1. **Connect** to a saved host or use Quick Connect in seconds.
2. **Inspect** files, system information, connection state, and diagnostics.
3. **Transfer** remote files with progress, cancellation, and conflict awareness.
4. **Operate** through a visible terminal, tunnel, snippet, or carefully selected broadcast action.
5. **Disconnect** cleanly, with bounded retries and no hidden background work.

The result is a practical MobaXterm alternative that stays keyboard-friendly, local-first, and honest about what is ready.

## Why operators choose MobaRust

- **Free and open source** — Apache-2.0 licensed, inspectable, forkable, and developed in public.
- **A serious MobaXterm alternative** — SSH, SFTP/SCP, local terminals, tunnels, session profiles, and diagnostics in one workspace.
- **Local-first by design** — no mandatory cloud account or subscription for the local application.
- **Native where it matters** — Rust handles sockets, PTYs, transfers, helpers, and sensitive operations behind typed Tauri commands.
- **Safer defaults** — sessions reference credentials; passwords and private keys are not duplicated through ordinary profile data, logs, or process arguments.
- **No vaporware marketing** — mature SSH foundations are separated from RDP/VNC candidates and platform work that still needs evidence.

## Built for the remote workday

Save a host once, organize it into folders, add tags and notes, mark favorites, and find it instantly. Browse remote files over SFTP, transfer recursively with progress and cancellation, start a tunnel, run a reviewed snippet, and keep the connection searchable without rebuilding the same context in several tools.

Snippets support descriptions, tags, and variables such as `${host}`, `${port}`, and `${username}`. Preview and edit before sending. Macros remain visible and cancellable. Multi-exec requires explicit target selection, shows a strong broadcast indicator, and provides an emergency disable path.

The best first use case is everyday SSH and SFTP work. RDP and VNC are real isolated candidates, while integrated X11, signed distribution, and complete cross-platform evidence remain deliberately on the roadmap.

## An open alternative to the commercial all-in-one toolbox

MobaXterm made the all-in-one remote toolbox familiar. MobaRust keeps that useful idea while giving operators a free, open-source implementation that can be inspected, improved, and adapted in public. There is no claim of feature-for-feature parity today; the roadmap makes the remaining work visible.

| If you value… | MobaRust gives you… |
| --- | --- |
| A free alternative | Apache-2.0 source with no mandatory account for local use |
| One place for remote work | Terminals, files, tunnels, diagnostics, and session context together |
| Control over sensitive data | Separated session configuration and secret material, with native credential resolution |
| A maintainable desktop stack | Rust/Tauri native boundaries and a React/TypeScript interface |
| Trustworthy progress | Reproducible fixtures, visible limitations, and a public roadmap |

## What can you use today?

The table below makes the current boundary explicit. “Verified locally” means repository code and deterministic local validation exist. Platform, real-server, hardware, signing, or interoperability gates are called out instead of being hidden behind a marketing claim.

| Area | Current status |
| --- | --- |
| **SSH terminal** | Native SSH, host-key verification, password/key references, SSH-agent path, keyboard-interactive authentication, PTY resize, keepalives, cancellation, reconnect, and actionable errors |
| **SFTP / SCP** | Remote browser, recursive transfers, bounded concurrency, progress, cancellation, atomic commits, conflict handling, permission preservation, and native SCP compatibility |
| **Session manager** | Saved profiles, folders, tags, favorites, recents, fast search, OpenSSH config import, jump-host chains, typed settings, and secret-free export |
| **SSH tunnels** | Local forwarding, remote forwarding, bounded dynamic SOCKS5, explicit lifecycle state, stop controls, and cancellable clients |
| **Local terminals** | Native PTY, shell lifecycle, resize, output batching, split panes, persistent tabs, child cleanup, explicit PowerShell/cmd/Unix shell targets, and WSL foundations |
| **Operator tools** | Snippets with preview, visible macros, explicit multi-exec targets, network diagnostics, bounded port checks, one-shot or opt-in low-frequency remote monitoring, privacy-conscious audit history, and sanitized diagnostic export |
| **Telnet / serial** | Legacy Telnet with clear unencrypted labelling, plus serial configuration, terminal I/O, refresh, reconnect, and device-loss handling |
| **RDP** | Isolated native candidate with framebuffer, protocol-aware bounded keyboard/mouse input, coalesced dynamic resize requests, lifecycle work, explicit hostname/IP and Gateway metadata, separate role-tagged native credential handoff, platform certificate validation, bounded configurable reconnect, an opt-in Windows-native clipboard path, a typed runtime capability report, a macOS self-signed-certificate rejection fixture, and local process tests; mature-engine integration, real-server interoperability, Windows/Linux evidence, Gateway trust/interoperability, macOS/Linux clipboard backends, audio, and production packaging remain open |
| **VNC** | Native helper with local RFB fixtures, authentication, raw/copy/Tight-JPEG framebuffer updates, bounded keyboard/mouse input with Unicode-to-X11 keysym mapping, explicit clipboard opt-in enforced at both native boundaries, scaling, quality profiles, bounded configurable reconnect, cancellation, and clean shutdown; broader interoperability remains open |
| **X11** | Explicit SSH forwarding to a configured external display; an integrated cross-platform X server remains a separate research and packaging decision |

This is a serious foundation — not a screenshot, a static placeholder, or a shell command wrapped in a web page.

## Native power with a focused interface

React and TypeScript make the desktop UI fast to iterate and easy to use. Rust and Tauri own the native boundary: sockets, PTYs, helper processes, file operations, vault access, cancellation, and typed IPC. The frontend is an interaction layer — not a credential store and not an unrestricted shell bridge.

### Safety that is visible

MobaRust is designed for operators who cannot afford an accidental production action:

- host-key verification is explicit;
- session configuration is separated from secret material;
- credentials are resolved inside the native boundary when possible;
- helper processes receive secrets through native channels, never command-line arguments;
- passwords, private keys, tokens, and sensitive environment values are redacted from logs;
- native terminal, persistence, and vault errors do not echo local paths or raw OS/backend details;
- broadcast input requires explicit target selection and has an emergency disable path;
- pasted multiline shell commands are not automatically executed;
- network work uses operation-specific timeouts, cancellation, and bounded retries;
- connection metadata is bounded and rejects control characters and accidental
  leading/trailing whitespace before native channels or helpers are opened;
- local protocol fixtures use disposable state and loopback networking.

Read the [threat model](docs/security/threat-model.md) and [safe testing policy](docs/security/safe-testing.md) before contributing protocol or credential code.

## Honest project status

The current engineering checklist is **61/68 items evidenced — approximately 89.7%**. This is a measure of verified repository work, not a claim of complete MobaXterm parity or production readiness on every operating system.

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

For a repository-local process-start probe after building the desktop binary:

```text
cargo xtask benchmark-app target/debug/mobarust
```

This runs only the app's `--version` path before Tauri initialization with a
sanitized environment. It reports process-launch timing and binary size; it
does not claim full cold-start, warm-start, memory, idle-CPU, renderer, or
cross-platform performance.

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
