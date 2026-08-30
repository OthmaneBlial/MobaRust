# ADR 0016: Keep serial access native and device-explicit

## Status

Accepted and implemented for the native transport/configuration primitive,
explicit Quick Connect/session-manager path, read-only device refresh, and
secret-free saved serial profiles.

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
after an explicit native connection request. Device enumeration is a separate,
read-only command invoked only by the visible Refresh action. Tests never
enumerate or open real devices, `/dev` paths, USB adapters, or the user's
hardware. Saved serial profiles contain only the device path and line
parameters; they contain no secret material.

## Lifecycle and device loss

Blocking driver calls run on Tokio's blocking pool and are bounded by the
driver timeout plus an operation guard. A broken pipe, not-connected, missing
device, or unexpected EOF is surfaced as a distinct recoverable device-loss
error and moves a connected session into `Reconnecting`. Explicit close and
cancel drop the native port handle and use the shared lifecycle state machine.
Reconnect is explicit and bounded: the visible `serial_reconnect` command
performs one reopen attempt, preserves the terminal identity, and leaves a
failed attempt retryable without background probing of hardware. The Tauri
manager owns one typed session command channel per
connection, batches output, retains a small bounded pre-attach buffer, and
emits `serial://state`, `serial://output`, and `serial://closed` events. The
React shell only sends typed connection parameters and terminal input; the
refresh result contains device path and coarse port type metadata only.

## Security and safety

Serial traffic is not assumed to be encrypted or authenticated. The UI must
display the selected device and connection parameters clearly. Device paths
and configuration values are validated before any open attempt; control
characters and zero timeouts are rejected. Logs must contain metadata only,
never terminal payloads by default.

Open and I/O failures are categorized before they cross the native boundary:
missing device, permission denied, device loss, timeout, or generic driver
failure. Raw device paths and driver descriptions are not included in the
user-facing error text.

## Verification

Unit tests cover all serial parameters, line-ending framing, invalid paths and
baud rates, device-loss classification, and cancellation lifecycle behavior.
The Unix integration fixture now creates a disposable pseudo-terminal inside
the test process and verifies real serialport reads, writes, and device-loss
classification. It never enumerates the host's ports or opens a physical
adapter. This proves only Unix pseudo-terminal behavior; it is not hardware
interoperability evidence.

## Follow-ups

- verify USB adapter disappearance on Windows, Linux, and macOS in controlled
  environments;
- add reconnect policy and line-ending/encoding controls to saved profiles.
- complete a hardware interoperability matrix on dedicated test hardware.
