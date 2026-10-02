<div align="center">

<img src="apps/desktop/src-tauri/icons/icon.png" alt="MobaRust" width="80" />

# MobaRust

### Your terminals. Your servers. One workspace.

A free, open-source remote workstation built with **Rust + Tauri**.
SSH, files, local shells, and tunnels — with no cloud account required.

[![Preview](https://img.shields.io/github/v/release/OthmaneBlial/MobaRust?include_prereleases&label=preview&style=flat-square&color=7d9967)](https://github.com/OthmaneBlial/MobaRust/releases/tag/v0.1.16) [![Quality](https://github.com/OthmaneBlial/MobaRust/actions/workflows/quality.yml/badge.svg)](https://github.com/OthmaneBlial/MobaRust/actions/workflows/quality.yml) [![License](https://img.shields.io/badge/license-Apache--2.0-4b6653?style=flat-square)](LICENSE)

**[Download](#download)** · **[Website](https://othmaneblial.github.io/MobaRust/)** · **[Docs](https://othmaneblial.github.io/MobaRust/docs.html)** · **[Roadmap](ROADMAP.md)** · **[Contribute](CONTRIBUTING.md)**

</div>

[![MobaRust native desktop walkthrough](site/media/mobarust-demo-preview.gif)](https://othmaneblial.github.io/MobaRust/#demo)

*Native macOS app (v0.1.11): real local terminals and split panes; SSH is shown as URI entry. [Demo provenance](docs/release/desktop-demo.md).*

MobaRust is an independent MobaXterm alternative for developers, operators, and homelab users. It is not affiliated with Mobatek or MobaXterm.

## Download

| Platform | Latest available installer | Install |
| --- | --- | --- |
| **macOS · Apple Silicon** | [v0.1.16 · DMG](https://github.com/OthmaneBlial/MobaRust/releases/download/v0.1.16/MobaRust-0.1.16-macos-arm64.dmg) | Move the app to Applications. |
| **macOS · Intel** | [v0.1.16 · DMG](https://github.com/OthmaneBlial/MobaRust/releases/download/v0.1.16/MobaRust-0.1.16-macos-x64.dmg) | Move the app to Applications. |
| **Windows · x64** | [v0.1.12 · Installer](https://github.com/OthmaneBlial/MobaRust/releases/download/v0.1.12/MobaRust-0.1.12-windows-x64.exe) | Includes WebView2 setup if needed. |
| **Ubuntu / Debian · x64** | [v0.1.12 · DEB](https://github.com/OthmaneBlial/MobaRust/releases/download/v0.1.12/MobaRust-0.1.12-linux-x64.deb) | `sudo apt install ./MobaRust-0.1.12-linux-x64.deb` |
| **Other Linux · x64** | [v0.1.12 · AppImage](https://github.com/OthmaneBlial/MobaRust/releases/download/v0.1.12/MobaRust-0.1.12-linux-x64.AppImage) | WebKitGTK 4.1 and FUSE may be required. |

**Preview builds:** installers are unsigned; macOS is not Developer ID signed or notarized. Windows/Linux downloads are older than the Mac preview, and improvements on `main` are not yet new release assets.

[Mac release notes and checksums](https://github.com/OthmaneBlial/MobaRust/releases/tag/v0.1.16) · [Windows/Linux notes and checksums](https://github.com/OthmaneBlial/MobaRust/releases/tag/v0.1.12) · [Installation guide](docs/release/preview-notes.md)

Open a local terminal or create an SSH session. Verify the server fingerprint before trusting its first connection. SHA-256 checksums verify downloaded bytes; they do not establish publisher identity.

## Built for everyday remote work

| Workflow | What you can do |
| --- | --- |
| **Connect** | SSH with explicit host-key verification, password/key/agent authentication, encrypted keys, saved sessions, jump hosts, and bounded reconnect. |
| **Move files** | Browse and manage SFTP files; run streaming SCP/SFTP transfers with progress, cancellation, recursive transfers, and explicit overwrite handling. |
| **Keep context** | Work across terminal tabs and split panes; organize sessions with folders, tags, favorites, recents, search, and OpenSSH config import. |
| **Reach services** | Manage local and remote forwards, SOCKS5 tunnels, bounded diagnostics, and opt-in SSH monitoring. |
| **Edit and automate** | Edit remote text with conflict detection; preview snippets, confirm macros, and select broadcast targets with an emergency stop. |
| **Use legacy links** | Open Telnet and serial sessions. Telnet is unencrypted; real serial-adapter coverage remains a separate gate. |

## Progress you can verify

**Updated 2026-10-02.** Recent work on `main`:

- Fixed quiet SSH sessions closing at the setup timeout, exit status arriving after EOF, and misleading host-trust errors.
- Expanded the disposable OpenSSH lab: encrypted keys, two distinct jump hosts, trust failures, cancellation, a dedicated Unix agent, and IPv6 loopback.
- Added Ubuntu/macOS/Windows quality CI. Ubuntu x64 and macOS ARM64 pass; the Windows ConPTY fixture currently fails and remains an open priority.
- Added contributor and architecture guides, private security reporting, release-asset regressions, and a reproducible benchmark receipt.

See the **[roadmap and next acceptance gates](ROADMAP.md)**, [native test evidence](docs/testing/hardware-interoperability.md), and [benchmark record](benchmarks/2026-10-02-local.md). There is no overall completion percentage: implementation, CI, packaged releases, and real-device evidence are tracked separately.

| Area | Current boundary |
| --- | --- |
| **Core workstation** | Implemented and available in preview builds; broader interoperability validation continues. |
| **RDP** | Experimental. The isolated helper remains excluded from normal installers while dependency and certificate gates are open. |
| **VNC** | Experimental. Local RFB fixtures exist; remote TCP is unencrypted and requires explicit opt-in. |
| **X11 and serial hardware** | Native paths exist; external X-server and physical-adapter matrices remain incomplete. |
| **Distribution** | Unsigned previews. Signing, notarization, clean installation, and aligned platform releases are still open. |

## Security by design

Rust owns protocols, processes, persistence, and credential access. Profiles keep credential references rather than plaintext secrets. Unknown SSH host keys are never silently accepted; multiline paste and remote execution have explicit confirmation boundaries.

[Threat model](docs/security/threat-model.md) · [Dependency audit](docs/security/dependency-audit.md) · [Report a vulnerability privately](SECURITY.md)

## Build locally

Use Rust stable **1.88+**, Node.js **22**, pnpm **10**, and your OS's Tauri prerequisites. Follow [CONTRIBUTING.md](CONTRIBUTING.md) for native dependencies and fixture setup.

```bash
git clone https://github.com/OthmaneBlial/MobaRust.git
cd MobaRust
pnpm install --dir apps/desktop --frozen-lockfile
cargo xtask check
pnpm --dir apps/desktop tauri dev
```

The full check covers Rust, frontend, isolated protocol helpers, synthetic package layouts, and fuzz-target compilation. It uses disposable state and fixtures; personal SSH files, agent identities, Keychain entries, remote hosts, and attached hardware are unnecessary.

```bash
cargo xtask test-ssh     # Disposable OpenSSH interoperability lab on Unix
cargo xtask check-rust   # Rust workspace, native fixtures, and Clippy
```

[SSH lab](docs/testing/ssh-lab.md) · [Safe testing](docs/security/safe-testing.md) · [Architecture](docs/architecture.md) · [Benchmarks](benchmarks/README.md)

## Contribute

Reproduce a bug, test a dedicated platform fixture, or finish an existing workflow. The [roadmap](ROADMAP.md) names concrete next steps and completion criteria; the [contributor guide](CONTRIBUTING.md) explains where to start.

[Report a bug](https://github.com/OthmaneBlial/MobaRust/issues/new/choose) · [Discuss a feature](https://github.com/OthmaneBlial/MobaRust/issues/new/choose) · [Star the project](https://github.com/OthmaneBlial/MobaRust/stargazers)

Please keep credentials, private keys, server inventories, and unredacted logs out of public reports.

Licensed under [Apache-2.0](LICENSE).
