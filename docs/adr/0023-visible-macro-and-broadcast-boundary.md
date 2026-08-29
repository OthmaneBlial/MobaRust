# ADR 0023: Keep macros visible and broadcast input opt-in

## Status

Accepted and implemented for the first bounded operator-automation slice.

## Decision

Macros are stored separately in the versioned, secret-free `macros.json`
catalog. A macro is a bounded list of typed actions: send text, wait, send a
key from an allowlist, execute a command by sending explicit terminal input,
open a saved session, or switch to an existing workspace. Loading and saving a
macro never executes it.

Every run requires a visible confirmation. The UI shows the current step and
explicit target labels while it runs. Cancellation is cooperative: each step
checks the cancellation flag and waits are sliced into 50 ms intervals so a
long delay cannot make the UI appear stuck. Macro text is never written to
logs, and the UI warns users not to put passwords, tokens, or private keys in
macro text.

Broadcast input is a separate explicit mode. The operator must select ready
terminal tabs, review the target list, and enable the mode. While enabled, a
high-signal warning banner remains above the terminal and every keystroke is
sent only to the selected native terminal identifiers. If any selected target
is not ready, the whole input event is rejected rather than partially fanned
out. `Esc` disables broadcast immediately.

The frontend receives opaque native terminal identifiers only for routing
typed input. It never receives or stores credential material. No arbitrary
shell command IPC was added; command actions reuse the same typed write
commands as normal terminal input.

## Consequences

This provides a useful, reviewable automation primitive without silently
turning saved data into an execution engine. The first slice does not record
keystrokes, bypass terminal confirmation policy, or support unrestricted
plugin code. Future policy work can add per-action approvals and stronger
browser stress coverage without changing the persistence boundary.

## Verification

Core tests validate action bounds, key typing, serialization, and rejection of
NUL/unbounded payloads. Store tests validate durable round trips, deletion, and
refusal to replace corrupt macro data. TypeScript, ESLint, and the production
Vite build validate the renderer and typed Tauri command wiring. Protocol and
hardware tests remain isolated from real hosts and devices.
