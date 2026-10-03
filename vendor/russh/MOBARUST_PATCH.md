# Repository-local russh patch

Baseline: [russh 0.63.3](https://github.com/Eugeny/russh/releases/tag/v0.63.3),
crates.io archive SHA-256
`036204edbd199552a5b3832f63c60dcdf395dc44c7f06b4af1c0e8139cc11bce`.
Packaged source commit: `f33baf439be8c59c49cb6cb2ae5976c579495bdf`.
Apache-2.0; original source headers and README remain. The archive omits its
license file, so this copy includes the canonical Apache-2.0 text.

The keyboard-interactive handler previously awaited an application response
inside the packet reader. A user leaving the dialogue unanswered therefore
prevented peer disconnects from being processed. The local change returns to
the packet loop immediately and handles answers through `Session::handle_msg`.
One pending flag refuses overlapping challenges, new authentication during a
challenge and unsolicited answers; terminal authentication results clear it.
`client_send_auth_response` is visible to its parent module for that route.

Regression: `cargo test --locked -p mobarust-ssh --test authentication`.
It includes a real loopback peer disconnect while a responder never completes,
requiring cancellation within one second and no response packet. Multi-round
and distinct password/OTP cases exercise normal answer delivery. No upstream
fix or upstream test-suite result is claimed. Remove this local copy when an
upstream release fixes the same behavior and these regressions pass unchanged.

The client packet loop also selects pending writes alongside reads. Awaiting
the entire flush previously stalled concurrent large PTY/SFTP streams over
two nested jump channels. Further application output stays in bounded queues
while a flush is pending; unsent replies exceeding twice the configured receive
window are refused. `src/sshbuffer.rs` retains a partial-write cursor across
select turns, so resuming a flush cannot duplicate its ciphertext prefix.
Sent-prefix compaction occurs after at least half the buffer has been written.
Final disconnect retains a final flush. Regression and limits:
[8 MiB concurrent IPv6/OpenSSH streams](../../docs/testing/routed-ipv6-streams.md),
run with `cargo xtask test-ssh` and `cargo xtask check`. Remove these changes only
when the concurrent stream and existing authentication regressions pass with
an upstream replacement.

The workspace excludes this dependency from its own test targets, and its
library test/doctest targets are disabled in the local manifest. Upstream
examples, benchmarks, external tests, development dependencies, library/client
test modules, key format fixtures, inline key fixtures and the key-bearing documentation
example are omitted; production code is otherwise unchanged except for the
client/packet-writer changes above. MobaRust generates test credentials in memory. Remaining
inline upstream tests are not executed as workspace tests.

`src/keys/format/mod.rs` retains upstream PEM delimiter literals. Its exact
unmodified SHA-256 is
`7ca4a796a523df1063f0ea0bac1b6d869288dd100d041a6455f8bd7d73b15678`.
The pre-push audit permits markers only in those exact indexed bytes; edited
parser bytes or markers in any other indexed file are refused. No private-key
fixture is included. The root dependency retains `default-features = false`
and enables only `aws-lc-rs` and `flate2`; RSA remains disabled.
