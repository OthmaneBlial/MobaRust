# ADR 0016: Keep serial access native and device-explicit

## Status

Accepted and implemented for the native transport/configuration primitive.
Device enumeration, desktop manager, and terminal UI wiring remain pending.

## Decision

Serial support lives in a dedicated `mobarust-serial` crate. Rust owns serial
driver configuration, open/close, blocking reads and writes, line-ending
framing, lifecycle transitions, timeouts, cancellation, and reconnect attempts.
The frontend will eventually send a typed configuration; it will not receive a
raw device handle or arbitrary filesystem capability.

The configuration explicitly models:

- device path;
- baud rate;
- data bits;
- stop bits;
- parity;
- software or hardware flow control;
- line ending;
- driver I/O and open timeouts.

The `serialport` dependency is used as the cross-platform driver abstraction
with default system-enumeration features disabled. A device is opened only
after an explicit native connection request. Tests never enumerate or open
real devices, `/dev` paths, USB adapters, or the user's hardware.

## Lifecycle and device loss

Blocking driver calls run on Tokio's blocking pool and are bounded by the
driver timeout plus an operation guard. A broken pipe, not-connected, missing
device, or unexpected EOF is surfaced as a distinct recoverable device-loss
error and moves a connected session into `Reconnecting`. Explicit close and
cancel drop the native port handle and use the shared lifecycle state machine.
Reconnect is explicit and bounded; there is no aggressive background probing
of hardware.

## Security and safety

Serial traffic is not assumed to be encrypted or authenticated. The UI must
display the selected device and connection parameters clearly. Device paths
and configuration values are validated before any open attempt; control
characters and zero timeouts are rejected. Logs must contain metadata only,
never terminal payloads by default.

## Verification

Unit tests cover all serial parameters, line-ending framing, invalid paths and
baud rates, device-loss classification, and cancellation lifecycle behavior.
The current tests intentionally use a synthetic path only for validation, so
they prove no hardware interoperability claim. Controlled pseudo-terminal or
loopback fixtures should be added later per platform without touching personal
devices.

## Follow-ups

- add an explicit, read-only device refresh command;
- add a manager with terminal output batching and clean cancellation;
- add platform fixtures for a disposable pseudo-terminal/loopback device;
- verify USB adapter disappearance on Windows, Linux, and macOS in controlled
  environments;
- add reconnect policy and line-ending/encoding controls to saved profiles.
