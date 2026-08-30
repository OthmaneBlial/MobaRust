# ADR 0035: Keep local shell selection typed and platform-aware

## Status

Accepted in the local PTY implementation. Runtime coverage on Windows and
Linux remains part of the hardware/interoperability matrix.

## Context

The local terminal must cover the shells operators already use: PowerShell and
cmd on Windows, WSL distributions on Windows, and bash, zsh, fish, or the
configured user shell on Unix systems. A free-form executable field would
expand the native process-launch surface and make the frontend responsible for
shell policy.

## Decision

`LocalTerminalTarget::Default` carries an optional typed `LocalShell` value.
Missing values deserialize as `default`, preserving existing local-terminal
profiles. The native terminal boundary maps the enum to a fixed executable:

- Windows: `powershell.exe`, `cmd.exe`, or the configured `ComSpec`;
- macOS/Linux: `bash`, `zsh`, `fish`, or the configured `SHELL`;
- Windows WSL remains a separate `wsl.exe --distribution <validated-name>`
  target.

The frontend exposes only the shell choices supported by the detected desktop
platform. It never accepts an arbitrary executable path for this feature and
never constructs a shell command string. Working directory, environment, and
startup-command validation remain unchanged and are applied before a PTY is
opened. The frontend's `startupCommand` field is explicitly mapped to the
native `startup_command` field at the serde boundary, so saved local profiles
keep the same typed payload contract as newly created terminals.

An explicitly unsupported choice returns a stable platform error before any
process or filesystem discovery. A missing Unix shell is allowed to fail at
the normal PTY spawn boundary with the operating system's unavailable-process
path; MobaRust does not scan the machine to discover arbitrary shells.

## Consequences

This gives Windows users first-class PowerShell/cmd entry points and gives
macOS/Linux users explicit bash/zsh/fish entry points while preserving the
configured default shell. It keeps the launch contract auditable and avoids
turning shell selection into an unrestricted process-execution API.

The local tests prove legacy JSON compatibility, the camelCase startup-command
payload, fixed executable mapping, and fail-closed behavior for a
platform-incompatible choice. Real Windows
PowerShell/cmd, WSL, Linux shell, and installed-shell behavior still require
target-runtime validation and must be recorded separately from macOS fixture
evidence.
