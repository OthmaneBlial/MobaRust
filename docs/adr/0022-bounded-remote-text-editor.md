# ADR 0022: Bound remote text editing and protect concurrent changes

## Status

Accepted and implemented for the bounded UTF-8/Windows-1252 remote editor
slice.

## Decision

The SFTP browser offers an edit action for regular files. Rust reads a bounded
document (maximum 4 MiB) as either UTF-8 or Windows-1252 and returns its
content, selected encoding, SHA-256 revision of the original bytes, and mode
metadata. The lightweight renderer editor keeps the content in an explicit
editable buffer and provides local search/replace plus a deliberate Save
action.

On save, Rust rereads the remote file and rejects the operation when its
revision differs from the token captured at open time. After the complete
temporary upload and mode application, it performs a second revision check
immediately before promotion so a concurrent update during a slow upload is
also rejected. It writes the new content to an exclusively created remote
temporary file, requesting mode `0600` at creation, reapplies the original
mode, moves the old file to a unique rollback name,
promotes the complete temporary file, and removes the rollback copy. If
promotion fails, Rust attempts to restore the original before returning the
error. If restoration also fails or its result is uncertain, Rust retains the
backup and temporary copy for recovery and tells the operator to inspect the
target before retrying. Other handled failures clean their temporary copy. If
the final rollback-copy cleanup fails after a successful promotion, Rust
reports that the new file was saved and leaves the backup for inspection.

After promotion is acknowledged, Save and Save as return a receipt of the
bytes they wrote, including the byte revision, encoding and size. They do not
reread the target: a later read failure must not misclassify an acknowledged
save, and a concurrent writer must not replace the editor's saved buffer or
revision. The receipt's modification time is unknown (`null`), and its mode
reflects the acknowledged creation/permission requests rather than a fresh
metadata observation. Explicit Open still returns observed remote metadata.
Backup removal failure sets `backupCleanupFailed` on the successful result;
both the editor and workspace notice show saved status with an inspection
warning. A subsequent save still checks the receipt's revision against the
current remote bytes before uploading.

Conflict checks hash the bounded original bytes without decoding them using
the selected output encoding. This permits UTF-8/Windows-1252 conversion and
still reports a byte conflict if a concurrent writer introduces invalid text.
Save as with explicit replacement uses the same byte checks; the existing
target need not share the new file's encoding. Opening a document still
requires valid text in the requested encoding, and encoding new content still
rejects lossy Windows-1252 conversion.

Save and Save as share a private temporary writer. Only an acknowledged
exclusive create establishes ownership: an existing regular file or symlink
at the temporary name is refused without truncation or cleanup. Write/close
failures after creation attempt cleanup of the owned part. New Save as files
retain mode `0600`; replacement reapplies the original mode after uploading.

## Security and reliability boundary

The editor requires remote metadata identifying a regular file. It refuses
directories, symbolic links, other file types, invalid UTF-8 reads, lossy
Windows-1252 writes, and files above the 4 MiB limit. A dangling symlink
still occupies its path for overwrite checks. Remote content is untrusted text
and is rendered only in a textarea; it is never interpreted as HTML or a
command. The renderer receives file
content because editing requires it, but no credential or shell state is
included.

SFTP v3 rename behavior is not uniformly atomic when the destination exists.
The implementation therefore promises complete-file promotion with rollback
attempts and conflict refusal, not an unconditional zero-gap guarantee. A
server-side path replacement between metadata checks and open/rename remains
a race because SFTP v3 offers no atomic no-follow compare-and-swap operation.
The editor refuses links visible at each metadata check but cannot prove that
the path stayed unchanged between requests. A future server capability check
may use a POSIX rename extension where available.

## Verification

The local OpenSSH fixture exercises upload, permission metadata, read, save,
permission preservation, conflict rejection, and cleanup over a real loopback
SFTP session, including symlink and dangling-link refusal. TypeScript, ESLint,
Rust tests, and the production build cover the command and editor wiring. Unit
checks cover the uncertain-restore diagnostic and saved-with-warning notice;
the fixture does not inject a double rename failure.

### Encoding conversion regression — 2026-10-03

On `main` after v0.1.24, Save and Save as use the shared bounded byte reader
for both pre-upload and pre-promotion checks. Previously they decoded existing
bytes using the new output encoding: converting accented Windows-1252 back
to UTF-8 failed with `RemoteFileNotUtf8`, even with the correct revision.
The new loopback OpenSSH regression failed on that operation before the fix
and passed after it.

```sh
cargo test --locked -p mobarust-ssh --test local_sshd \
  remote_editor_encoding_changes_preserve_byte_conflicts_and_permissions -- --exact
```

