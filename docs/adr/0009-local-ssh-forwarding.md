# ADR 0009: Use native SSH channels for forwarding

## Status

Accepted and implemented for local (`-L`), remote (`-R`), and bounded dynamic
SOCKS5 (`-D`) forwarding paths.

## Decision

The Rust SSH manager owns the local listener and opens one SSH
`direct-tcpip` channel for each accepted local client. The React renderer only
supplies a typed bind/target request and receives typed lifecycle events:

`Listening -> Running -> Stopping -> Stopped`

or:

`Listening/Running -> Failed`.

The listener may bind port `0` so the operating system selects a free port.
Each tunnel accepts at most 16 simultaneous local clients. A tunnel is tied to
the SSH terminal that created it; closing that terminal cooperatively cancels
all of its tunnels. Each worker forwards bytes with bounded async I/O and
shares the SSH connection without interrupting the interactive shell reader.

The frontend never receives an SSH channel or arbitrary socket capability. It
gets endpoint metadata, state, connection count, byte count, and sanitized
errors through `ssh://tunnel` events. Bind and target values are validated in
Rust, and a local bind failure is reported before the tunnel is registered.

Remote forwarding uses the SSH server's `tcpip-forward` request and routes
each server-initiated `forwarded-tcpip` channel through a native receiver.
Dynamic forwarding performs an unauthenticated SOCKS5 CONNECT handshake on a
bounded local listener, then opens a native `direct-tcpip` channel for the
requested destination. The SOCKS proxy is deliberately local-only and does
not expose a shell or arbitrary IPC operation.

## Rationale

- `direct-tcpip` is the SSH mechanism intended for client-side local
  forwarding; it avoids faking a tunnel with terminal commands or screenshots.
- A separate channel per local client preserves independent connection
  lifecycles and allows the interactive PTY to continue receiving output.
- The bounded client count protects the application from accidental fan-out.
- The native layer can cancel listeners and workers without exposing raw
  sockets to the webview.

## Rejected for this milestone

- exposing a generic `execute_anything` or socket-open IPC command;
- unbounded client tasks;
- silently binding a wildcard address;
- claiming jump-host support because local forwarding exists.

## Verification

The local SSH fixture opens real `direct-tcpip` and `forwarded-tcpip` channels
to local echo servers and verifies bidirectional payload delivery. SOCKS5
framing has deterministic in-memory tests. Manager-level lifecycle and
cancellation remain native integration coverage to extend with a desktop
fixture; manual interoperability checks are still required on Windows, Linux,
and macOS.

## Follow-ups

- saved tunnel definitions and session ownership UI;
- reconnect behavior for tunnels when an SSH session drops;
- platform matrix and security review for binding addresses.
