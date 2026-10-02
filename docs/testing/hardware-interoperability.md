# Hardware and interoperability evidence matrix

This document is a runbook for the hardware and cross-platform checks that
remain before MobaRust can claim broad interoperability. It is a test plan,
not evidence that those checks have already passed.

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
| Native PTY | macOS ARM64 local evidence; Ubuntu x64 and macOS ARM64 CI exercise resize, input/output, exit, and child cleanup | Full shell/version coverage, WSL, real clipboard/window-manager behavior |
| SSH | Local SSH fixture covers authentication, PTY I/O, resize, SFTP, and disconnect; a native macOS app smoke test also connected to a disposable loopback server and loaded the SFTP browser | Internet-host interoperability, sustained GUI terminal input, or the operator's SSH configuration |
| Telnet | Local TCP fixture covers negotiation, I/O, reconnect, and cancellation | Security; Telnet remains unencrypted |
| Serial | Disposable pseudo-terminal fixture covers lifecycle and device-loss handling | USB driver, permission, baud/parity, and real-adapter behavior |
| VNC | Isolated helper controls local RFB fixtures, including password auth and reconnect | Mature-engine selection, encrypted transport, and cross-platform packaging |
| RDP | Isolated helper and a real loopback IronRDP server fixture cover TLS/Hybrid authentication, framebuffer, input, resize, and reconnect | Real Windows desktops, platform certificate stores, audio, gateway interoperability, and multi-monitor behavior |

These rows must remain distinct from the release matrix below. A fixture or
unit test must never be promoted to hardware or cross-platform evidence by
inference.

### Automated native CI, 2026-10-02

[Quality run 36996373657](https://github.com/OthmaneBlial/MobaRust/actions/runs/36996373657)
tests source `622dfbc08dd4ce11defcbdc1cbe19268dc6c9921` with Rust
`1.99.0 (b940084d7 2026-09-28)`:

| Runner / compiler host | Observed result | Native fixture scope |
| --- | --- | --- |
| Ubuntu 22.04 / `x86_64-unknown-linux-gnu` | Passed | PTY I/O/resize/exit/cleanup, Unix serial PTY, disposable OpenSSH lab including a dedicated agent |
| macOS 15 / `aarch64-apple-darwin` | Passed | PTY I/O/resize/exit/cleanup, Unix serial PTY, disposable OpenSSH lab including a dedicated agent |
| Windows / `x86_64-pc-windows-msvc` | Failed | Native build succeeds; PowerShell/ConPTY I/O fixture times out; Clippy is not reached |

Each job runs locked frontend unit tests, TypeScript checking, lint, build,
release-asset tests, then `cargo xtask check-rust` (workspace formatting,
shippable VNC-helper staging, workspace tests, and Clippy). Unix shell variants
run only when installed. The OpenSSH suite has conditional IPv6/Xvfb checks;
a passing harness result alone does not prove an optional fixture executed.
The current check command retains successful-test output to expose explicit
skip messages in subsequent runs.

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

## Required matrix

| Target | PTY / shell | SSH / SFTP | Serial adapter | RDP | VNC | Clipboard / display | Status |
| --- | --- | --- | --- | --- | --- | --- | --- |
| macOS ARM64 | Native CI/local fixtures; GUI checks remain | Local/CI OpenSSH; broader manual checks remain | Dedicated adapter required | Dedicated server required | Dedicated server required | Record native behavior | Partial fixture/local evidence |
| macOS x64 | Separate runtime required | Separate runtime required | Dedicated adapter required | Dedicated server required | Dedicated server required | Record native behavior | Pending |
| Windows x64 | Real Windows runtime, PowerShell/cmd, and WSL | Real Windows runtime | Dedicated adapter/driver required | Real Windows RDP server required | Dedicated server required | Clipboard, DPI, multi-monitor | Pending |
| Windows ARM64 | Real Windows ARM64 runtime | Real Windows ARM64 runtime | Dedicated adapter/driver required | Real Windows RDP server required | Dedicated server required | Clipboard, DPI, multi-monitor | Pending |
| Linux x64 | Native CI fixture; X11/Wayland GUI checks remain | CI OpenSSH fixture; manual checks remain | Dedicated adapter/permissions required | Dedicated server required | Dedicated server required | Clipboard and window manager | Partial CI fixture evidence |
| Linux ARM64 | Separate runtime required | Separate runtime required | Dedicated adapter/permissions required | Dedicated server required | Dedicated server required | Clipboard and window manager | Pending |

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
