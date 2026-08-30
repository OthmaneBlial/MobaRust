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

Each file is committed independently: downloads use a temporary local sibling
and a guarded atomic replacement; uploads use a unique remote temporary path
and rename it into place only after the stream completes. Local destination
symlinks are refused, and Windows replacement does not delete an existing file
before the replacement succeeds. Existing files require the explicit overwrite
choice. Cancellation removes the in-flight temporary file and leaves no
partial destination file presented as complete.

## Consequences

An interrupted directory transfer may contain files that completed before the
interruption; the transfer manager reports cancellation and never claims the
whole tree is atomic. Directory metadata and symbolic links are not silently
recreated. SCP remains a separate single-file compatibility primitive until it
has its own transfer-manager integration.

## Verification

The local SSH fixture already verifies real SFTP streaming, cancellation,
rename, and cleanup behavior. The recursive path is covered by native compile,
workspace tests, and the desktop quality pipeline; a future fixture extension
should exercise multi-file trees, overwrite conflicts, symlink refusal, and
mid-tree cancellation end to end.
