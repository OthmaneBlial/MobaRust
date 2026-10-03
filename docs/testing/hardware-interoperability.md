# Hardware and interoperability evidence matrix

This document records controlled evidence and the hardware/cross-platform
checks that remain before MobaRust can claim broad interoperability. The
required matrix is a test plan; pending checks have not passed.

## Safety boundary

Run this matrix only in a dedicated lab environment with explicit operator
approval. The lab should use disposable fixtures, non-production hosts, and a
dedicated serial adapter or test appliance. Never use a personal SSH setup,
production server, personal clipboard history, or a private key from the
operator's normal machine.

The test operator must:

- keep the repository and test artifacts inside the project workspace or an
  explicitly disposable temporary directory;
- use a sanitized process environment and a separate test home directory;
- use fixture-only credentials, stored only in the native test vault or passed
  through the documented test channel;
- redact hostnames, usernames, device serial numbers, filesystem paths, and
  remote output before saving evidence;
- stop the application and disconnect the fixture before removing hardware;
- delete only the dedicated temporary test artifacts after the run.

The test must not inspect or copy `~/.ssh`, SSH agent state, Keychain data,
browser data, personal configuration, or unrelated files. A missing lab device
or server is a pending result, not a reason to probe the local machine.

## Current repository evidence

| Area | Safe evidence available now | What it does not prove |
| --- | --- | --- |
| Native PTY | macOS ARM64 local evidence; Ubuntu x64, macOS ARM64, and Windows x64 CI exercise resize, input/output, exit, and child cleanup | Full shell/version coverage, WSL, real clipboard/window-manager behavior |
| SSH / SFTP / tunnels | Local/CI SSH fixtures plus the [native ARM64 workflow receipt](native-workflow.md): terminal input/output, SFTP editing/save and download, local/remote/SOCKS5 HTTP round trips and listener cleanup | Internet-host interoperability, sustained GUI use, wider retry/authentication cases, or the operator's SSH configuration |
| Telnet | Local TCP fixture covers negotiation, I/O, reconnect, and cancellation | Security; Telnet remains unencrypted |
| Serial | Disposable pseudo-terminal fixture covers lifecycle and device-loss handling | USB driver, permission, baud/parity, and real-adapter behavior |
| VNC | Isolated helper controls local RFB fixtures, including password auth and reconnect | Mature-engine selection, encrypted transport, and cross-platform packaging |
| RDP | Isolated helper and a real loopback IronRDP server fixture cover TLS/Hybrid authentication, framebuffer, input, resize, and reconnect | Real Windows desktops, platform certificate stores, audio, gateway interoperability, and multi-monitor behavior |

These rows must remain distinct from the release matrix below. A fixture or
unit test must never be promoted to hardware or cross-platform evidence by
inference.

### Automated native CI, 2026-10-02

