# ADR 0018: Bound recursive SFTP transfers by streaming files

## Status

Accepted and implemented for the SFTP transfer manager.

## Decision

Recursive upload and download are exposed through the existing typed transfer
command with an explicit `recursive` flag. The native Rust manager walks one
directory tree at a time, computes an aggregate byte total, and streams each
regular file individually. File contents are never collected into an in-memory
buffer.

The walk is capped at 100,000 entries. Local upload refuses to follow
symbolic links and only accepts regular files and directories. Remote names
are validated as single path components before they are joined, preventing a
server-provided name from escaping the selected local destination.
Recursive upload checks each remote destination directory without following
symlinks, so an existing directory link cannot silently redirect its files.
The SFTP request sequence cannot prevent a concurrent remote path swap.
Recursive download accepts only entries identified as regular files or
directories; remote symlinks and special files are refused rather than opened.

Each file is committed independently: downloads use a temporary local sibling
and a guarded atomic replacement; uploads use a unique remote temporary path
and rename it into place only after the stream completes. Local destination
symlinks are refused, and Windows replacement does not delete an existing file
before the replacement succeeds. Existing files require the explicit overwrite
choice. Cancellation removes the in-flight temporary file and leaves no
partial destination file presented as complete.
On main after v0.1.24, each uploaded file uses the shared no-follow destination
guard: explicit overwrite replaces a final-path link itself; real directories,
special files and absent type metadata refuse. Destination directory links
remain refused. [Protocol checks and native limits](0008-sftp-transfer-pipeline.md#upload-destination-regressions--2026-10-03).
Recursive downloads also refuse existing local subdirectory symlinks before
creating files. A concurrent local process can still swap an ancestor after
that check; directory-handle-relative operations would be needed to close
that race across platforms.

## Consequences

An interrupted directory transfer may contain files that completed before the
interruption; the transfer manager reports cancellation and never claims the
whole tree is atomic. Directory metadata and symbolic links are not silently
recreated. The transfer manager routes single-file SCP jobs through the legacy
SCP compatibility primitive; recursive SCP remains unsupported, and the UI
directs directory transfers to SFTP.

## Verification

The local SSH fixture already verifies real SFTP streaming, cancellation,
rename, and cleanup behavior. The recursive path is covered by native compile,
workspace tests, and the desktop quality pipeline; a future fixture extension
should exercise multi-file trees, overwrite conflicts, symlink refusal, and
mid-tree cancellation end to end.
