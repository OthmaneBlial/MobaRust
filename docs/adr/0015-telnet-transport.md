# ADR 0015: Keep Telnet as an explicit plaintext native adapter

## Status

Accepted and implemented for the native single-connection transport. Desktop
session-manager and saved-profile wiring remain pending.

## Decision

Telnet is implemented in a dedicated `mobarust-telnet` crate instead of being
treated as an SSH variant. Rust owns TCP connection setup, incremental IAC
framing, the small supported option set, terminal encoding, lifecycle state,
timeouts, reconnect attempts, and cancellation. The future desktop manager
will expose only typed requests and terminal events to React.

The adapter supports UTF-8 and Windows-1252 text decoding, terminal type,
NAWS dimensions, suppress-go-ahead, and server echo negotiation. Unknown
options are refused explicitly. Subnegotiation frames are bounded to 4 KiB and
terminal reads use a bounded raw buffer.

Telnet is always presented as unencrypted. The adapter must not reuse SSH
security copy or imply host-key verification, credential confidentiality, or
transport integrity. If credentials are added later, they must remain behind
the native vault boundary and the UI must display the plaintext risk first.

## Cancellation and reconnect

Connect and reconnect attempts have operation-specific timeouts. Dropping an
in-flight future cancels the cooperative operation; an explicit `cancel`
shuts down the writer and enters the shared cancelled lifecycle state. An
explicit reconnect uses a bounded retry policy and exponential backoff; there
is no infinite background loop.

## Verification

Unit tests cover incremental framing, literal IAC bytes, supported-option
responses, terminal-type/NAWS payloads, invalid configuration, and bounded
subnegotiation. A local TCP fixture performs a real negotiation, round-trips
plaintext terminal bytes, and verifies explicit close/cancel lifecycle
behavior. It binds only to `127.0.0.1` on an operating-system-selected port.

## Rejected for this milestone

- invoking the system `telnet` command through a shell;
- advertising Telnet as encrypted or SSH-equivalent;
- accepting arbitrary option negotiation without bounds;
- testing against legacy equipment or internet hosts from the developer
  workstation;
- claiming the desktop protocol is complete before manager/UI integration.
