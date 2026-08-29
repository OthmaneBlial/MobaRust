# ADR 0026: Use a bounded fixed query for SSH remote monitoring

## Status

Accepted and implemented for the one-shot SSH monitor surface.

## Decision

MobaRust collects a remote system snapshot only through a fixed, native-owned
read-only query. The frontend sends a terminal identifier, never shell text.
The query emits a small set of tagged fields for hostname, kernel, uptime,
load, memory, root filesystem usage, and process count where the host exposes
the required standard capability.

The native SSH layer bounds combined stdout and stderr to 64 KiB and bounds the
whole operation to six seconds. A non-zero command status is reported as a
typed monitor error without forwarding arbitrary stderr to the renderer.
Missing `/proc`, `sysctl`, `df`, or `ps` data becomes an unavailable metric.
The UI requests one snapshot when opened and refreshes only on an explicit
button press; it does not poll continuously.

## Why

The monitor is useful for ordinary administration without requiring an agent,
but an unrestricted remote-exec IPC command would materially widen the attack
surface. A fixed query also makes command construction auditable and keeps
credentials, private keys, and arbitrary remote output outside the React state
model.

## Consequences

Linux hosts expose the most complete set of fields. BSD, macOS, appliances, and
minimal shells may expose only identity or a subset of metrics. This is an
intentional graceful-degradation behavior, not a claim of universal GNU/Linux
compatibility. Continuous dashboards and process-detail views remain future
work and must preserve the same bounded/cancellable boundary.

## Verification

Parser tests cover login-banner noise, optional fields, memory conversion,
malformed values, and the unsupported-host result. The SSH integration fixture
continues to use a temporary local server, generated fixture keys, and
loopback-only networking; no personal SSH configuration, key, agent, hardware,
or external host is accessed.
