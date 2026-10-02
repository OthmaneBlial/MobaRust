# Transfer cancellation before file promotion — 2026-10-02

A cancellation could arrive after the byte copy had finished, while fsync or
SFTP metadata/close operations were still pending. The desktop then promoted
the complete temporary file without checking the request again. This could
replace an existing destination despite a cancellation already being queued.

## Corrected boundary

- Every local download commit checks its cancellation receiver before creating
  or replacing the final path. Rejected commits use the existing partial-file
  cleanup path. This covers single-file SFTP/SCP and recursive SFTP downloads.
- Single-file SFTP/SCP uploads and recursive SFTP uploads call the cancellable
  promotion method. It checks before preparation and again after remote metadata,
  immediately before the first rename. A sent cancellation or closed control
  channel produces cancellation and attempts temporary-file cleanup.
- Once that first rename begins, promotion or rollback must finish. Dropping
  that future halfway through a backup/restore sequence could endanger the
  original. A late request at this stage can therefore finish as completed.
- The existing non-cancellable library promotion method remains available;
  both methods share the same replacement, permission and rollback logic.

The correction is on `main` after v0.1.18; existing release assets are unchanged.

## Runnable checks

```sh
cargo test --locked -p mobarust local_commit
cargo xtask test-ssh
cargo xtask check
```

The local commit regression checks explicit cancellation and a closed sender,
with both an existing overwrite target and an absent create-only target. It
asserts unchanged original bytes or no destination, then verifies part cleanup.
Existing checks retain successful replacement, create-only collision refusal,
create-only completion and symlink-target preservation.

The new OpenSSH integration case uploads real temporary bytes to a disposable
loopback daemon. It exercises both overwrite and create-only promotion with:

1. cancellation queued before preparation;
2. cancellation after polling the promotion into its asynchronous metadata
   request, before the first rename;
3. a closed cancellation sender.

All six cases assert cancellation, original-byte preservation or no created
final destination, removal of the completed part and absence of a backup.
The metadata case polls the future once and then sends cancellation; no sleep
or probabilistic timer selects the phase. Invalid same-path promotion is also
rejected without deleting the original.

`cargo xtask test-ssh` passed all 41 unit tests and 12 integration cases on
macOS ARM64. IPv6 ran; the optional real Xvfb case reported its prerequisite
skip. The wrapper isolates HOME/XDG and removes inherited SSH-agent/askpass
configuration. Daemons use generated keys/trust and loopback addresses; their
fixture destructors kill/reap the owned daemons and remove temporary data.
No system SSH service, Remote Login, router or firewall change is required.

The full local `cargo xtask check` also passed: workspace tests and Clippy,
frontend tests/type checking/lint/build, release-asset checks, isolated RDP/VNC
fixtures, package-layout checks and fuzz-target compilation. The pre-push
license and private-key-marker audit passed. These are local checks; GitHub
Actions remain disabled.

## Limits and next acceptance gate

These pre-promotion regressions are local commit and real-SFTP protocol checks.
The separate [interrupted-stream checks](interrupted-transfers.md) verify 16 MiB
SFTP/SCP upload/download transport loss, original preservation, explicit remote
cleanup failure and full retry. These do not exercise the native manager.
The separate [native lifecycle receipt](transfer-lifecycle.md) verifies 32 MiB
single-file SFTP Cancel, download Retry and active-upload SSH-tab closure on
macOS ARM64. Broader recursive/SCP cancellation, interrupted large transfers,
app Quit during transfer and cross-platform native behavior remain open.
Metadata and cleanup requests retain their existing timeout; this change does
not promise immediate cancellation while one is pending. If the remote server
cannot remove a part, the cleanup failure remains explicit.

Recursive transfers commit each file separately. Cancelling the current file
does not undo earlier completed files or directories. A transport failure after
a rename can still require the existing backup/restore diagnostics; no distributed
filesystem transaction or automatic resume is claimed.
