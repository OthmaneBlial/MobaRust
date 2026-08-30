# ADR 0017: Keep network diagnostics explicit and bounded

## Status

Accepted and implemented for native TCP checks, bounded port-range scans,
desktop DNS/TCP/port-scan command and UI wiring, and the bounded platform
`ping`/`traceroute` surface.

## Decision

Network diagnostics live in a Rust-native `mobarust-network` crate. A check
requires an explicit host, an explicit port, and an operation timeout. A scan
requires an explicit inclusive range, a maximum concurrency, and a timeout.
The range is limited to 4096 ports and concurrency to 128 tasks. Cancellation
is represented by a native watch receiver; cancellation aborts and joins
outstanding tasks before returning.

The surface covers TCP reachability, bounded port status (`open`, `closed`, or
`timed-out`), DNS resolution, and explicit platform-native ping/traceroute
commands. Ping and traceroute have their own platform and permission review;
fingerprint inspection remains a separate native capability.

Diagnostic failures exposed to the UI use stable categories rather than raw
OS resolver, process, or task text. This keeps local paths, command details,
and host-specific system messages out of the IPC error surface while retaining
actionable distinctions such as DNS failure, timeout, and process failure.

## Safety boundary

- no implicit local-network discovery;
- no default target or default range;
- no unbounded task fan-out;
- no shell invocation for TCP checks;
- no unbounded native diagnostic stdout buffering;
- no automatic recurring scan;
- no claim that a TCP result proves a service is safe or authenticated.

The frontend shows the exact target and range before starting a scan, exposes
progress and cancellation, and keeps the result framed as a legitimate
administration diagnostic rather than an offensive security tool.

## Verification

Unit tests reject empty targets, invalid ports, excessive ranges, and
excessive concurrency. A local `127.0.0.1` listener proves an explicit open
port result. A pre-cancelled receiver proves that cancellation returns before
any scan task is scheduled. The highest legal port is tested to ensure range
arithmetic cannot wrap around and rescan unexpectedly.

No test targets an external host, scans the local network, or invokes a system
utility. The desktop view preserves the explicit-target contract and does not
start background discovery or scans automatically.
