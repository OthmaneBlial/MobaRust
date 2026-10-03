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
also rejected. It writes the new content to a unique remote temporary file,
reapplies the original mode, moves the old file to a unique rollback name,
promotes the complete temporary file, and removes the rollback copy. If
promotion fails, Rust attempts to restore the original before returning the
error. If restoration also fails or its result is uncertain, Rust retains the
backup and temporary copy for recovery and tells the operator to inspect the
target before retrying. Other handled failures clean their temporary copy. If
the final rollback-copy cleanup fails after a successful promotion, Rust
reports that the new file was saved and leaves the backup for inspection.

Conflict checks hash the bounded original bytes without decoding them using
the selected output encoding. This permits UTF-8/Windows-1252 conversion and
still reports a byte conflict if a concurrent writer introduces invalid text.
Save as with explicit replacement uses the same byte checks; the existing
target need not share the new file's encoding. Opening a document still
requires valid text in the requested encoding, and encoding new content still
rejects lossy Windows-1252 conversion.

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
tests cover the distinct recovery messages;
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
recovery acceptance, post-save observation failures, the final rename race,
and Windows/Linux execution remain separate gates. Published v0.1.24 Mac
and v0.1.12 Windows/Linux downloads are unchanged.
