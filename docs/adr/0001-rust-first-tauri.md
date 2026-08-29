# ADR 0001: Rust-first desktop boundary

## Status

Accepted for the first vertical slice; revisit after the SSH and remote-desktop prototypes.

## Decision

MobaRust uses a Rust workspace for connection/session state, process and PTY ownership, transfer orchestration, and future protocol adapters. Tauri 2 provides the desktop boundary. React and TypeScript own presentation and interaction; xterm.js owns terminal rendering.

The UI communicates with the native side through narrow commands and event payloads. Terminal output is emitted in bounded chunks rather than one event per byte. Secrets are never placed in the UI state model or diagnostics.

## Why

- Rust gives one testable home for lifecycle invariants, cancellation, and I/O ownership.
- Tauri keeps the desktop shell small while leaving a browser-compatible UI for visual development.
- xterm.js is a mature renderer for ANSI, alternate screens, and terminal input semantics.

## Risks and probes

1. High-output terminal streams must be measured before adding remote protocols.
2. PTY and signal behavior must be verified on each target OS.
3. Embedding FreeRDP/VNC may need a native child-surface strategy instead of web content.
4. IPC payload contracts must remain versioned and bounded.
