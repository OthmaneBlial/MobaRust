# Interrupted SFTP/SCP streams — 2026-10-03

## Reproduced failure and correction

A disposable OpenSSH test cut an established loopback relay after an SFTP
upload reported 256 KiB. The copy did not terminate within the test's 20-second
deadline. An interrupted download instead reported `local file operation failed`.
The wider workspace run also reproduced a pending SCP upload after the same
controlled transport loss, again exceeding the 20-second test deadline.

The locked `russh-sftp` 2.4.0 file writer pipelines write acknowledgements. Its
pending acknowledgement receivers do not use the raw session's request timeout;
the stream can remain pending after transport loss. The cancellable copy also
mapped both remote and local stream errors to `LocalIo`.

Remote file reads, writes, flush and close now use a 12-second operation deadline.
An error or expired deadline on a closed SSH transport reports
`SftpConnectionLost`; an expired operation on an open transport reports `Timeout`.
Local source/destination failures remain redacted `LocalIo` errors. These are
operation deadlines, not a total duration limit for a large transfer. Normal
and cancellable SFTP entry points share the copy implementation; editor Save
and Save as reuse that upload path before their existing promotion checks.
The bounded editor read retains its existing per-request timeout.
SCP remote data reads/writes, acknowledgement/control-byte reads and final EOF
now also have 12-second operation deadlines, with cancellation still selectable.
A stalled SCP operation reports the existing typed timeout rather than holding
its caller indefinitely; ordinary SCP channel/protocol errors remain redacted.

## Runnable evidence

```sh
cargo xtask test-ssh
cargo xtask check
```

`interrupted_large_sftp_and_scp_transfers_fail_then_retry_from_the_start`
exercises all four combinations of SFTP/SCP and upload/download against the
existing disposable OpenSSH daemon:

- Each source contains 16 MiB of deterministic binary bytes and has a Unicode,
  space-containing filename.
- The transfer reaches a 256 KiB progress barrier. Uploads additionally wait
  for that many bytes to exist in the server's part file: queued progress alone
  does not prove delivery.
- The test stops polling the copy, cuts and joins its own TCP relay, then resumes
  the copy. Cancellation remains unsignalled. Every copy must fail within a
  bounded deadline, rather than complete or hang.
- Original destination bytes remain unchanged; the actual nonempty part is
  shorter than the source and exactly matches its prefix. Local parts are removed.
  Upload cleanup over the lost connection must explicitly report
  `RemoteTemporaryCleanupFailed` and leave the part present.
- A new authenticated connection with unchanged generated trust removes the
  remote part, retries from byte zero and verifies all 16 MiB byte-for-byte.
  Uploads then exercise successful overwrite promotion without a leftover part
  or backup. Downloads verify the copied part and remove it; this protocol test
  does not invoke the desktop's local commit helper.
- Each relay worker is joined and its released listener address is rebound.
  The fixture destructor kills/reaps only its daemon and removes generated data.

`sftp_local_stream_failures_remain_local_and_redacted` also uses real SFTP with
deliberately unreadable/unwritable local file handles. Both normal entry points
must report `LocalIo`, without exposing paths, while preserving source bytes.

The local SSH suite passed on macOS ARM64 with 42 unit tests, 13 automated
authentication cases and 14 OpenSSH cases. Three manual authentication labs
remained ignored; the optional real Xvfb case reported its prerequisite skip.
All daemon and relay listeners use `127.0.0.1`; credentials and trust are generated,
HOME/XDG are disposable, and no personal agent or system SSH service is used.
The OpenSSH fixture authenticates the existing OS account with generated keys;
it does not create a new OS account.

The full local `cargo xtask check` passed after both corrections: workspace
tests/Clippy, frontend tests/type checking/lint/build, release/lab tooling tests,
isolated RDP/VNC fixtures, package-layout contracts and fuzz compilation. These
checks do not establish native GUI or installer acceptance for the new changes.

## Limits

This verifies protocol streams, caller-owned parts and explicit reconnect/retry.
It does not prove native transfer-manager state, recursive recovery, automatic
resume, daemon restart during a transfer, Windows/Linux execution or very large
multi-gigabyte workloads. The existing [native transfer lifecycle receipt](transfer-lifecycle.md)
remains separate. An unreachable server can prevent remote part cleanup, and
cleanup retains its own request deadline. No distributed transaction or remote
cleanup guarantee is claimed.

The correction is included in the v0.1.22 Mac preview. Windows/Linux downloads
remain v0.1.12 and do not contain these changes.
GitHub Actions remain disabled.
