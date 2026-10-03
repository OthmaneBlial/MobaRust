# Concurrent streams through an IPv6 jump chain — 2026-10-03

## Reproduction and correction

The OpenSSH lab connected to two distinct bastions and a target using `::1`
at every endpoint and generated `[::1]:port` known_hosts records. Rejecting the
key at each endpoint failed closed. An 8 MiB SFTP upload timed out when run
concurrently with 8 MiB of PTY output through that chain. The same workloads
passed when the PTY output was drained before starting the file copy.

The repository-local `russh` client awaited an entire encrypted packet flush
before returning to its packet reader. A nested channel transport can block
that flush on SSH flow control while inbound traffic needs draining. Increasing
buffers or the file-operation deadline would leave that dependency intact.

The client now selects pending packet writes alongside reads. It leaves further
application output in the existing bounded queues while a write is pending,
and refuses unsent replies exceeding twice its configured receive-window size.
Final disconnect packets still use the final flush path.

The packet writer retains its transmitted-byte cursor across select turns.
Each successful partial write advances it before another await; restarting a
flush resumes the remaining ciphertext instead of sending the prefix again.
Sent prefixes are compacted after at least half the buffer has been transmitted.
This uses Tokio's [cancellation-safe `write`](https://docs.rs/tokio/latest/tokio/io/trait.AsyncWriteExt.html#method.write),
with progress stored outside the future; `write_all` alone does not retain that
progress when its future is dropped.

Frequent server rekeying also exposed a separate output-loss bug. OpenSSH sent
exit status zero after 6,488,290 bytes, then delivered the remaining output;
the complete stream contained 8,389,269 bytes including shell setup and markers.
The desktop consumers stop on exit status, so the shared shell reader now retains
that status until output EOF or channel close. Its state survives cancellation
of `next_output`. EOF before status still preserves the later status, and a
closed channel without status remains a closure rather than an invented success.
The [SSH connection protocol](https://www.rfc-editor.org/rfc/rfc4254.html#section-6.10)
defines process exit status separately from channel closure.

## Runnable evidence

```sh
cargo xtask test-ssh
cargo xtask check
```

`ipv6_jump_chain_streams_large_shell_output_and_sftp_concurrently` and
`ipv6_jump_chain_streams_large_shell_output_and_sftp_during_frequent_rekey`
both verify:

- Three separately generated OpenSSH daemons, generated keys and explicit IPv6
  trust. A wrong pin at each hop rejects the attempt before the successful chain.
  The additional rekey case sets
  [RekeyLimit 256K](https://man.openbsd.org/sshd_config#RekeyLimit) on each daemon,
  exercising key renewal during streams much larger than that threshold.
- A real PTY shell emitting exactly 8 MiB of UTF-8, emoji, ANSI colour sequences
  and newlines. The test disables PTY newline conversion and compares bytes
  between output markers, then requires exit status zero.
- Concurrent SFTP upload followed by download on the same target connection,
  using a space-containing Unicode filename. Both 8 MiB copies match the source
  byte-for-byte, including the target's actual on-disk file.
- A bounded output collection and joint deadline, followed by SFTP
  close and disconnect of the complete chain. All generated trust bytes remain
  unchanged. Owned daemons are killed/reaped, their directories are removed,
  and both IPv4 and IPv6 listener addresses are rebound after teardown. The
  ordinary case retains its 30-second deadline; the added frequent-rekey case
  uses 60 seconds for repeated cryptographic exchanges across all three hops.
  Production SFTP operation deadlines remain 12 seconds.

`shell_exit_status_waits_for_the_end_of_output` uses the existing memory-only
SSH wire fixture. It sends status before both stdout and stderr, then EOF/close;
the consumer stops on the delivered status and must still receive every byte.
A second case sends EOF before status. Both require the exact exit code and no
later output event. Before the correction the first case received only its
setup banner; the focused regression passed after the shared-reader fix.

A diagnostic 90-second observation budget confirmed continuing download
progress rather than a stalled operation: the rekey workload completed in
20.90 seconds. Enabling TCP_NODELAY on the native client socket produced a
single 20.65-second run, insufficient evidence of a meaningful improvement;
that speculative setting was removed. Under the ordinary parallel test harness,
the rekey case showed continuing download progress through 6 MiB at 29.55 seconds
before its initial 30-second observation deadline expired. Limiting Tokio to
two workers also failed that deadline. The final suite preserves the original
ordinary workload and adds rekeying as a distinct case with a 60-second budget.
These debug observations are not a throughput benchmark or a general latency
claim.

When loopback IPv6 is unavailable the case prints an explicit skip. The daemon
also binds `127.0.0.1` for the existing startup probe; all actual chain endpoints
use `::1`. No wildcard, LAN or public listener, system SSH service, personal
credential, SSH agent or remote server is involved. The existing OS account
authenticates with generated keys and a disposable shell HOME/ZDOTDIR.

The local SSH suite passed on macOS 26.6 ARM64 (Apple M2), Rust 1.95.0 and
OpenSSH 10.3p1 / LibreSSL 3.3.6: 42 unit tests, 14 automated authentication cases
and 16 OpenSSH cases. IPv6 executed; three manual labs
remained ignored and the optional real Xvfb case reported its prerequisite skip.
The [16 MiB interrupted SFTP/SCP checks](interrupted-transfers.md) also passed.
The ordinary streams completed at 2.85 seconds and the frequent-rekey streams
at 32.11 seconds in the final parallel workspace run; teardown also passed.
The existing X11 case failed its five-second channel deadline in the first
workspace run, then passed unchanged in isolation, the parallel diagnostic and
the final workspace run. Its deadline and assertions were retained; the cause
of that isolated timing failure was not established.

The full local `cargo xtask check` then passed: workspace tests and Clippy,
frontend tests/type checking/lint/build, release/lab tooling checks, isolated
RDP/VNC fixtures (all 17 VNC process cases), synthetic package-layout contracts
and fuzz compilation. The local pre-push audit also passed. These checks do
not establish native GUI or installer acceptance for the new runtime changes.

## Limits

This is protocol and real-PTY evidence on macOS ARM64, not a native WebView
rendering, throughput, input-latency or long-run stability benchmark. It does not
establish Windows/Linux runtime behavior, arbitrary server interoperability,
routed non-loopback networks, or sustained-output GUI responsiveness.
The shared client correction applies to both address families; this particular
large concurrent workload exercises IPv6. No upstream fix or upstream test-suite
result is claimed.

The source changes are after v0.1.21; published installers are unchanged.
GitHub Actions remain disabled.
