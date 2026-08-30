# ADR 0024: Keep ping and traceroute bounded at the native process boundary

## Status

Accepted and implemented for the desktop diagnostics view. Fingerprint
inspection remains a separate, unscheduled capability.

## Decision

MobaRust invokes the operating system's native `ping` or traceroute utility
from Rust with an explicit argument vector. It never constructs a shell
command string. Each operation requires an explicit target and timeout; ping
sends one echo request, while traceroute accepts at most 32 hops. The native
worker exposes a typed lifecycle event and a cancellation command.

The child process receives no stdin, has stderr discarded, and is configured
to terminate when its Rust child handle is dropped. Native stdout is read
through a 64 KiB cap before it is accumulated in memory; an overflow kills
and reaps the child with a typed error. The traceroute output is then treated
as untrusted diagnostic text and truncated to a bounded number and length of
lines before it crosses the Tauri event boundary. React renders it as escaped
preformatted text rather than HTML.

## Safety boundary

- no default or background target;
- no shell interpolation or arbitrary executable path from the frontend;
- timeout and cancellation apply to the native child process;
- command stdout is bounded before buffering, then traceroute output is
  bounded again before IPC and display;
- the UI labels results as diagnostics, not a security audit;
- tests use only `127.0.0.1` and do not inspect or use personal SSH material,
  external hosts, or serial hardware.

Native utility availability and platform permissions can differ across
Windows, Linux, and macOS. A missing or failing utility is reported as an
actionable diagnostic error; it is not replaced by a fake result.

## Verification

The network crate tests one explicit loopback ping, pre-cancelled process
startup, bounded traceroute argument/output behavior, existing bounded TCP
scans, and local loopback listeners. No test sends traffic to an Internet
host or scans the local network.
