# MobaRust

<p align="center">
  <img src="apps/desktop/src-tauri/icons/icon.svg" alt="MobaRust logo" width="104" />
</p>

<h3 align="center">The free, open-source MobaXterm alternative for people who operate machines.</h3>

<p align="center">
  One focused desktop workspace for SSH, SFTP, SCP, terminals, tunnels, diagnostics, and remote operations — built with Rust.
</p>

<p align="center">
  <a href="https://othmaneblial.github.io/MobaRust/">Project website</a> ·
  <a href="https://othmaneblial.github.io/MobaRust/docs.html">Documentation</a> ·
  <a href="ROADMAP.md">Roadmap</a> ·
  <a href="https://github.com/OthmaneBlial/MobaRust/issues">Issues and feedback</a>
</p>

<p align="center">
  <a href="https://github.com/OthmaneBlial/MobaRust/blob/main/LICENSE"><img src="https://img.shields.io/github/license/OthmaneBlial/MobaRust?style=flat-square&label=license" alt="Apache 2.0 license" /></a>
  <a href="https://github.com/OthmaneBlial/MobaRust"><img src="https://img.shields.io/github/repo-size/OthmaneBlial/MobaRust?style=flat-square&label=source" alt="Repository size" /></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/built%20with-Rust-orange?style=flat-square&logo=rust" alt="Built with Rust" /></a>
</p>

> **One window for the machines you operate.** No subscription required. No opaque session database required. No “send everything to the cloud” workflow required.

MobaRust is a free and open-source **MobaXterm alternative**, SSH client, SFTP/SCP file-transfer tool, terminal workspace, and remote-operations console for developers, DevOps engineers, system administrators, infrastructure teams, and homelab users.

It is an independent project and is not affiliated with Mobatek or MobaXterm.

## Why MobaRust?

Remote work gets messy when the terminal lives in one app, file transfer in another, tunnels in a third, and connection profiles in a database you cannot inspect. MobaRust brings those daily workflows together in one keyboard-friendly desktop application while keeping the important boundaries visible.

- **Free and open source** — Apache-2.0 licensed, inspectable, forkable, and built for contribution.
- **A real operator workspace** — terminals, remote files, tunnels, diagnostics, snippets, monitoring, and session context in one place.
- **Rust at the safety boundary** — networking, PTYs, transfer pipelines, persistence, cancellation, and sensitive operations stay native where possible.
- **Credentials treated as credentials** — saved sessions reference vault entries; passwords and private-key material are not copied into React state, logs, exports, or process arguments.
- **Built for practical platforms** — Windows-first attention, with macOS and Linux architecture kept in the design from the beginning.
- **Honest by default** — experimental protocol work is labelled as experimental until real interoperability and platform evidence exist.

## What you can use today

The current baseline is implemented in the repository and covered by local unit, integration, property, fixture, and packaging checks. The table below separates working foundations from features that still need target-platform or real-server validation.

| Workflow | Current state |
| --- | --- |
| **SSH terminal** | Native SSH, host-key verification, password/key references, SSH agent path, keyboard-interactive authentication, PTY resize, keepalives, cancellation, reconnect, and actionable errors |
| **SFTP and SCP** | Remote browser, recursive transfers, bounded concurrency, progress, cancellation, atomic commits, conflict handling, and native SCP compatibility |
| **Session manager** | Saved profiles, folders, tags, favorites, recents, fast search, OpenSSH config import, jump-host chains, typed settings, and secret-free export |
| **Tunnels** | Local forwarding, remote forwarding, bounded dynamic SOCKS5, explicit lifecycle state, stop controls, and cancellable clients |
| **Local terminals** | Native PTY, shell lifecycle, resize, output batching, split panes, persistent tabs, child cleanup, and WSL foundations |
| **Operator tools** | Snippets with preview, visible macros, explicit multi-exec/broadcast targets, network diagnostics, bounded port checks, remote monitoring, and privacy-conscious audit history |
| **Legacy equipment** | Telnet with clear unencrypted labelling, plus serial configuration, terminal I/O, refresh, reconnect, and device-loss handling |
| **RDP** | Isolated native candidate with framebuffer/input/lifecycle code and loopback-safe local experiments; mature-engine selection, real-server interoperability, Windows evidence, gateway, audio, clipboard, and production packaging remain open |
| **VNC** | Real native helper with local RFB fixtures, authentication, framebuffer updates, keyboard, mouse, clipboard, scaling, quality profiles, reconnect, cancellation, and pipe-independent process shutdown; cross-platform interoperability remains open |
| **X11** | Explicit SSH forwarding to a configured external display; an integrated cross-platform X server is a separate research and packaging decision |

