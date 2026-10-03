# SSH tunnel worker ownership

## Owned and bounded workers after v0.1.29 — 2026-10-03

Local, SOCKS and remote-forward jobs previously ran as detached Tauri tasks.
They bypassed both the session's 32-worker admission limit and its operation
drain. Removing a cancellation control is only a stop request; it does not
prove that a listener or its connection workers have finished.

On main after v0.1.29, all three tunnel types are dispatched into the existing
session `JoinSet`. Its 32-worker limit is shared by finite file/monitor
operations, active/waiting transfers and tunnel jobs. Long-running tunnels
consume a worker slot; a cancelled job releases its slot when it has actually
finished and its completed entry is reaped. Counting only cancellation
controls would allow repeated Start/Stop to bypass this bound.

Excess queued tunnel starts emit one Failed event with zero connections and
forwarded bytes, the static session-busy error and control removal. Remote
starts also settle their response with that error. Local/SOCKS starts can have
already returned their provisional listening response, so their final refusal
arrives through the existing tunnel event channel. Their owned listeners drop
on refusal. Queue retirement retains its original Stopped/cancellation reason.

Close, shutdown and transport replacement signal cancellation and join these
jobs before session completion/transport cleanup. Each runner retains its
existing child-worker shutdown; remote-forward requests and listener
cancellation retain their existing deadlines. Accepted file/editor operations
still drain cooperatively rather than being aborted during promotion/rollback.
Terminal input and the independent resize channel remain available at worker
saturation. Per-tunnel connection limits (16), the one-remote-forward rule,
64 command slots and the three-transfer execution limit remain separate.

```sh
cargo test --locked -p mobarust saturated_session_refuses_tunnels_and_releases_loopback_listeners
cargo test --locked -p mobarust retired_queue_reports_cancelled_jobs_and_releases_loopback_listeners
cargo test --locked -p mobarust tunnel_workers_are_owned_and_joined_before_transport_cleanup
cargo xtask check
```

The admission regression reuses the existing retirement fixture. With 32
pending workers, it checks refusal of all three tunnel types, their final
events, remote response settlement, empty control maps and rebinding of both
discarded loopback listeners. It first failed on the original implementation
because a tunnel was admitted at saturation.

The encrypted loopback regression uses the same production tunnel dispatcher
and runners with generated memory-only host keys/passwords and a pinned host
fingerprint. It observes the session-owned worker, cancels/joins it, checks
empty controls and rebinds local/SOCKS listeners while the SSH transport is
still live. The remote case pauses and rejects server approval; it verifies
cancellation of an unresolved request, not a real remote listening socket.
The transport is then disconnected, its server task joined and its listener
rebound. The regression fails when all three runners are deliberately detached
from the session owner; that mutation was restored.

The full local `cargo xtask check` passed on macOS ARM64 / Apple M2 in 202.73
seconds, including 113 desktop tests, workspace tests/Clippy, frontend unit
tests/type checking/lint/build, protocol/helper cases, release/lab tooling,
package contracts and fuzz compilation. Its optional real Xvfb case retained
the prerequisite skip. An earlier run passed the Rust tests but stopped at a
test-only Clippy nested-if finding; the corrected complete run passed.

This is backend admission and idle/pending-job cleanup evidence. It does not
prove native Close/Quit during sustained tunnel traffic, actual remote listener
revocation, broad server interoperability, Windows/Linux acceptance, a
whole-process memory bound or a bound on concurrent listener-binding requests.
Those gates remain open. Published v0.1.29 installers do not include this
later source change; GitHub workflows remain disabled.
