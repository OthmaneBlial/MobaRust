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

## Follow-ups

- recursive jobs with per-item conflict decisions;
- native file/directory picker integration;
- retry policy with preserved transfer identity;
- pause/resume where the protocol and remote semantics support it;
- permissions/owner display and chmod actions;
- remote editing with modification detection and atomic upload.