It checks UTF-8 → Windows-1252 → UTF-8 with exact accented/currency bytes,
returned encoding and size, mode `0640`, strict UTF-8 read refusal, stale
revision refusal after a non-UTF-8 external update, lossy-output refusal,
Create only preserving the existing legacy target, explicit Save as
replacement, and absence of editor temporary/backup files. It uses generated
keys, private fixture files and a `127.0.0.1` daemon; no system SSH service or
personal SSH configuration is changed.

The full local `CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true cargo xtask check`
passed on macOS ARM64: workspace tests and Clippy, all 17 OpenSSH cases,
frontend tests/type checking/lint/build, release/lab tooling, isolated RDP/VNC
fixtures, package-layout checks and fuzz-target compilation. The optional
real Xvfb case reported its prerequisite skip. GitHub workflows stayed disabled.

This is backend protocol evidence. Native encoding-selection and conflict
recovery acceptance, lost promotion replies, the final rename race,
and Windows/Linux execution remain separate gates. Published v0.1.24 Mac
and v0.1.12 Windows/Linux downloads are unchanged.

### Temporary ownership regression — 2026-10-03

On `main` after v0.1.24, editor temporary files use SFTP `CREATE | EXCLUDE |
WRITE` with creation permissions `0600`, rather than the ordinary upload
primitive's create-or-truncate behavior. Before the fix, a pre-existing
regular temporary path was accepted and the regression failed. The ordinary
upload API retains its existing overwrite semantics.

```sh
cargo test --locked -p mobarust-ssh --test local_sshd \
  remote_editor_temporary_collisions_preserve_unowned_files -- --exact
```

The local OpenSSH case starts a fresh child test process so the editor's
temporary-name counter is deterministic without changing the shared test
environment. For both Save and replacing Save as, it pre-creates a regular
temporary or a symlink to an unrelated fixture file. All four cases require
refusal, unchanged original/unrelated bytes, and preservation of the unowned
temporary or link. It then verifies successful new Save as with actual mode
`0600`, normal Save preserving `0640`, and no owned editor parts/backups.

The full local `CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true cargo xtask check`
passed again on macOS ARM64 with all 18 OpenSSH cases, workspace tests and
Clippy, frontend checks, release/lab tooling, RDP/VNC fixtures, package-layout
checks and fuzz-target compilation. Real Xvfb reported its prerequisite skip;
GitHub workflows remained disabled. No timeout or assertion was relaxed.

This does not make a malicious SFTP server trustworthy or prevent a path
from being replaced after creation. If the server creates a file but its
reply is lost, ownership is uncertain and the create-error path deliberately
does not remove that name. Native acceptance, wider servers/platforms and
updated installers remain pending; published downloads are unchanged.

### Committed-save receipt regressions — 2026-10-03

On `main` after v0.1.24, three portable regressions cover nine normal Save,
replacing Save as and creating Save as flows over authenticated loopback SSH:

```sh
cargo test --locked -p mobarust-ssh --test remote_editor
```

The scripted SFTP peer keeps files and generated host keys in memory and
injects failures at the protocol operation, without sleeps selecting a race:

- Deny reading the target after acknowledged promotion. The save must still
  return the committed text/revision and make no post-promotion target read.
- Replace the target with another writer's bytes immediately after promotion.
  The receipt must retain our saved buffer/revision; retry must refuse the
  changed remote file without writing or removing the external bytes.
- Reject backup removal after a replacing save. The result must retain a
  usable saved revision and a serialized cleanup warning, preserve the old
  backup, and permit a subsequent edit. Creating Save as has no backup and
  must not report this warning.

All three failed on the previous implementation: it reported the completed
save as a read/cleanup error or returned the other writer's content. They pass
with the receipt boundary. Checks also cover exact saved size, unknown fresh
mtime, absence of owned temporary files, and shutdown of the owned SSH/SFTP
workers. Frontend unit checks cover the shared saved/cleanup notice text used
by both save callbacks and the editor warning; type checking, lint and build
cover its wiring, rather than proving native display/focus acceptance.

The full local `CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true cargo xtask check`
passed on macOS ARM64: workspace tests and Clippy, all 18 OpenSSH cases and
the three new portable regressions, frontend tests/type checking/lint/build,
release/lab tooling, RDP/VNC fixtures, package-layout checks and fuzz-target
compilation. Real Xvfb reported its prerequisite skip. GitHub workflows stayed
disabled; no deadline or assertion was relaxed.

This does not prove a distributed transaction, a successful rename whose reply
was lost, or native warning/focus acceptance. Native/cross-platform GUI checks,
broader SFTP servers and updated installers remain pending. Published downloads
are unchanged.
