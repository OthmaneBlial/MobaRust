# MobaRust

### SSH, files, terminals, and tunnels in one native workspace.

MobaRust is a free, open-source MobaXterm alternative for people who work across remote machines. Connect over SSH, move files with SFTP/SCP, and keep local shells, port forwards, and diagnostics beside your sessions. Built with Rust and Tauri; no cloud account required.

[![GitHub stars](https://img.shields.io/github/stars/OthmaneBlial/MobaRust?style=flat-square&logo=github)](https://github.com/OthmaneBlial/MobaRust/stargazers) [![Preview release](https://img.shields.io/github/v/release/OthmaneBlial/MobaRust?include_prereleases&label=preview&style=flat-square)](https://github.com/OthmaneBlial/MobaRust/releases/tag/v0.1.15) [![Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-4b6653?style=flat-square)](LICENSE)

[Download v0.1.15 for Mac](https://github.com/OthmaneBlial/MobaRust/releases/tag/v0.1.15) · [Project site](https://othmaneblial.github.io/MobaRust/) · [Documentation](https://othmaneblial.github.io/MobaRust/docs.html) · [Roadmap](ROADMAP.md)

Independent project. Not affiliated with Mobatek or MobaXterm.

[![MobaRust desktop workspace preview](site/media/mobarust-demo-preview.gif)](https://github.com/OthmaneBlial/MobaRust/releases/tag/v0.1.15)

## Get started

Download an installer, open MobaRust, and start with a local terminal or an SSH session. On the first SSH connection, inspect and verify the host-key fingerprint before trusting the server.

| Platform | Download | Notes |
| --- | --- | --- |
| Windows x64 | [Installer v0.1.12](https://github.com/OthmaneBlial/MobaRust/releases/download/v0.1.12/MobaRust-0.1.12-windows-x64.exe) | Previous preview; WebView2 is installed if needed. |
| macOS Apple Silicon | [DMG v0.1.15](https://github.com/OthmaneBlial/MobaRust/releases/download/v0.1.15/MobaRust-0.1.15-macos-arm64.dmg) | Move MobaRust to Applications. |
| macOS Intel | [DMG v0.1.15](https://github.com/OthmaneBlial/MobaRust/releases/download/v0.1.15/MobaRust-0.1.15-macos-x64.dmg) | Move MobaRust to Applications. |
| Ubuntu / Debian x64 | [DEB v0.1.12](https://github.com/OthmaneBlial/MobaRust/releases/download/v0.1.12/MobaRust-0.1.12-linux-x64.deb) | Previous preview; `sudo apt install ./MobaRust-0.1.12-linux-x64.deb` |
| Other Linux x64 | [AppImage v0.1.12](https://github.com/OthmaneBlial/MobaRust/releases/download/v0.1.12/MobaRust-0.1.12-linux-x64.AppImage) | Previous preview; WebKitGTK 4.1 and FUSE may be required. |

[Mac release files and SHA-256 checksums](https://github.com/OthmaneBlial/MobaRust/releases/tag/v0.1.15) · [v0.1.15 release notes](docs/release/v0.1.15.md) · [Windows/Linux v0.1.12](https://github.com/OthmaneBlial/MobaRust/releases/tag/v0.1.12)

These are preview builds without publisher signing. Windows may show SmartScreen; macOS is not Developer ID signed or notarized. Checksums detect download corruption, not publisher identity. See the [Mac release notes](docs/release/v0.1.15.md) or [v0.1.12 installation notes](docs/release/preview-notes.md).

## One workspace for the whole session

| Workflow | What MobaRust includes |
| --- | --- |
| **Connect** | Interactive SSH, explicit host-key verification, vault-backed password/key and SSH-agent authentication, saved sessions, jump hosts, reconnect, and terminal resize. |
| **Move files** | SFTP file browsing and management; SCP and SFTP transfers with progress, cancellation, recursive operations, and safe commits. |
| **Keep context** | Local and remote terminals, tabs, split panes, folders, tags, favorites, recents, search, OpenSSH config import, and secret-free profile export. |
| **Reach services** | Local and remote port forwarding, SOCKS5 tunnels, bounded network diagnostics, and optional SSH monitoring. |
| **Work deliberately** | Reviewed snippets, cancellable macros, explicit broadcast targets, remote text editing, and an optional local audit history. |
| **Use legacy links** | Telnet and serial connection workflows are available; Telnet is unencrypted and serial hardware coverage is still being built. |

## Security is part of the workflow

- Rust owns network sessions, local processes, filesystem work, persistence, and credential access.
- Session profiles store credential references, not passwords or private-key contents.
- SSH host keys must be explicitly verified; an unknown server is never silently trusted.
- Multiline terminal paste asks before sending text to a shell.
- Imports, exports, snippets, macros, and diagnostics have explicit boundaries.

Read the [threat model](docs/security/threat-model.md) and [safe local testing policy](docs/security/safe-testing.md).

## Preview status

MobaRust focuses on SSH, SFTP/SCP, terminals, and tunnels. Some protocol and platform work is still experimental:

- **RDP:** experimental; the RDP helper is not included in these installers.
- **VNC:** experimental; remote TCP is unencrypted and requires explicit opt-in.
- **Platform evidence:** real-world Windows/Linux hardware coverage, signing, and notarized installation are not complete.

See the [roadmap](ROADMAP.md) for shipped work and open evidence gaps. If you find a bug, [open an issue](https://github.com/OthmaneBlial/MobaRust/issues) with your OS, architecture, and reproduction steps. Never attach credentials, private keys, host inventories, or unredacted logs.

## Build and check locally

### Requirements

- Rust stable 1.88 or newer
- Node.js 22 and pnpm
- Tauri prerequisites for your operating system

```bash
git clone https://github.com/OthmaneBlial/MobaRust.git
cd MobaRust
pnpm install --dir apps/desktop
cargo xtask check
pnpm --dir apps/desktop tauri dev
```

`cargo xtask check` runs the local validation suite. Tests use isolated state and disposable fixtures; they do not need your personal SSH files, Keychain, GitHub keys, SSH agent, real hosts, or attached hardware.

Focused checks:

```bash
cargo xtask package-check
cargo xtask portable-check
cargo xtask package-layout-check
cargo xtask license-check
cargo xtask pre-push-check
```

## Help MobaRust reach more operators

If MobaRust makes remote work easier, [give the repository a star](https://github.com/OthmaneBlial/MobaRust/stargazers). It helps other developers and operators discover the project. Bug reports, careful testing, and contributions are welcome.

Licensed under the [Apache License 2.0](LICENSE).
