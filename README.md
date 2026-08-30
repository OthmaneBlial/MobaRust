# MobaRust

## Every machine. One control room.

MobaRust is a Rust-first, open-source remote workstation for people who operate systems. It brings terminals, file movement, tunnels, saved connections, diagnostics, and remote-desktop experiments into one focused desktop workspace.

[Open the project site](https://othmaneblial.github.io/MobaRust/) · [View the roadmap](ROADMAP.md) · [Report an issue](https://github.com/OthmaneBlial/MobaRust/issues)

![MobaRust operations workspace](apps/desktop/src-tauri/icons/icon.svg)

MobaRust is built around a simple promise: make the next remote operation obvious, fast, and safe to inspect before it runs.

## Why it exists

Modern operators rarely need one protocol. They need a terminal, an SFTP browser, a tunnel, a quick diagnostic, a serial console, a saved profile, and a reliable way to move between them without losing context.

MobaRust is the attempt to make that workflow feel like one coherent control room instead of a pile of disconnected utilities.

## What is real today

The current engineering baseline is intentionally evidence-led:

- Native SSH sessions with host-key verification, password/key authentication paths, PTY terminals, cancellation, timeouts, structured errors, and reconnect state handling.
- Integrated SFTP and SCP primitives, recursive transfers, bounded progress, cancellation, conflict-aware remote editing, and transfer history.
- A Rust-owned terminal boundary with typed Tauri commands, local terminal support, link safety, title/zoom behavior, and paste safeguards.
- SSH forwarding and tunnels with explicit lifecycle management rather than opaque background processes.
- Saved sessions, groups, tags, typed settings, migrations, import/export foundations, snippets, visible macros, and explicitly selected broadcast targets.
- A local encrypted vault boundary, privacy-conscious logging, threat-model documentation, and isolated validation that does not require personal machine credentials.
- Telnet and serial foundations for legacy equipment, plus focused network diagnostics and optional remote monitoring paths.
- RDP and VNC experiments kept behind controlled helper boundaries while real interoperability and packaging evidence is still being completed.

This repository favors a smaller number of honest capabilities over a longer list of screenshots or placeholders.

## The honest frontier

The roadmap is approximately **89% evidenced** against the current engineering checklist. That is a useful progress signal, not a claim of complete MobaXterm parity.

The remaining frontier includes:

- Production-grade RDP integration with a mature engine, Windows evidence, and the full clipboard/audio/gateway/resizing matrix.
- Cross-platform VNC interoperability and manual evidence beyond the local loopback fixtures.
- Real Windows/Linux/macOS runtime and hardware matrices, including PTYs, serial devices, WSL, clipboard, and multi-monitor behavior.
- Integrated X-server strategy, signed portable distributions, clean-install evidence, and final packaging/release hardening.

These limits are visible on purpose. Contributions and issue reports should turn them into measured evidence, not marketing claims.

## Built for operators

### One workspace, many operations

Keep the shell, remote files, tunnel state, and session context close together. The UI is designed for dense daily use without turning every action into a modal detour.

### Native where it matters

Rust owns protocol boundaries, cancellation, filesystem policy, storage, and sensitive operations. The React frontend receives narrow, typed capabilities instead of an unrestricted shell or filesystem bridge.

### Safe by default

The local test harness isolates `HOME` and XDG directories, strips credential-related environment variables, uses loopback-only protocol fixtures, and never needs to inspect your personal `~/.ssh`, Keychain, GitHub keys, or real hosts.

The application is also designed around explicit host-key and certificate decisions, redacted logs, bounded retries, operation-specific timeouts, and visible broadcast mode.

## Start locally

Requirements: Rust stable, Node.js, and pnpm.

```bash
git clone https://github.com/OthmaneBlial/MobaRust.git
cd MobaRust
pnpm install --dir apps/desktop
cargo xtask check
cargo tauri dev --manifest-path apps/desktop/src-tauri/Cargo.toml
```

For the isolated local validation path:

```bash
cargo xtask check
cargo xtask package-check
cargo xtask pre-push-check
```

Validation uses repository-local or temporary state and loopback fixtures. It does not connect to production machines or read personal credential stores.

## Architecture

```text
React / TypeScript UI
        │ typed Tauri commands and events
Rust desktop boundary
        ├── session orchestration and cancellation
        ├── SSH / SFTP / SCP / tunnels
        ├── PTY and local terminal lifecycle
        ├── encrypted vault and versioned store
        ├── transfer, editor, diagnostics, and monitoring services
        └── isolated RDP / VNC helper experiments
```

The architecture is deliberately protocol-aware. A session configuration references credential material; it does not duplicate secrets through every UI state object or log event.

## Documentation

- [Roadmap](ROADMAP.md)
- [Safe testing policy](docs/security/safe-testing.md)
- [Threat model](docs/security/threat-model.md)
- [Architecture decisions](docs/adr/)
- [Research notes](docs/research/)
- [Keyboard shortcuts](docs/keyboard-shortcuts.md)
- [Release and packaging notes](docs/release/packaging.md)

## Contributing

Useful contributions are small, testable, and explicit about platform evidence. Before opening a pull request, run the isolated checks above and explain which behavior is covered by unit tests, fixtures, or manual verification.

Do not include private keys, passwords, host inventories, real connection logs, or personal configuration exports in issues or pull requests.

## License

See [LICENSE](LICENSE).