This is a serious foundation for a MobaXterm replacement — not a screenshot, a mock terminal, or a shell command wrapped in a web page.

## The MobaRust difference

### One workspace, less context switching

Open an SSH session, browse its files over SFTP, start a tunnel, inspect a port, run a saved snippet, and keep the connection searchable without rebuilding the workflow in five different tools.

### Native operations, focused UI

React and TypeScript provide the fast, approachable desktop interface. Rust and Tauri own the native boundary: sockets, PTYs, helper processes, file operations, vault access, cancellation, and typed IPC. The frontend is an interaction layer, not a credential store or an unrestricted shell bridge.

### Safety that is visible in the product

MobaRust is designed for administrators who cannot afford a “nearly sent that to production” moment:

- host-key verification is explicit;
- credentials are separated from session configuration;
- helper processes are isolated and receive secrets through a native channel;
- logs and debug formatting redact secrets and sensitive payloads;
- broadcast input requires explicit target selection and has an emergency disable path;
- pasted multiline shell commands are not automatically executed;
- network operations have operation-specific timeout and cancellation paths;
- local protocol fixtures run on loopback and do not need personal SSH files.

Read the [threat model](docs/security/threat-model.md) and [safe testing policy](docs/security/safe-testing.md) for the boundaries in detail.

## MobaRust vs. a closed-source remote toolbox

MobaRust is for users who want the convenience of an all-in-one remote-work application with the transparency of an open-source project. It does not ask you to trust a hidden session database, and it does not require an account or subscription for the local application.

| | MobaRust | A closed-source remote toolbox |
| --- | --- | --- |
| License | Apache-2.0 open source | Depends on the product and edition |
| Session model | Inspectable profiles that reference secrets | Product-specific storage and export rules |
| Native boundary | Rust/Tauri with typed commands | Product-specific |
| Automation safety | Visible, bounded, approval-aware actions | Product-specific |
| Development model | Public roadmap, issues, source, and fixtures | Vendor-controlled roadmap |

The goal is not to imitate every screen. The goal is to give operators a trustworthy, extensible tool they can understand and improve.

## Honest project status

The current engineering checklist is **60/67 items evidenced — approximately 89.6%**. This is a progress measure for verified repository work, not a claim of complete MobaXterm parity.

The local implementation layer is further along than the release matrix. The macOS PTY path, isolated RDP candidate, real VNC helper with loopback fixtures, target-aware unsigned package layout, and explicit PTY child reaping are implemented and checked locally.

The next validation gates are deliberately visible:

- mature-engine RDP integration with real Windows interoperability;
- broader VNC interoperability beyond local fixtures;
- Windows, Linux, macOS, WSL, clipboard, serial-hardware, DPI, and multi-monitor evidence;
- an integrated/external X-server strategy across supported desktops;
- signed, notarized, portable packages and clean-install verification.

RDP and VNC are clearly marked experiments until those gates are proven. That honesty is part of the product quality bar.

See the complete [roadmap](ROADMAP.md), [RDP research](docs/research/rdp.md), [VNC research](docs/research/vnc.md), [X11 strategy](docs/research/x11.md), and [release matrix](docs/release/packaging.md).

## Quick start for contributors

MobaRust is currently source-first. The commands below build and validate the local desktop application on macOS; target-specific runtime and signed-release checks belong in their respective Windows/Linux environments later.

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

The isolated RDP candidate is not staged by the normal build path. For an explicit repository-local development run, use `cargo xtask stage-rdp-helper` first; this does not make RDP production-ready, bypass its separate dependency audit, or provide Windows/Linux interoperability evidence.

The helper also has a local process smoke test: it starts the real compiled binary, sends `Start` and the credential through native pipes, checks a bounded terminal outcome against a disposable loopback socket that closes immediately, and verifies process exit without exposing the fixture secret. This validates helper integration, not a real RDP server or cross-platform interoperability.

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

Session configuration and secret material are separate by design. A session stores an opaque credential reference; the secret is resolved only inside the native boundary when a protocol needs it. Helper processes receive credentials through a dedicated native pipe, never through command-line arguments.

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

## Help shape the open-source alternative

The most useful contributions are concrete:

1. Try the SSH, SFTP/SCP, terminal, tunnel, and session workflows.
2. Report the exact platform, environment, and reproducible steps.
3. Tell us which MobaXterm, PuTTY, Remmina, Tabby, or terminal workflow you would need before switching.
4. Contribute tests, protocol fixtures, platform evidence, documentation, or focused code.

Please do not include passwords, private keys, host inventories, real connection logs, or personal configuration exports in issues or pull requests.

MobaRust is released under the [Apache License 2.0](LICENSE).
