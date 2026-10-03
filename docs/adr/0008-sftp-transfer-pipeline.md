# ADR 0008: Stream SFTP transfers through a bounded native manager

## Status

Accepted and implemented for single-file SFTP and SCP jobs, recursive SFTP
transfers (see [ADR 0018](0018-recursive-sftp-transfers.md)), and bounded
remote text editing (see [ADR 0022](0022-bounded-remote-text-editor.md)).

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

The SSH session owns its dispatched transfer workers and reaps completed tasks
while reading shell output. Session closure cancels its jobs and waits for their
cleanup before disconnecting the transport. It also drains the old generation's
workers before reconnecting. Workers are not aborted during promotion/rollback;
loss of the transport can still make remote cleanup fail explicitly.
[Native cancellation and SSH-close evidence](../testing/transfer-lifecycle.md).
Normal application Quit/window close defers runtime exit until the manager has
signalled all SSH sessions and awaited their completion; new SSH work is rejected
once shutdown begins. The receipt verifies macOS ARM64 menu Quit during an upload
and window close during a download. OS-originated Quit and other platform acceptance remain open;
force-kill, crash and runtime restart cleanup are not guaranteed.

Downloads stream into a uniquely named local sibling `.mobarust.part` file,
sync it, and replace the destination only after the complete remote byte count
has been copied. Uploads stream into a uniquely named remote sibling `.part`
file and rename it to the requested path only after completion. Replacing an
existing destination requires an explicit `overwrite` flag. Local commits
refuse symlink destinations; Windows uses the OS replace-existing move with
write-through semantics instead of deleting an existing file first.
Standard SFTP v3 rename refuses an existing target. For an explicit remote
overwrite, the native layer tries direct promotion, then moves the old file
to a nearby backup before promoting the complete temporary file. It restores
the backup if promotion fails. If restoration cannot be confirmed, it keeps
both files for manual recovery and reports the uncertainty; a failed backup
cleanup is reported after the new file has been promoted. When the existing
target is a regular file, its permission bits are applied to the complete
temporary file before promotion.
On main after v0.1.24, all three upload paths share an `LSTAT` destination
check. Directories and missing/unsupported type metadata refuse before
promotion. Explicit overwrite replaces a final-path symlink itself, including
directory and dangling links, without inheriting its mode or opening its target.
The same check runs again before fallback backup creation.
Single-file SFTP and SCP uploads reject selected local symlinks, including
dangling links, before opening a remote transfer. On Unix, file uploads also
open with `O_NOFOLLOW` and check the opened handle is a regular file, preventing
a final-path symlink swap between the initial check and open. Windows retains
that local path-swap race.

The frontend never receives an SSH connection, SFTP object, credential, or
secret. It receives only paths, byte counters, lifecycle state, and sanitized
operation errors. The native layer owns local file handles, SFTP channels,
cleanup, and cancellation.

Progress events also carry a native bytes-per-second estimate and a bounded
ETA derived from the transfer's monotonic elapsed time. These values are
advisory and are omitted until enough bytes have moved to produce a useful
estimate; the frontend does not infer network timing from wall-clock events.
Native progress notifications are throttled to at most one per 100 ms or per
8 MiB, with initial and terminal events always retained, so a fast large file
cannot turn every copy chunk into an IPC/UI update.

Directory listing and remote mutations (create directory, rename, delete, and
bounded POSIX permission changes) use separate native SFTP jobs as well. They are spawned from the SSH
session loop, so a slow directory operation cannot stop the shell reader from
forwarding terminal output. Delete inspects remote metadata in Rust instead of
trusting a frontend-provided file type; deleting the remote root is rejected.

## Rationale

- The initial single-file transfer slice shipped independently; recursive
  SFTP and remote editing were added later under their own decisions.
- A separate SFTP channel allows terminal output to remain responsive while a
  transfer is active.
- Bounded buffers avoid loading large files into memory.
- Temporary destinations prevent a cancelled or failed transfer from looking
  like a completed file.
- Typed events keep the Tauri IPC surface narrow and auditable.

## Rejected for the initial single-file milestone

- sending file bytes through React or Tauri event payloads;
- one unbounded task per user click;
- silently overwriting local or remote files;
- exposing a generic filesystem or shell command to the frontend;
- claiming recursive transfer, pause/resume, or remote editing before their
  cancellation and conflict semantics were implemented. Recursive SFTP and
  remote editing were later added; see ADRs 0018 and 0022. Pause/resume remains
  unimplemented.

