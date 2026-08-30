# ADR 0008: Stream SFTP transfers through a bounded native manager

## Status

Accepted and implemented for single-file upload/download.

## Decision

SFTP transfers are owned by the Rust SSH manager and exposed to the React
renderer through typed `sftp://transfer` progress events. A transfer is
identified by an opaque UUID and has an explicit lifecycle:

`Queued -> Preparing -> Running -> Completed`

or:

`Running -> Cancelling -> Cancelled` / `Failed`.

At most three transfers run concurrently per desktop process. Cancellation is
cooperative: the manager signals the transfer, the copy loop observes the
signal between bounded reads/writes, and the transfer removes its temporary
destination before reporting cancellation.

Downloads stream into a uniquely named local sibling `.mobarust.part` file,
sync it, and rename it into place only after the complete remote byte count has
been copied. Uploads stream into a uniquely named remote sibling `.part` file
and rename it to the requested path only after completion. Replacing an
existing destination requires an explicit `overwrite` flag.

The frontend never receives an SSH connection, SFTP object, credential, or
secret. It receives only paths, byte counters, lifecycle state, and sanitized
operation errors. The native layer owns local file handles, SFTP channels,
cleanup, and cancellation.

Progress events also carry a native bytes-per-second estimate and a bounded
ETA derived from the transfer's monotonic elapsed time. These values are
advisory and are omitted until enough bytes have moved to produce a useful
estimate; the frontend does not infer network timing from wall-clock events.

Directory listing and remote mutations (create directory, rename, delete, and
bounded POSIX permission changes) use separate native SFTP jobs as well. They are spawned from the SSH
session loop, so a slow directory operation cannot stop the shell reader from
forwarding terminal output. Delete inspects remote metadata in Rust instead of
trusting a frontend-provided file type; deleting the remote root is rejected.

## Rationale

- A single-file transfer can be useful immediately without pretending that
  recursive transfers, pause/resume, or remote editing are complete.
- A separate SFTP channel allows terminal output to remain responsive while a
  transfer is active.
- Bounded buffers avoid loading large files into memory.
- Temporary destinations prevent a cancelled or failed transfer from looking
  like a completed file.
- Typed events keep the Tauri IPC surface narrow and auditable.

## Rejected for this milestone

- sending file bytes through React or Tauri event payloads;
- one unbounded task per user click;
- silently overwriting local or remote files;
- exposing a generic filesystem or shell command to the frontend;
- claiming recursive transfer, pause/resume, drag-and-drop, or remote editing
  before their cancellation and conflict semantics are implemented.

The browser now exposes server-provided mode, UID/GID, and owner/group metadata
when available. A chmod action accepts only a validated octal mode and requires
an explicit confirmation before sending a typed native SFTP metadata request;
it never builds a shell command.

Failed and cancelled transfer rows expose an explicit retry action. Retrying
creates a new bounded job and transfer ID, reuses only the non-secret source,
destination, protocol, and recursive flag, and asks for destination overwrite
confirmation again.

## Follow-ups

- recursive jobs with per-item conflict decisions;
- native file/directory picker integration;
- pause/resume where the protocol and remote semantics support it;
- remote editing with modification detection and atomic upload.
