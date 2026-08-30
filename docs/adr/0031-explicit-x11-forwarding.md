# ADR 0031: opt-in SSH X11 forwarding through an explicit external display

## Status

Accepted for the native SSH transport. An integrated X server is not part of
this decision and remains a separate cross-platform packaging project.

## Context

SSH X11 forwarding is not an X server. MobaRust needs a local display endpoint
and a policy for remote X11 channels, while Windows and macOS generally depend
on an external X server. Automatically reading `DISPLAY`, locating
`.Xauthority`, changing firewall rules, or starting an unreviewed server would
make a normal SSH connection perform surprising local operations.

## Decision

- X11 is disabled unless the connection request contains an explicit display
  target.
- Accepted targets are loopback `tcp://127.0.0.1:<port>`/`tcp://[::1]:<port>`
  (or the equivalent socket address) and `unix://<absolute-socket-path>`.
- `:0`, `$DISPLAY`, host-name discovery, environment lookup, and relative Unix
  paths are rejected.
- Rust sends the SSH `x11-req` on the PTY channel and creates the temporary
  `MIT-MAGIC-COOKIE-1` request value natively. The cookie is not serializable,
  logged, or exposed to React.
- Server-initiated X11 channels are accepted only for an enabled connection.
  Rust bridges each channel directly to the configured TCP or Unix display.
- The bridge has a five-second display-connect timeout, cooperative task
  cancellation, and a maximum of eight simultaneous display channels.
- A disabled connection explicitly rejects incoming X11 channels instead of
  accepting and silently dropping them.

## Consequences

The feature provides real SSH channel forwarding to an already-running
external display. It does not claim to bundle or manage an X server, discover
platform display configuration, or provide MobaXterm-equivalent integrated
X11 on every platform. The UI therefore labels the setting as opt-in and
external-server-only.

The native bridge keeps display bytes and authentication cookies outside the
frontend. A future integrated server must use a separately reviewed helper
process, explicit lifecycle/cancellation controls, dependency and license
review, and platform interoperability evidence before it can change this
boundary.

## Evidence

`crates/mobarust-ssh` has unit tests for strict display parsing and loopback
socket bridging. The Unix integration fixture starts an isolated OpenSSH
server with generated keys, enables X11 forwarding with a fixture-local
`xauth` executable, launches a synthetic Python X11 client through the SSH
shell, and verifies that the payload reaches a loopback display fixture.

The remaining cross-platform X-server and packaging matrix is intentionally a
release gate; this ADR does not mark that work complete.
