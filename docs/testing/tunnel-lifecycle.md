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

The subsequent [live operation history correction](live-operation-history.md)
keeps live tunnel Stop controls visible beyond the old 20-row total cap.
It is frontend retention evidence, with native large-list acceptance pending.

## Active TCP and pending-handshake cleanup — 2026-10-03

The existing encrypted ownership regression now exercises seven cases: idle
local/SOCKS jobs, pending remote approval, local/SOCKS traffic, a SOCKS client
stopped before its greeting and a local channel-open with a withheld peer
reply. It uses generated memory-only authentication/host keys and the
production tunnel dispatcher, runners and session drain.

For each traffic case, an owned TCP echo endpoint binds only `127.0.0.1`.
The SSH fixture forwards only to that preselected address/port, rejects other
destinations without resolving them and bounds its own forwarding workers.
A 32 KiB binary payload is delivered and echoed byte-for-byte through the
encrypted channel. With the client and target connections still open,
cancellation/session drain closes the client socket and the upstream TCP
socket before SSH transport teardown. The target task is joined, and target,
tunnel and SSH listeners are rebound after their owners finish.

The pending SOCKS case checks closure or the bounded protocol failure frame
followed by EOF. The pending local-open case checks client EOF before releasing
the peer's withheld response. That child cannot reach its copy-loop cancellation
select while waiting for channel approval, so the runner must actually abort
and join it. Deliberately replacing the local runner's child shutdown with
detachment fails at this named client-close deadline; the production bytes
were restored. This is a mutation check, not a reproduced defect in current
production code.

```sh
cargo test --locked -p mobarust tunnel_workers_are_owned_and_joined_before_transport_cleanup
cargo xtask check
```

The full local `cargo xtask check` passed on macOS ARM64 / Apple M2 in 232.49
seconds, including the expanded seven-case regression, 113 desktop tests,
workspace tests/Clippy, the complete frontend checks (including live-history
retention), protocol/helpers, release/lab tooling, package contracts and fuzz
compilation. The optional real Xvfb case retained its prerequisite skip.

This strengthens backend cleanup evidence for open connections and unresolved
protocol work. The traffic cases complete one binary roundtrip before Stop;
they do not prove cancellation amid saturated writes, 30-minute stability or
native Close/Quit under sustained traffic. The remote case still rejects
approval and opens no real remote listener. Real remote listener revocation,
broader OpenSSH/server compatibility and Windows/Linux/native acceptance remain
open. No production application logic or published installer changed in this
test milestone; the earlier source fixes remain absent from v0.1.29 installers.

## Explicit remote ports remain cancellable — 2026-10-03

On main after v0.1.29, the SSH adapter preserves an explicitly requested port
when the server's successful forwarding response has no port payload. The
underlying client reports that empty response as zero; previously, the adapter
returned zero to the desktop runner. Its displayed endpoint then used zero,
and the cancellation adapter rejected that value instead of asking the server
to release the actual listener. Automatically allocated ports use the port
returned by the server. Requests above 65535 are rejected before dispatch, and
a successful allocation cannot produce a zero-port response to the caller.

The new encrypted loopback regression reproduced the original explicit-port
failure before the fix. It checks both explicit and automatically allocated
ports: the server owns the listener, Cancel releases it before SSH disconnect,
and the same transport can start and cancel another forward. Invalid-port
requests never reach the server. Generated keys/passwords stay in memory;
the fixture accepts only `127.0.0.1` and its preselected port, does no DNS, and
joins its server task before rebinding the owned SSH endpoint.

The existing disposable OpenSSH workflow also requests an explicit loopback
port, checks the returned endpoint and listener ownership, cancels it, and
rebinds it before disconnecting. Its targeted check passed locally on macOS
ARM64; the portable regression passed as well.

The complete local `cargo xtask check` passed in 234.37 seconds on macOS ARM64
/ Apple M2: 113 desktop tests, the new forwarding regression, workspace tests
and Clippy, frontend unit/type/lint/build checks, protocol/helpers, release/lab
tooling, package contracts and fuzz compilation. The optional real Xvfb case
retained its prerequisite skip. An earlier full run failed with `AddrInUse`
at the active-TCP target rebind. That fixture now retains its target listener
until its connection task has completed, reducing its port handoff window;
the targeted cleanup check and the corrected parallel full run passed.

```sh
cargo test --locked -p mobarust-ssh --test forwarding
cargo test --locked -p mobarust-ssh --test local_sshd connects_to_a_reproducible_local_sshd_fixture_with_a_real_pty_shell -- --exact --nocapture
cargo xtask check
```

This is SDK and localhost OpenSSH listener-revocation evidence. It does not
prove native Stop/Close/Quit acceptance, cancellation during unresolved remote
approval, timeout/revocation-failure recovery or broad server/platform support.
The attempted native live-history check could not access a window for either
the running isolated app or Finder; no SSH fixture was started. That app and
its owned shell were terminated for lab cleanup, which is not normal UI Quit
acceptance. Native large-list checks remain pending. Published v0.1.29
installers do not include this correction; GitHub workflows remain disabled.

## Uncertain remote forwarding retires its transport — 2026-10-03

Cancelling a pending global forwarding request previously dropped only its
reply future. The server could still approve and allocate a listener, while
the desktop removed the local control. Likewise, an unconfirmed revocation
left the SSH connection usable despite uncertainty about its server listener.

