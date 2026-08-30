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
| macOS ARM64 PTY | Native disposable-PTY fixture passes locally; explicit bash/zsh/fish targets are bounded in the local contract | Windows/Linux shell runtime behavior, real clipboard/window-manager behavior |
| SSH | Local SSH fixture covers authentication, PTY I/O, resize, SFTP, and disconnect | Internet-host interoperability or the operator's SSH configuration |
| Telnet | Local TCP fixture covers negotiation, I/O, reconnect, and cancellation | Security; Telnet remains unencrypted |
| Serial | Disposable pseudo-terminal fixture covers lifecycle and device-loss handling | USB driver, permission, baud/parity, and real-adapter behavior |
| VNC | Isolated helper controls local RFB fixtures, including password auth and reconnect | Mature-engine selection, encrypted transport, and cross-platform packaging |
| RDP | Isolated helper and wire/lifecycle tests only | A real desktop, Windows interoperability, certificate validation, audio, gateway, and multi-monitor behavior |

These rows must remain distinct from the release matrix below. A fixture or
unit test must never be promoted to hardware or cross-platform evidence by
inference.

## Required matrix

| Target | PTY / shell | SSH / SFTP | Serial adapter | RDP | VNC | Clipboard / display | Status |
| --- | --- | --- | --- | --- | --- | --- | --- |
| macOS ARM64 | Run and record | Run local fixture and manual UI checks | Dedicated adapter required | Dedicated server required | Dedicated server required | Record native behavior | Partial local evidence |
| macOS x64 | Separate runtime required | Separate runtime required | Dedicated adapter required | Dedicated server required | Dedicated server required | Record native behavior | Pending |
| Windows x64 | Real Windows runtime, PowerShell/cmd, and WSL | Real Windows runtime | Dedicated adapter/driver required | Real Windows RDP server required | Dedicated server required | Clipboard, DPI, multi-monitor | Pending |
| Windows ARM64 | Real Windows ARM64 runtime | Real Windows ARM64 runtime | Dedicated adapter/driver required | Real Windows RDP server required | Dedicated server required | Clipboard, DPI, multi-monitor | Pending |
| Linux x64 | X11 and Wayland runtimes | Real Linux runtime | Dedicated adapter/permissions required | Dedicated server required | Dedicated server required | Clipboard and window manager | Pending |
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
