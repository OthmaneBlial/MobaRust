# PTY platform matrix

This document records the local evidence for the native pseudo-terminal path.
It deliberately separates source-level portability from runtime evidence on
each operating system.

## Fixture contract

`apps/desktop/src-tauri/src/terminal.rs` uses a disposable native PTY and
checks the same contract on every supported desktop target:

- open a PTY with an initial size;
- resize it to a second size;
- write a line through the master side;
- receive an output marker and the echoed input;
- wait for a clean child exit.
- on explicit close, terminate a still-running child and reap it within the
  native close operation; a child that already exited is treated as closed.

The same cleanup rule applies when the output reader reaches EOF or cannot
start: the manager removes the session, terminates a still-live child, and
waits for it before publishing the closed event. If the stream worker itself
cannot be created, the just-opened session is taken back and cleaned up before
the spawn error is returned. This keeps process ownership deterministic across
Unix and Windows even when the terminal disappears before the user clicks
Close.

The fixture command is explicit and platform-specific: `/bin/sh -c ...` on
Unix and `cmd.exe /C ...` on Windows. The local Unix test also launches each
fixed `bash`, `zsh`, and `fish` target that is installed and checks a marker
through a native PTY. The product launch contract now also
accepts only typed shell choices: `powershell.exe`/`cmd.exe` on Windows and
`bash`/`zsh`/`fish` on Unix, with the configured `SHELL` or `ComSpec` retained
for the default target. It does not accept an arbitrary executable from the
frontend, use a login profile for discovery, contact a network endpoint, or
read a personal file.

## Evidence matrix

| Target | Source/test coverage | Runtime evidence | Status |
| --- | --- | --- | --- |
| macOS ARM64 | native PTY fixture and local shell branch | `cargo xtask check` on the local ARM64 host | Verified locally |
| Windows x64 | Windows shell branch with typed PowerShell/cmd targets, WSL parser and conditional discovery path | Requires a real Windows runtime | Pending |
| Linux x64 | Unix shell branch with typed bash/zsh/fish targets and native PTY path | Requires a real Linux desktop runtime | Pending |
| macOS x64 | Same Unix source branch | Requires a separate x64 runtime or artifact | Pending |
| Windows ARM64 | Windows shell branch | Requires a real Windows ARM64 runtime | Pending |
| Linux ARM64 | Unix shell branch | Requires a real Linux ARM64 runtime | Pending |

The current development machine has only the `aarch64-apple-darwin` desktop
target installed. No cross-target installation was performed for this matrix.
The test suite's Windows WSL discovery test is parser-only on macOS/Linux and
does not invoke `wsl.exe`.

## Acceptance gates

Before checking the roadmap's cross-platform PTY item, run the same repository
validation on at least one real Windows x64 and one real Linux x64 environment,
and record:

- PTY creation, resize, input, output batching, and clean close;
- default shell discovery and non-login environment behavior;
- explicit PowerShell/cmd or bash/zsh/fish launch where each executable is installed;
- cancellation when the child exits or disappears;
- explicit close reaping without leaving an unreaped local child;
- Unicode and Windows path handling where applicable;
- clipboard and keyboard shortcuts in the desktop UI;
- WSL distribution discovery and launch on Windows;
- Wayland/X11 behavior on Linux where the terminal window manager matters.

No real Windows/Linux evidence is inferred from a successful macOS compile.
