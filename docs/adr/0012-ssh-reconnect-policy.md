# ADR 0012: Reconnect SSH shells with bounded native retries

## Status

Accepted and implemented for interactive SSH shell loss.

## Decision

When the interactive SSH channel ends unexpectedly, the native session worker
keeps the terminal identity and renderer scrollback, emits `reconnecting`, and
tries to establish a fresh transport and PTY up to the configured attempt
limit (default three, maximum ten; zero disables reconnect). Delays begin at
one, two, and four seconds and cap at 32 seconds. Each attempt rebuilds the
transport from the secret-free request and retrieves any credential material
only inside the Rust vault boundary. Jump-host chains are rebuilt from the
same typed hop descriptors.

Closing a session uses a dedicated native cancellation watch rather than the
normal command queue. The worker observes it during the backoff delay, transport
connection, and PTY establishment, so a user close can cancel a stalled
reconnect without waiting for the full retry schedule. Dropping a cancelled
attempt releases the in-flight transport future; the final cleanup still emits
the normal disconnected state.

On success the worker replaces its shell reader/writer and emits `connected`.
Successful reconnects that each lose their shell within 30 seconds also
consume the configured budget. A shell that stays open for at least 30 seconds
resets this short-lived-shell count. This prevents an accepted transport
followed by an immediate shell close from starting unbounded retry cycles.
On exhaustion it emits `failed`, cleans up transfers/tunnels owned by the
session, and emits the existing terminal close event with an actionable
reason. A normal shell exit is not retried when the channel reports an exit
status. There is no infinite reconnect loop.

## Safety and limits

The retry policy stores hostnames, ports, auth references, and host-trust
configuration only; it does not cache plaintext passwords or private-key
material. The configured connect timeout (100–60,000 ms; default 12,000 ms)
applies to the target and each jump host, including shell and jump-channel
establishment. It is not an inactivity limit: an established SSH connection,
shell, file browser, or tunnel can remain quiet longer than the setup deadline.
Optional keepalives retain the native bounded missed-response policy.
Cancellation is cooperative first: it
drops the active delay or transport future, then lets the session worker perform
bounded protocol cleanup.

## Verification

The lifecycle model tests the lost-connection/reconnect transition. Native
worker policy tests inject three consecutive failures, a success after a
failure, and cancellation during an in-flight attempt without opening a socket.
The local `sshd` fixture verifies that a fresh PTY can be established through
the same native transport and jump-channel implementation. An idle-connection
regression completes SFTP traffic, waits beyond a two-second setup deadline
without keepalives, and then verifies actual shell execution. Desktop TypeScript
and native lint/build checks cover the state-event integration.
