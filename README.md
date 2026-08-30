# MobaRust

## The free, open-source MobaXterm alternative built with Rust

MobaRust is a serious remote workstation for developers, DevOps engineers, system administrators, infrastructure teams, cloud operators, and homelab users who want one focused application for remote work.

It brings the daily MobaXterm workflow into an independent, Rust-first desktop app: SSH terminals, SFTP/SCP file operations, tunnels, saved sessions, local shells, diagnostics, serial consoles, Telnet, and remote-operations tooling.

[Open the project website](https://othmaneblial.github.io/MobaRust/) · [View the roadmap](ROADMAP.md) · [Try the source](https://github.com/OthmaneBlial/MobaRust)

![MobaRust logo](apps/desktop/src-tauri/icons/icon.svg)

> One window for the machines you operate. No subscription required. No closed-source session database required.

MobaRust is not affiliated with Mobatek or MobaXterm. It is an independent open-source alternative, built for people who want a transparent and extensible toolchain.

## Why use MobaRust?

MobaXterm is popular because it puts remote terminals, file transfer, sessions, and administration utilities in one place. MobaRust follows that same practical idea while keeping the core open, inspectable, and powered by Rust.

- **Free and open source** — inspect the code, run it locally, and contribute improvements.
- **One remote workspace** — keep terminal, files, tunnels, diagnostics, and session context together.
- **Rust-first core** — SSH, PTY, transfer, storage, cancellation, and sensitive operations stay native where possible.
- **Operator-grade safety** — host-key verification, bounded retries, typed IPC, redacted logs, explicit broadcast mode, and isolated local tests.
- **Built to grow** — the architecture leaves room for RDP, VNC, X11, WSL, portable mode, and broader platform support without pretending those gates are already finished.

## What MobaRust can do today

The current product baseline is real, tested, and intentionally narrower than a marketing mockup:

| Remote-work workflow | Current MobaRust capability |
| --- | --- |
| SSH terminal | Native SSH, host-key verification, password/key references, agent path, keyboard-interactive auth, PTY, resize, keepalive, cancellation, reconnect, and actionable errors |
| SFTP / SCP | Integrated remote browser, recursive streaming transfers, bounded concurrency, progress, cancellation, atomic commits, conflict handling, and SCP compatibility |
| Sessions | Saved profiles, folders, tags, favorites, recents, search, OpenSSH import, secret-free export, jump-host chains, and typed settings |
| Tunnels | Local forwarding, remote forwarding, dynamic SOCKS5, explicit lifecycle state, stop controls, and bounded clients |
| Local work | Native local PTY, shell lifecycle, resize, input/output batching, split panes, persistent tabs, and platform-aware WSL foundations |
| Operator tools | Snippets with preview, visible macros, explicit multi-exec/broadcast targets, network diagnostics, port checks, remote monitoring, and audit history without terminal transcripts |
| Legacy equipment | Telnet with clear unencrypted labeling and serial configuration/lifecycle support with graceful device-loss handling |
| Security | Platform vault abstraction, portable encrypted vault, typed Tauri commands, redacted tracing, threat model, fuzz targets, and safe loopback fixtures |

The goal is the kind of workflow people choose instead of MobaXterm for daily administration — not a static terminal screenshot or a thin wrapper around the system `ssh` command.

## The honest status

The roadmap is currently **58/65 items evidenced — approximately 89.2%**. This is an engineering progress measure, not a claim of complete MobaXterm parity.

The remaining work is visible:

- Production RDP through a mature engine, with Windows interoperability, certificate validation, gateway, clipboard, audio, resize, and multi-monitor evidence.
- Cross-platform VNC interoperability beyond local loopback fixtures.
- Real Windows, Linux, and macOS runtime matrices, including WSL, PTYs, clipboard, serial hardware, and multi-monitor behavior.
- Integrated/external X-server strategy, signed portable distributions, notarization, and clean-install release evidence.

Until those gates are proven, RDP and VNC remain clearly marked experiments. MobaRust does not claim a fake 1:1 replacement experience.

## MobaXterm users: the migration path

If you are evaluating alternatives to MobaXterm, the strongest path today is:

1. Start with SSH and local terminal workflows.
2. Move files through the integrated SFTP/SCP surface.
3. Recreate repeatable connections with saved sessions, tags, folders, and OpenSSH config import.
4. Add tunnels, snippets, diagnostics, and remote monitoring as your workspace grows.
5. Track RDP/VNC and platform milestones openly as they become evidenced.

OpenSSH import currently focuses on common fields such as `Host`, `HostName`, `User`, `Port`, `IdentityFile`, `ProxyJump`, and `ServerAliveInterval`. Unsupported directives are reported rather than silently claimed as compatible.

## Start locally

Requirements: Rust stable, Node.js, and pnpm.

```bash
git clone https://github.com/OthmaneBlial/MobaRust.git
cd MobaRust
pnpm install --dir apps/desktop
cargo xtask check
cargo tauri dev --manifest-path apps/desktop/src-tauri/Cargo.toml
```

Run the repository-scoped safety and packaging checks before contributing:

```bash
cargo xtask check
cargo xtask package-check
cargo xtask portable-check
cargo xtask pre-push-check
```

The validation pipeline uses isolated home/XDG directories, repository or temporary fixture paths, and loopback-only protocol servers. It does not need your personal `~/.ssh`, GitHub keys, Keychain, SSH agent, real hosts, or attached hardware.

cargo xtask portable-check is currently a macOS-only local smoke check. It
assembles an unsigned .tar.gz beside the generated bundle, verifies its
contents and SHA-256 manifests, and leaves the artifact under ignored
target/. It is not a signed, notarized, or cross-platform release.

## Architecture

```text
React + TypeScript + xterm.js
                │ typed Tauri commands/events
Rust desktop boundary
  ├─ SSH / SFTP / SCP / jump hosts / tunnels
  ├─ PTY, local shells, terminal tabs and splits
  ├─ session store, settings, import/export and vault references
  ├─ transfers, editor, diagnostics, monitoring and audit events
  └─ isolated RDP / VNC helper experiments
```

The frontend is a presentation and interaction layer. Session configuration references credentials; secrets are not copied through UI state, normal storage, logs, or command-line arguments. Sensitive protocol operations remain in Rust or an explicitly controlled helper process.

## Security and engineering standards

- Unknown SSH host keys are not silently accepted.
- Passwords, private keys, credential tokens, and sensitive environment values are not logged.
- Frontend IPC uses narrow typed commands; there is no unrestricted `execute_anything(command)` bridge.
- Remote filenames and terminal/editor content are treated as untrusted input.
- Transfers, reconnects, diagnostics, helpers, and remote commands have bounded cancellation and timeout behavior.
- `base/` is a local, ignored research corpus and is never part of the application or release payload.
- The RDP candidate remains excluded from normal bundles while its dependency audit reports `RUSTSEC-2023-0071` through `rsa 0.10.0-rc.18`.

Read the [safe testing policy](docs/security/safe-testing.md), [threat model](docs/security/threat-model.md), and [dependency audit record](docs/security/dependency-audit.md).

## Documentation

- [Roadmap](ROADMAP.md)
- [Architecture decisions](docs/adr/)
- [Reference-project research](docs/research/reference-projects.md)
- [RDP research](docs/research/rdp.md)
- [VNC research](docs/research/vnc.md)
- [X11 strategy](docs/research/x11.md)
- [Remote editor decision](docs/adr/0022-bounded-remote-text-editor.md)
- [Keyboard shortcuts](docs/keyboard-shortcuts.md)
- [Hardware and interoperability test matrix](docs/testing/hardware-interoperability.md)
- [Release and packaging notes](docs/release/packaging.md)

## Contributing

Small, testable contributions are welcome. Run the local checks and explain which behavior is covered by unit tests, protocol fixtures, fuzzing, or manual platform evidence.

Please do not include passwords, private keys, host inventories, real connection logs, or personal configuration exports in issues or pull requests.

## License

MobaRust is released under the [Apache License 2.0](LICENSE).
