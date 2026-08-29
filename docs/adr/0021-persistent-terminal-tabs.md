# ADR 0021: Keep terminal tabs as persistent native session views

## Status

Accepted and implemented for terminal tabs; split panes remain a separate
follow-up.

## Decision

The workspace keeps a small descriptor for each open terminal tab: a UI
identifier, stable instance key, label, protocol, native terminal/session
identifier, and lifecycle status. Local PTYs and remote SSH, Telnet, and
serial sessions use the same `TerminalViewport` bridge. Switching tabs hides a
viewport without unmounting it, so its xterm buffer and native session remain
alive. Closing a tab removes the descriptor and unmounts the viewport, which
cooperatively closes the associated native session.

Remote file and tunnel actions resolve against the active tab’s native session
identifier. A final tab close creates a fresh local tab so the workspace never
ends in an unusable empty terminal surface.

## Security and reliability boundary

Tab descriptors contain no credential material. Native identifiers are used
only for typed commands and are not treated as secrets. Each tab filters
terminal events by its own native identifier, preventing output from one
session appearing in another tab. Hidden tabs do not create additional network
connections; they preserve the connection that the operator explicitly
opened.

## Consequences

This provides simultaneous local and remote sessions with predictable
per-session lifecycle handling. Split panes need a layout/state model and
careful resize/focus semantics, so they are not represented as implemented by
this ADR.

## Verification

The existing PTY and protocol lifecycle tests remain green, while TypeScript,
ESLint, and the production Vite build validate the multi-tab renderer wiring.
