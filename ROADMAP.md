# MobaRust roadmap

**Updated 2026-10-02.** The next goal is a dependable remote workstation across Windows, macOS, and Linux. Work is ordered by reliability, security, and operator value.

A checked item means its stated implementation or test exists. It does not imply signed installers, broad server compatibility, or hardware certification. Changes on `main` and published downloads are separate milestones.

## Where the project stands

| Area | Progress | What remains |
| --- | --- | --- |
| **Published previews** | macOS ARM64/x64 v0.1.17; Windows/Linux x64 v0.1.12 | Align platform releases; verify clean install/uninstall; signing and notarization. |
| **Core workstation** | Rust/Tauri shell, xterm.js, tabs, nested splits, settings, session organization, and native PTY | Complete the shell/platform and GUI evidence matrix. |
| **SSH and files** | Interactive SSH, jump chains, reconnect, SFTP/SCP, recursive transfers, tunnels, and remote editing | Broader authentication/server matrix, restart recovery, and sustained workloads. |
| **Quality checks** | Full local macOS ARM64 suite; one completed green Ubuntu/macOS/Windows run | GitHub workflows are disabled by request. New Linux shell variants and repeated Windows startups still need runtime evidence. |
| **Remote desktop** | Isolated RDP/VNC helpers and controlled loopback fixtures | RDP dependency/certificate gates; real-server sessions; platform packaging and long-run stability. |
| **Hardware and distribution** | Unix serial PTY fixtures and unsigned package-layout checks | Real serial adapters, external X servers, GUI behavior, and trusted installers. |

Sources: [native/platform evidence](docs/testing/hardware-interoperability.md), [SSH lab](docs/testing/ssh-lab.md), [dependency audit](docs/security/dependency-audit.md), and [release notes](docs/release/v0.1.17.md).

## Completed in the current improvement cycle

These changes are included in the v0.1.17 Mac preview. Windows/Linux installers remain v0.1.12.