[Quality run 36998538983](https://github.com/OthmaneBlial/MobaRust/actions/runs/36998538983)
passed all three jobs on source `ac70e39c98b54d8f2a444b0c4b3232486fd6e3bf` with Rust
`1.99.0 (b940084d7 2026-09-28)`:

| Runner / compiler host | Observed result | Native fixture scope |
| --- | --- | --- |
| Ubuntu 22.04 / `x86_64-unknown-linux-gnu` | Passed | PTY I/O/resize/exit/cleanup, Unix serial PTY, disposable OpenSSH lab including a dedicated agent |
| macOS 15 / `aarch64-apple-darwin` | Passed | PTY I/O/resize/exit/cleanup, Unix serial PTY, disposable OpenSSH lab including a dedicated agent |
| Windows 2025 / `x86_64-pc-windows-msvc` | Passed | Native PowerShell and cmd ConPTY round trips, resize/exit/cleanup, workspace tests, and Clippy |

Each job runs locked frontend unit tests, TypeScript checking, lint, build,
release-asset tests, then `cargo xtask check-rust` (workspace formatting,
shippable VNC-helper staging, workspace tests, and Clippy). Unix shell variants
run only when installed. The OpenSSH suite has conditional IPv6/Xvfb checks;
a passing harness result alone does not prove an optional fixture executed.
Successful-test output records bash 5.1.16 on Ubuntu (zsh/fish unavailable)
and bash 3.2.57 plus zsh 5.9 on macOS (fish unavailable). IPv6 and the real
Xvfb fixture executed on Ubuntu; IPv6 executed on macOS, where Xvfb was
explicitly skipped. Windows shell versions were not recorded.

GitHub Quality and installer workflows were disabled after this successful
baseline at the maintainer's request. The follow-up run for source `6bbf349`
was cancelled. Its new SSH restart case passed locally; the added Linux
zsh/fish installation and three consecutive Windows startup checks have no
completed CI receipt. Quality has no push/PR triggers; use local checks.

These are native CI fixtures, not GUI, WSL, USB serial, clean installation,
signed release, experimental-helper, or external-server evidence. The local
macOS full suite additionally checks the isolated RDP/VNC helpers, loopback
RDP fixture, synthetic package layouts, and fuzz-target compilation. Local
IPv6 loopback and dedicated-agent execution passed; real Xvfb was skipped.

On 2026-09-27, an unsigned macOS ARM64 debug bundle built from `1f9bd93`
passed `cargo xtask package-check`. A separate portable copy opened a native
window without loading the installed app's sessions. A saved session connected
to a loopback OpenSSH server with a generated key and an explicit fixture
`known_hosts` file. The server accepted the key, the app recorded a successful
connection, and the SFTP browser loaded a directory. Its parent control also
completed without a listing error. The focused real-PTY loopback test passed.
The GUI run did not verify sustained shell input, file transfer, or a remote
host. The app and server were stopped, and generated SSH keys were removed.

On 2026-09-27, source `a6c1b47` also passed `cargo xtask package-check` and
`cargo xtask portable-check` on macOS ARM64. The unsigned app bundle, bundled
VNC helper, portable archive, and SHA-256 manifests passed their local checks.
The archive SHA-256 was
`352f0378a3f1724e2d1ec1589cf6f30c91a82537a4c5a885de48b3bdcbcaa8fd`.
The startup probe exercised only `mobarust --version`; this run did not open a
GUI or add interoperability evidence.

On 2026-09-27, source `04875af` passed both packaging checks again on macOS
ARM64 after the SSH and session-import fixes. The unsigned app bundle, VNC
helper, checksums, and portable archive were verified. The archive SHA-256 was
`8b5b2c4cc821da44928264128aaeb6508f1666dfd675c82305e05d9c6f93cedb`.
The packaged executable returned `MobaRust 0.1.12` on its CLI startup probe.
This run did not open the GUI or test a packaged SSH connection.

On 2026-09-27, tag `v0.1.13` produced locally built macOS ARM64 and x64 DMGs.
Both disk images passed `hdiutil verify`; the mounted apps passed package layout
and ad hoc signature checks. `lipo` confirmed that each app executable and VNC
helper matched its labeled architecture, and each packaged executable returned
`MobaRust 0.1.13` for `--version`. The four uploaded release assets were
downloaded again and their SHA-256 files passed. The workflow stayed disabled.
This does not establish GUI behavior, clean installation, or Intel hardware
interoperability.

On 2026-09-27, v0.1.14 source passed `cargo xtask check`, including local
RDP/VNC loopback fixtures and fuzz-target compilation. Locally built ARM64 and
x64 DMGs passed `hdiutil verify`; their mounted apps passed package layout and
ad hoc signature checks. `lipo` confirmed the app and VNC helper architectures,
and both packaged executables returned `MobaRust 0.1.14` for `--version`.
The collected DMGs passed their SHA-256 files. No GUI or Intel hardware session
was exercised in this run.

On 2026-09-27, v0.1.15 source passed `cargo xtask check`, including RDP/VNC
loopback fixtures. Locally built ARM64 and x64 DMGs passed `hdiutil verify`;
their mounted apps passed package layout and ad hoc signature checks. `lipo`
confirmed each app and VNC helper architecture, and both packaged executables
returned `MobaRust 0.1.15` for `--version`. The collected DMGs passed their
SHA-256 files. No GUI or Intel hardware session was exercised in this run.

On 2026-09-27, v0.1.16 source passed `cargo xtask check`, including the macro
target-binding regression test and local RDP/VNC fixtures. Locally built ARM64
and x64 DMGs passed `hdiutil verify`; their mounted apps passed package layout
and ad hoc signature checks. `lipo` confirmed each app and VNC helper
architecture, and both packaged executables returned `MobaRust 0.1.16` for
`--version`. The collected DMGs passed their SHA-256 files. This run did not
open the GUI or exercise an Intel Mac hardware session.

On 2026-10-02, the v0.1.17 preview published locally built ARM64 and Intel
DMGs from application source `7713783`, tagged with documentation at `74d2bb4`.
The final images were mounted read-only with valid disk checks; mounted apps
passed layout and strict ad hoc signature checks. App/VNC-helper architectures,
packaged CLI versions and both checksum manifests passed. The [native ARM64
receipt](native-workflow.md) and [94-second recording](../release/desktop-demo.md)
add short GUI workflow evidence. All six published release assets were
downloaded again and matched the local files byte for byte. Both GitHub
workflows remained disabled; no tag CI run occurred. Intel CLI startup ran
through Rosetta on ARM64, not on Intel hardware. Signing, notarization and
clean-install gates remain open.

On 2026-10-02, v0.1.18 at tag commit `b0ae2c3` passed the full local suite. Locally built
ARM64/x64 DMGs passed disk-image verification, read-only layout, strict ad hoc
signature, matching app/helper architectures, CLI version and SHA-256 manifests.
The [ARM64 release-copy receipt](remote-desktop-renderer.md#v0118-release-bundle-recheck)
records Full-HD rendering, keyboard input before/after reconnect, focused
settings errors with unchanged persistence, and native Quit releasing an active
zsh PTY and VNC helper. All fixture listeners were loopback-only and stopped.
All four published assets were downloaded again and matched local files byte
for byte; both downloaded SHA-256 manifests passed. Quality and installer
workflows stayed disabled. Intel CLI startup used Rosetta; no Intel GUI or
clean-install result is claimed.
Fullscreen entry was observed in release accessibility state, while complete
fullscreen input/exit acceptance remains the separate ARM64 debug receipt.

After v0.1.18, the [native SSH retry receipt](ssh-reconnect.md) observed two
failed reconnect attempts, explicit fresh-profile recovery, accurate final
error/closed presentation, normal shell exit and Quit releasing an active SSH
session plus local PTY in isolated ARM64 debug copies. Both fixture listeners
were verified loopback-only, stopped, and their ports closed. This post-tag
correction is not included in the unchanged public v0.1.18 DMGs. Wider retry
budgets, actual daemon-restart GUI recovery and other platforms remain pending.

## Offline Linux ARM64 backend runtime, 2026-10-03

Source `ecb382ff6e19f6b7a0da5ef0122e9934e0215184` executed seven static musl
SSH harnesses in Ubuntu 24.04.4 ARM64 under QEMU/HVF: **128 passed, zero
failed, five ignored**. IPv6 known-hosts and both two-bastion 8 MiB shell/SFTP
cases ran; real Xvfb skipped. The [receipt](linux-ssh-vm.md) records the signed
image, compiler setup, executable hashes, exact scope and disposable cleanup.

The guest had no network adapter, host forwarding or shared host directories;
fixture traffic remained on guest loopback. No guest test-account processes
or TCP/UDP listeners remained before shutdown. QEMU exited and its private
credential-bearing disks/seeds were removed. This proves Linux SSH backend
execution, not a native desktop/glibc build, local-shell PTY matrix, GUI,
installer or hardware interoperability. Workflows remain disabled.

## Required matrix

| Target | PTY / shell | SSH / SFTP | Serial adapter | RDP | VNC | Clipboard / display | Status |
| --- | --- | --- | --- | --- | --- | --- | --- |
| macOS ARM64 | Native CI/local fixtures and short GUI input/split checks; broader shell checks remain | Local/CI OpenSSH plus GUI SFTP edit/download and three tunnel modes; sustained/recovery checks remain | Dedicated adapter required | Dedicated server required | Dedicated server required | Record native behavior | Partial fixture/local GUI evidence |
| macOS x64 | Separate runtime required | Separate runtime required | Dedicated adapter required | Dedicated server required | Dedicated server required | Record native behavior | Pending |
| Windows x64 | PowerShell/cmd native CI fixtures; WSL and GUI checks remain | Real Windows runtime required | Dedicated adapter/driver required | Real Windows RDP server required | Dedicated server required | Clipboard, DPI, multi-monitor | Partial CI fixture evidence |
| Windows ARM64 | Real Windows ARM64 runtime | Real Windows ARM64 runtime | Dedicated adapter/driver required | Real Windows RDP server required | Dedicated server required | Clipboard, DPI, multi-monitor | Pending |
| Linux x64 | Native CI fixture; X11/Wayland GUI checks remain | CI OpenSSH fixture; manual checks remain | Dedicated adapter/permissions required | Dedicated server required | Dedicated server required | Clipboard and window manager | Partial CI fixture evidence |
| Linux ARM64 | Local-shell PTY matrix still required | [Offline Ubuntu guest](linux-ssh-vm.md) ran cross-built static musl SSH/SFTP fixtures; GUI and broader server checks remain | Dedicated adapter/permissions required | Dedicated server required | Dedicated server required | Clipboard and window manager | Partial backend VM evidence |

## Serial test cases

Use a loopback adapter or a disposable test appliance. Record the configured
device parameters without recording private device identifiers.

1. Refresh devices and verify that only the explicitly selected fixture is
   shown.
2. Connect with baud rate, data bits, stop bits, parity, and flow control set
   explicitly.
3. Exchange a bounded UTF-8 line and verify the configured line ending.
4. Resize or reopen the terminal view without losing the connection state.
5. Remove the adapter or stop the fixture and verify a recoverable device-loss
   state rather than a crash.
6. Reattach the same dedicated fixture, refresh, reconnect explicitly, and
   verify that no stale handle is reused.
7. Cancel during open, read, write, and reconnect; each operation must return
   within its documented timeout.

No test should write arbitrary firmware, change host configuration, or run a
shell command on the attached device.

## Remote-protocol test cases

For RDP and VNC, use a dedicated local or lab server and record the exact
engine/helper version. Verify, as applicable:

- connect, authentication failure, cancellation, disconnect, and bounded
  reconnect;
- negotiated resolution, local scaling, keyboard, mouse, and pointer release;
- clipboard only after an explicit user action, with bounded text and no
  automatic execution;
- certificate/transport policy and the displayed security warning;
- fullscreen, resize, color depth, audio, gateway, and multi-monitor behavior;
- helper crash containment and cleanup of the child process.

Unsupported capabilities must produce an explicit diagnostic. A screenshot,
mock framebuffer, or successful helper startup is not interoperability
evidence.

## Evidence record template

Copy this template for a dedicated run. Replace every value with a redacted,
non-sensitive description before committing or sharing it.

```text
date_utc:
app_commit:
os_and_arch:
runtime_or_fixture:
protocol_and_version:
test_case:
result: passed | failed | pending
observed_lifecycle:
limitations:
artifact_paths: workspace-local and redacted
operator_notes: no secrets, host inventories, or private paths
```

The roadmap item remains open until at least one real Windows runtime, one
real Linux runtime, and the dedicated hardware cases have been executed and
reviewed. External interoperability results must be recorded separately from
the local deterministic test suite.