On main after v0.1.29, both SDK forwarding calls use a pending-request guard.
Dropping a polled future, exceeding the unchanged 12-second deadline or
receiving an unconfirmed result retires that SSH transport. Malformed port
allocations also retain the guard. Confirmed approval disarms it; explicit
approval refusal disarms it because the server rejected the listener. Only
confirmed revocation disarms the cancellation guard. Invalid caller arguments
are rejected before it is armed. The typed `RemoteForwardUncertain` error
explains the connection closure and instructs the operator to reconnect.

The existing repository-local SSH client wraps its owned packet task in the
already available futures `Abortable`. Its handle can signal retirement
without waiting to enqueue DISCONNECT behind normal protocol messages.
This drops the owned transport when the task is polled; the SDK also retires
its local lifecycle state. It does not introduce a detached cleanup task,
extend either deadline or silently retry a forwarding request.

This fallback affects all work on that SSH connection. It cannot force a
noncompliant server to release listeners that the server deliberately keeps
outside the connection lifecycle. Server-side teardown remains required;
native recovery, routed transports and broader server interoperability need
their own acceptance evidence.

The encrypted, memory-only loopback regression reproduced the old dropped
approval remaining live at a named two-second transport-close deadline.
It now exercises dropped explicit/allocated approval, dropped revocation,
rejected revocation and both real production 12-second deadlines. The peer
owns actual `127.0.0.1` listeners, withholds replies deterministically, then
allows its packet loop to observe the retired transport. Each case joins the
server and rebinds its SSH and forwarding endpoints. A separate case proves
explicit refusal and invalid caller ports preserve the healthy connection;
the existing repeated explicit/allocated Start/Stop checks still pass.

The desktop session-owner regression also checks that cancellation during
pending remote approval retires the SDK connection before transport cleanup.
That peer rejects approval and creates no listener; it complements the SDK
late-allocation cases. Native UI acceptance remains pending. Published
v0.1.29 installers do not include this later correction; workflows stay disabled.

```sh
cargo test --locked -p mobarust-ssh --test forwarding
cargo test --locked -p mobarust tunnel_workers_are_owned_and_joined_before_transport_cleanup
cargo xtask check
```

The full local `cargo xtask check` passed on macOS ARM64 / Apple M2 in 272.52
seconds, including 113 desktop tests, all four forwarding regressions, workspace
tests/Clippy, frontend unit/type/lint/build checks, protocol/helpers, release/lab
tooling, package contracts and fuzz compilation. The optional real Xvfb case
retained its explicit prerequisite skip. An earlier full run timed out waiting
for the explicit X11 fixture's channel; the same source and deadline passed
standalone and in the full parallel rerun. No X11 code or timeout was changed,
and the cause of that earlier timing failure is not established.


## Live payload counters after v0.1.30 — 2026-10-03

The [v0.1.30 native acceptance](native-tunnels-v0.1.30.md) exposed 0 B rows
while open clients had completed byte-matched echoes. The three desktop
runners added `copy_bidirectional` totals only after successful completion;
long-lived copies emitted no progress, and cancellation/errors discarded
partial totals.

On main after v0.1.30, a shared stream wrapper counts only successful payload
writes in both directions. SOCKS greeting/reply bytes are excluded. Counters
saturate rather than wrap and survive copy errors, cancellation and aborted
children. The existing bidirectional copy implementation, session ownership,
16-client limit and joined cleanup remain in use. Each runner samples changed
counts at 250 ms intervals, skipping missed ticks; idle samples emit nothing.
Connection/completion/state events also carry current totals. Final Stopped or
Failed events are emitted after child shutdown, retaining the final snapshot.
Accepted stream writes are not a remote application acknowledgement.

The existing encrypted, memory-only ownership fixture now covers 11 cases.
Local, SOCKS5 and remote traffic each carry an exact 32 KiB echo and report
65,536 payload bytes while both clients remain open. Cancellation retains that
count. Additional successful-completion cases check that totals are not added
twice. The remote traffic peer owns one OS-assigned `127.0.0.1` listener and
joins its listener/channel worker on Cancel; this complements the original
pending-approval rejection case. Client/target EOF, control cleanup and listener
rebinding remain checked before transport teardown. Generated credentials and
pinned host keys stay in memory; caller-selected destinations are refused.

The executed regression failed before the fix at the live local-traffic
counter assertion, after owned fixture cleanup. Both targeted tests pass after
the fix, including a bounded in-memory copy-error case that retains accepted
partial writes and excludes unwritten bytes. A first test-edit compile failed
on a duplicated fixture field, which was corrected before that executed
regression.

The complete local `cargo xtask check` passed on macOS ARM64 / Apple M2 in
294.11 seconds, including 114 desktop tests, the four SDK forwarding cases,
25 OpenSSH cases, workspace tests/Clippy, frontend unit/type/lint/build checks,
protocol/helpers, release/lab tools, package contracts and fuzz compilation.
The optional real Xvfb case retained its prerequisite skip.

```sh
cargo test --locked -p mobarust ssh::backpressure_tests::tunnel_ -- --nocapture
cargo xtask check
```

This is source/backend evidence. The published v0.1.30 Mac packages retain
the counter defect; native acceptance of this new candidate and its next
installer remain pending. These short checks do not prove sustained throughput,
cancellation amid saturated writes, broad server or Windows/Linux acceptance.
GitHub workflows remain disabled; the 57/76 roadmap checklist is unchanged.