The desktop SFTP view accepts an explicit native Tauri file-drop gesture for
uploads and has separate file and folder picker actions. Each action receives
only the paths selected by that user gesture, deduplicates and caps them at 16
entries, defaults folder transfers to SFTP, and still asks for a remote
destination and overwrite confirmation. The picker command returns path
metadata only; it does not read file contents. MobaRust does not watch
directories or scan the local filesystem. Downloads likewise use an explicit
native file-save or folder picker before the Rust transfer manager creates any
local temporary file.

The browser now exposes server-provided mode, UID/GID, and owner/group metadata
when available. A chmod action accepts only a validated octal mode and requires
an explicit confirmation before sending a typed native SFTP metadata request;
it never builds a shell command.

Failed and cancelled transfer rows expose an explicit retry action. Retrying
creates a new bounded job and transfer ID, reuses only the non-secret source,
destination, protocol, and recursive flag, and asks for destination overwrite
confirmation again.

## Follow-ups

- per-item conflict decisions for recursive jobs;
- pause/resume where the protocol and remote semantics support it;

## Upload destination regressions — 2026-10-03

The SDK's POSIX type predicates used overlapping codes as independent flags:
a symlink also matched regular/character, and block devices or sockets could
match directory. Type setters could corrupt another type, and conversion from
Unix filesystem metadata could relabel a socket. The local dependency now
compares the complete type field, replaces it on assignment, clears only an
exact match, and preserves the original Unix mode.

Before correction, the real OpenSSH link-replacement test reproduced an upload
created with mode `0600` becoming `0755`. Three metadata regressions also failed,
and the scripted SFTP fixture accepted an occupied destination without type
metadata. None of these checks uses a public SSH server.

```text
cargo test --locked -p mobarust-ssh --test sftp_metadata
cargo test --locked -p mobarust-ssh --test remote_editor
cargo test --locked -p mobarust-ssh --test local_sshd upload_replacement_replaces_links_without_following_their_targets -- --exact
cargo xtask check
```

The metadata cases verify mutually exclusive predicates, setter/clear behavior
and conversion of owned Unix files, directories, symlinks and a Unix-domain
socket. The private OpenSSH fixture checks file/directory/dangling symlinks
through both promotion APIs: create-only and queued cancellation preserve the
link; replacement preserves exact uploaded bytes and mode `0600`, leaves link
targets unchanged and removes owned parts/backups. A real directory refuses.
The existing ordinary-file replacement check still verifies mode `0640`.

The authenticated, memory-only SFTP fixture checks absent, zero, FIFO,
character, directory, block, socket and unknown type codes through both APIs
(16 combinations). Preflight and promotion return the same typed error, make
zero rename requests, preserve original bytes and remove the owned part.

The first full run passed 18/19 OpenSSH cases; the existing loopback X11 check
hit its five-second channel deadline before its xauth wrapper ran. Its unchanged
targeted rerun passed in 1.94 seconds, and the next full run passed all 19
OpenSSH cases in 23.74 seconds. The cause of that initial delay is not established.
No existing payload, assertion or deadline was relaxed.
A later run also failed the existing initial host-key rejection assertion;
that assertion now distinguishes unexpected redacted errors from acceptance
without changing its required result. The final workspace run passed all 19
OpenSSH cases in 33.12 seconds, plus the metadata and scripted fault cases.
These intermittent lab failures remain unresolved rather than a diagnosed fix.
The complete local `cargo xtask check` passed: workspace tests/Clippy,
frontend unit tests/type checking/lint/build, release/lab tooling, RDP/VNC
fixtures, package-layout contracts and fuzz compilation. Three manual
authentication labs remain ignored; real Xvfb reports its prerequisite skip.

These are source/protocol checks on macOS ARM64. Desktop SCP, single-file SFTP
and recursive per-file preflights use the shared guard, but native collision
dialogues and whole recursive workflows still need acceptance. Other-platform
metadata conversion is unobserved. Ancestor/final-path swaps between requests,
a malicious server and lost rename replies remain limitations. Published
v0.1.24 Mac and v0.1.12 Windows/Linux downloads are unchanged; no broader roadmap
gate is closed by these regressions.