- [x] Keep established quiet SSH sessions alive beyond their setup deadline.
- [x] Preserve exit status sent after channel EOF, so normal shell exit does not become a reconnect.
- [x] Distinguish rejected host keys, unreadable trust files, and transport failures; preserve redacted errors.
- [x] Add `cargo xtask test-ssh` with disposable HOME/state, generated credentials, encrypted-key rejection/recovery, and stalled-handshake cancellation/cleanup.
- [x] Test a distinct two-bastion/target topology, per-hop key rejection, and Unicode SFTP over the chain.
- [x] Exercise empty/wrong-key rejection and successful signing with a dedicated Unix SSH agent.
- [x] Exercise `::1` and explicit IPv6 known_hosts locally; expose a skip when loopback IPv6 is unavailable.
- [x] Interrupt an established loopback SSH transport, reject connections while its server is stopped, and recover after restart with unchanged keys/trust.
- [x] Verify frontend, native workspace tests, and Clippy on Ubuntu, macOS, and Windows in [Quality run 36998538983](https://github.com/OthmaneBlial/MobaRust/actions/runs/36998538983), source `ac70e39`.
- [x] Include the Windows resource icon required by ordinary native builds; Windows now reaches its runtime tests.
- [x] Fix release-asset tests launched from a different working directory and include them in the local suite.
- [x] Add contributor/security/architecture guides and GitHub report/PR templates.
- [x] Refresh advisory records and publish a benchmark receipt with hardware, samples, and observed CPU load.
- [x] Replace native tunnel browser prompts with an integrated form; verify invalid-port rejection, all three modes and listener cleanup on macOS ARM64.
- [x] Build and check both v0.1.17 Mac DMGs and record the real native workflows.

The SSH lab reports **41 unit tests and 11 integration test cases**. IPv6 and real Xvfb have conditional prerequisites; harness success alone does not prove those optional cases ran. The local IPv6 case ran successfully; real Xvfb was skipped.

## Next, in priority order

### 1. Maintain the native validation baseline locally

**Completed baseline:** Windows builds, PowerShell/cmd PTY tests, workspace tests, and Clippy pass in the recorded three-platform run. An earlier PowerShell startup timed out; the fixture now uses direct console output and joins its reader during cleanup. GitHub Quality and installer workflows are disabled at the maintainer's request; Quality has no push/PR triggers.

- [x] Add direct console fixture I/O, child/reader cleanup, and a distinct cmd round trip with bounded deadlines.
- [x] Run workspace tests and Clippy to completion on Windows x64.
- [x] Obtain one completed green Quality run across Ubuntu, macOS, and Windows.
- [ ] Run the stricter three-startup PowerShell/cmd checks on a dedicated Windows runtime and exercise zsh/fish on Linux; these additions were locally validated but their follow-up CI run was cancelled when CI was disabled.

**Next gate:** every repeated startup must produce expected output, exit successfully, and release disposable state. Retain local validation receipts; the recorded CI baseline does not prove the newer repeated-startup cases ran.

### 2. Turn shell and platform assumptions into evidence

- [ ] Exercise PowerShell and cmd input/output, resize, cancellation, and exit separately.
- [ ] Validate WSL discovery and terminal startup on a dedicated Windows runtime.
- [x] Record installed bash/zsh/fish variants and explicit skips on Unix; Linux zsh/fish runtime coverage remains pending.
- [ ] Exercise native GUI focus, paste, split-pane lifecycle, and application shutdown on Windows, macOS, and Linux.

**Done when:** the [platform matrix](docs/testing/hardware-interoperability.md) records OS/architecture, shell version, source commit, exact command, result, and limits. CI fixtures and GUI/manual results remain separate.

### 3. Expand the OpenSSH interoperability lab

- [ ] Exercise the desktop automatic reconnect loop and retry budget against a restarting server; transport interruption and explicit fresh-connection recovery are now covered.
- [ ] Add password and keyboard-interactive/PAM/MFA cases using dedicated disposable accounts or servers.
- [ ] Cover Windows Pageant/alternative agents and additional OpenSSH versions.
- [ ] Exercise sustained output, interrupted large transfers, and routed IPv6 in a controlled lab.

**Done when:** each added case has success, rejection/failure, bounded cancellation, and cleanup checks, runnable through the existing lab command. No personal accounts, agents, or production hosts are test prerequisites.

### 4. Align and verify the next preview release

- [ ] Build Windows/Linux previews containing the latest source fixes alongside both Mac architectures.
- [ ] Verify startup, native helper/resources, clean installation/uninstallation, and downloaded SHA-256 manifests per target.
- [x] Validate v0.1.17 Mac notes, versions, architectures and artifact names; Windows/Linux remain pending.
- [ ] Establish Windows publisher signing, macOS Developer ID/notarization, and a maintainable signed distribution path.

**Done when:** verified artifacts and their limitations are documented per platform. GitHub workflows remain disabled. Signing requires real credentials/infrastructure and is a separate gate from unsigned previews.

### 5. Polish daily terminal and file workflows

- [ ] Exercise keyboard navigation, focus return, resize, reconnect, and failure recovery in the native app.
- [ ] Validate multi-file/recursive transfers, collision decisions, cancellation, and progress under realistic workloads.
- [x] Verify a basic native remote-editor save and a byte-matched SFTP download in the disposable macOS ARM64 lab.
- [ ] Verify remote-editor conflict recovery and save-as behavior through a complete UI workflow.
- [x] Record a new native macOS ARM64 demo with authenticated disposable SSH, SFTP editing/download and a working tunnel; redact account labels and local paths.

**Done when:** the workflows can be completed and recovered using the keyboard, errors explain the next action, and recordings show the tested product rather than synthetic states. The [v0.1.17 demo provenance](docs/release/desktop-demo.md) and [native lab receipt](docs/testing/native-workflow.md) record the tested scope. Broader failure recovery remains open.

## Experimental work and beta gates

| Area | Already implemented | Gate before broader support claims |
| --- | --- | --- |
| **RDP** | Isolated IronRDP helper, credential/IPC boundaries, framebuffer/input/resize/reconnect loopback tests | Resolve the RSA advisory and certificate-validation boundary; authenticate against a controlled Windows desktop; verify framebuffer, keyboard/mouse, resize, failure recovery, and a 30-minute session. Keep it out of normal bundles until security gates pass. |
| **VNC** | Native helper, controlled RFB/password/JPEG fixtures, input, clipboard opt-in, quality controls, and reconnect | Validate a real controlled VNC server, explicit transport policy, unsupported capability diagnostics, a 30-minute session, and helper packaging on each target. TCP remains unencrypted. |
| **X11** | Opt-in SSH channel bridge to an explicitly selected external display; loopback tests and optional Xvfb fixture | Run real external X-server cases on Linux, macOS, and Windows; verify authentication, DISPLAY setup, cancellation, and cleanup. |
| **Serial** | Native configuration, refresh, sessions, and Unix pseudo-terminal lifecycle/device-loss tests | Test dedicated physical adapters, drivers/permissions, baud/parity/flow control, removal, and explicit reconnection per OS. |

## Implemented foundation

These existing capabilities should be improved rather than recreated:

- [x] Typed Rust connection/transfer state, native PTY batching, tabs, nested split panes, resize, and explicit close.
- [x] Session folders, tags, favorites, recents, search, OpenSSH import with compatibility reporting, and secret-free import/export.
- [x] SSH trust policy, password/key/agent auth, encrypted keys, keyboard-interactive path, per-hop jump transport, and bounded reconnect.
- [x] SFTP browsing/file actions, streaming SCP/SFTP, recursive SFTP, a bounded transfer queue, cancellation, and explicit file promotion.
- [x] Local/remote forwarding, SOCKS5 tunnels, bounded DNS/TCP/ping/traceroute diagnostics, and opt-in remote monitoring.
- [x] OS credential references, a marker-gated encrypted portable vault, atomic settings persistence, and optional bounded audit history.
- [x] Reviewed snippets, variables and preview, cancellable macros/recording, explicit broadcast targets, and emergency disable.
- [x] Remote text editing with conflict detection, syntax highlighting, search/replace, encoding selection, and save-as.
- [x] Telnet transport/UI and native serial transport/UI with controlled fixtures.
- [x] Fuzz targets, synthetic package-layout contracts, and terminal/session benchmark tooling.

## Performance and maintenance

- [ ] Repeat terminal batching and 10K-profile benchmarks under comparable idle conditions; record hardware, compiler, samples, medians, and load.
- [ ] Measure sustained native terminal rendering and UI responsiveness before claiming throughput improvements.
- [ ] Recheck advisories at release time; keep RDP's known-vulnerable dependency path isolated and track the remaining GTK/glib warnings.

The [2026-10-02 benchmark receipt](benchmarks/2026-10-02-local.md) was measured under high CPU contention. It is reproducible evidence of that run, not a before/after performance claim.
