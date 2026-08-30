# ADR 0032: Explicit Windows WSL terminal target

## Status

Implemented in the native terminal boundary; Windows runtime validation remains
part of the cross-platform PTY release matrix.

## Decision

WSL is exposed as an explicit local terminal target. The desktop runtime offers
`wsl.exe --list --quiet` through a bounded native capability query on Windows.
The UI displays only the returned distribution names. Launching a selected
distribution uses an interactive PTY with explicit `wsl.exe --distribution`
arguments; no shell command string is assembled.

The default local shell remains unchanged. Existing local-terminal shortcuts
continue to open the default shell, while the WSL picker is available from the
New Terminal controls and the command palette.

## Safety boundary

- macOS and Linux return an explicit unsupported result without starting a
  process, reading a WSL file, or probing a device;
- Windows discovery has a three-second timeout and does not inherit a shell
  command line from the user;
- distribution names are trimmed, bounded, and rejected if they contain control
  characters or begin with `-`;
- WSL targets are held in the workspace terminal state and do not contain
  credentials, SSH paths, or vault references;
- the feature does not inspect `~/.ssh`, the SSH agent, GitHub keys, Keychain,
  or any remote host.

## Alternatives considered

- launching `wsl` through a shell: rejected because it creates unnecessary
  command parsing and injection surface;
- accepting a free-form distribution or command: rejected because the picker
  should launch only a discovered distribution and must remain a terminal,
  not an arbitrary command executor;
- using a Windows-specific filesystem or registry API: deferred because the
  documented `wsl.exe` capability query is smaller and works across supported
  Windows versions.

## Verification

The macOS test suite verifies UTF-16 WSL output normalization, duplicate and
option-like name rejection, and the non-Windows unsupported branch. A Windows
validation pass is still required for installed-distribution discovery, PTY
input/output, resize, and clean WSL process cancellation.
