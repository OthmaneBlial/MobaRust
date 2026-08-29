# ADR 0022: Bound remote text editing and protect concurrent changes

## Status

Accepted and implemented for the first UTF-8 remote editor slice.

## Decision

The SFTP browser offers an edit action for regular files. Rust reads a bounded
UTF-8 document (maximum 4 MiB) and returns its content plus a SHA-256 revision
token and mode metadata. The lightweight renderer editor keeps the content in
an explicit editable buffer and requires a deliberate Save action.

On save, Rust rereads the remote file and rejects the operation when its
revision differs from the token captured at open time. It writes the new
content to a unique remote temporary file, reapplies the original mode, moves
the old file to a unique rollback name, promotes the complete temporary file,
and removes the rollback copy. If promotion fails, Rust attempts to restore
the original before returning the error. Temporary and rollback paths are
cleaned on all handled failure paths.

## Security and reliability boundary

The editor refuses directories, non-UTF-8 data, and files above the 4 MiB
limit. Remote content is untrusted text and is rendered only in a textarea;
it is never interpreted as HTML or a command. The renderer receives file
content because editing requires it, but no credential or shell state is
included.

SFTP v3 rename behavior is not uniformly atomic when the destination exists.
The implementation therefore promises complete-file promotion with rollback
attempts and conflict refusal, not an unconditional zero-gap guarantee. A
future server capability check may use a POSIX rename extension where
available.

## Verification

The local OpenSSH fixture exercises upload, read, save, conflict rejection, and
cleanup over a real loopback SFTP session. TypeScript, ESLint, Rust tests, and
the production build cover the command and editor wiring.
