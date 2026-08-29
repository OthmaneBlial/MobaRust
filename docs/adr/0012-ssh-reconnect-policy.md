# ADR 0012: Reconnect SSH shells with bounded native retries

## Status

Accepted and implemented for interactive SSH shell loss.

## Decision

When the interactive SSH channel ends unexpectedly, the native session worker
keeps the terminal identity and renderer scrollback, emits `reconnecting`, and
tries to establish a fresh transport and PTY at most three times. Delays are
one, two, and four seconds. Each attempt rebuilds the transport from the
secret-free request and retrieves any credential material only inside the Rust
vault boundary. Jump-host chains are rebuilt from the same typed hop
descriptors.

On success the worker replaces its shell reader/writer and emits `connected`.
On exhaustion it emits `failed`, cleans up transfers/tunnels owned by the
session, and emits the existing terminal close event with an actionable
reason. A normal shell exit is not retried when the channel reports an exit
status. There is no infinite reconnect loop.

## Safety and limits

The retry policy stores hostnames, ports, auth references, and host-trust
configuration only; it does not cache plaintext passwords or private-key
material. Timeouts remain per operation. The current close command is bounded
by the active reconnect attempt and the configured transport timeout; a future
iteration can add an independent cancellation watch if manual interruption
during handshake becomes a product requirement.

## Verification

The lifecycle model tests the lost-connection/reconnect transition. The local
`sshd` fixture verifies that a fresh PTY can be established through the same
native transport and jump-channel implementation. Desktop TypeScript and
native lint/build checks cover the state-event integration.
