# Native transfer cancellation and SSH-close receipt — 2026-10-02

## Reproduced problem and correction

Closing an SSH tab during an upload disconnected its transport before signalling
the detached transfer worker. In the native lab, the original destination stayed
intact, but a 5,373,952-byte hidden part remained. The transfer eventually reported
the existing temporary-file cleanup error.

Each SSH session now owns its transfer workers in a `JoinSet`, reaps completed
workers while reading the shell, and cancels/drains them before disconnecting or
reconnecting. Explicit Close signals cancellation immediately. Workers finish
their existing part cleanup or a promotion/rollback already in its critical
section; they are not aborted midway through file replacement.

Recursive traversal and directory preparation also check cancellation between
directories and before reporting completion. A closed control channel counts as
cancellation, including for an empty upload tree. Earlier committed files and
created directories are not rolled back.

This is a post-v0.1.18 correction on `main`. Published installers are unchanged.

## Native evidence

macOS ARM64 / Apple M2, debug native app with the production frontend, based on
`167e358` and then rebuilt with this correction. A copied portable bundle had a
unique identity and disposable HOME/ZDOTDIR/XDG paths. Its actual PID/environment
was verified after CUA selected it. Generated Ed25519 keys and explicit generated
trust authenticated a disposable OpenSSH daemon; personal SSH configuration,
agent, keys and Keychain were not used.

The daemon and throttled TCP relay listened only on `127.0.0.1:55529` and
`127.0.0.1:55530`, verified with `lsof`. Source files and original destinations
were generated in separate fixture directories. Each source contained 32 MiB.

Observed on the first candidate:

- Native Download → destination picker → Replace started real SFTP streaming.
  Cancel at approximately 2.6 MiB produced `cancelled`, preserved the original
  destination bytes, and removed the local part.
- Retry required a new explicit approval, retained the cancelled row, and created
  a new job. With throttling removed, it completed all 33,554,432 bytes and matched
  the remote source exactly. SHA-256:
  `e09320c5b00b34bb704802136c599a95b3996332ba84d7c7f21112b6231b6bd0`.
- Native Upload → generated source picker → destination → Replace started SFTP
  streaming. Cancel at approximately 4.5 MiB preserved the original remote bytes
  and removed the remote part. The manager exposed Retry.
- Retrying that upload and closing its SSH tab reproduced the leftover described
  above. The error was visible in the global manager; it did not report success.

Observed on the rebuilt corrected candidate:

- The same upload was running with 4,325,376 remote part bytes before Close.
  Closing its SSH tab produced `cancelled` and zero active jobs. Original remote
  bytes were unchanged, the part was absent, no upload backup remained, and the
  relay observed transport disconnection.
- Native menu Quit exited the owned app with status zero after the transfer had
  finished cancelling. The owned daemon/relay were stopped and reaped, both ports
  refused new connections, and the generated key/trust directory was removed.

UI actions used native CUA controls and accessibility/screenshots, without page
JavaScript injection. Transfer rates from this throttled lab are not benchmarks.
No system SSH service, Remote Login, router or firewall change was made.

## Runnable regressions and validation

```sh
cargo test --locked -p mobarust session_transfer_drain
cargo test --locked -p mobarust recursive_empty_upload_scan
cargo xtask check
```

The drain regression verifies cancellation is scoped to the closing session,
part removal/original preservation, and that the session waits until its worker
finishes cleanup. It deliberately holds the worker pending after cancellation;
the other session's control remains live. The empty-tree regression checks both
explicit cancellation and a closed sender.

Both targeted checks, both native candidate builds and the full final local
`cargo xtask check` passed. The full check includes workspace tests/Clippy,
frontend tests/type/lint/build, release assets, isolated RDP/VNC fixtures, package
layouts and fuzz-target compilation. CI remains disabled.
The pre-push Apache-2.0/private-key-marker audit and documentation link checks
also passed.

## Remaining acceptance gates

This verifies single-file SFTP Cancel, download Retry, and active-upload SSH-tab
closure on macOS ARM64. App Quit while a transfer is still active, queued-command
closure races, recursive/multi-file GUI recovery, native SCP, interrupted network
cleanup, larger workloads and Windows/Linux behavior remain open. A lost transport
can still prevent remote cleanup; the explicit cleanup error remains necessary.
Metadata/cleanup retain their request timeouts. There is no immediate-cancellation
or whole-directory transaction guarantee. See the [promotion boundary](transfer-cancellation.md).
