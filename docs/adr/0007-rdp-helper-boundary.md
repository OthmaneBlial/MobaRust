# ADR 0007: Isolate RDP behind a controlled helper process

## Status

Accepted as the RDP integration direction; the Rust-side helper contract is
implemented, while the FreeRDP engine integration is not started.

## Decision

MobaRust will study and package a pinned FreeRDP-based helper rather than
reimplement RDP or bind the complete C ABI directly into the UI process. The
first real experiment uses a controlled subprocess and a framebuffer bridge.
The native Rust parent owns lifecycle, typed configuration validation,
credential handoff, cancellation, diagnostics, and the helper's process
boundary. The helper owns the protocol engine and platform-specific FreeRDP
details.

The first IPC contract will be versioned and minimal: start, stop, resize,
keyboard, pointer, clipboard, connection state, framebuffer regions, and
diagnostics. Credentials are sent through a protected native channel and never
appear in process arguments, environment variables, or logs.

## Rationale

FreeRDP is mature and broad, but its CMake/ABI/plugin/dependency surface is too
large to treat as a casual Rust dependency. A helper contains crashes,
permits controlled forced termination, and keeps the browser frontend away
from protocol and credential details. A framebuffer bridge has a clearer
cross-platform contract than native window embedding for the first experiment.

## Rejected for the first experiment

- implementing RDP packet, codec, certificate, clipboard, and gateway logic
  from scratch;
- copying Remmina's GPL code or assets into MobaRust;
- accepting an arbitrary third-party RDP executable without version and
  capability checks;
- claiming RDP support from a screenshot, mock, or static framebuffer.

Native window embedding and direct FFI remain measurable follow-up experiments
if the helper bridge cannot meet input latency or display performance targets.
