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
SFTP download preflight accepts regular files and directories for planning;
unknown/special types refuse. Each file copy requires regular type metadata
from both STAT and its opened handle's FSTAT before reading. Ordinary single-file
symlinks still follow their regular target; directory reads require recursion.
Every native SFTP download result attempts session close, including preflight
failures. File close and local commit decide completion; a later session-close
error does not relabel committed files as failed.
Transfer SFTP parts and editor parts now share exclusive creation requesting
mode `0600`. Native SCP first reserves an exclusive private SFTP part, then
streams real legacy SCP data using `C0600`. Local download parts share
create-new semantics and request mode `0600` on Unix. A new final file retains
that mode; replacing an existing remote regular file still restores its mode.
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
forwarding terminal output. Delete inspects no-follow remote metadata in Rust
instead of trusting a frontend-provided file type: final-path links are unlinked,
and only real empty directories use RMDIR. Native mutations require a named final
entry: root aliases and final `.`/`..` components refuse before session lookup.
Trailing slashes are removed before no-follow inspection; significant whitespace
and interior/leading components remain unchanged. Permission changes use fresh
LSTAT type metadata, refuse final symlinks and unknown types, and require explicit
selection of a link's target. Known special entries remain chmod-capable without
opening their contents.
The confirmation names symbolic links and explains the empty-directory limit.

On main after v0.1.25, browser listings and recursive download planning share
per-directory limits: 10,000 displayed entries, 10,002 received entries including
filtered `.`/`..`, and 16 MiB of cumulative decoded filename/longname/owner/group
text plus derived action paths. The READDIR phase has one 12-second deadline;
channel setup, OPENDIR, mutex waiting and the subsequent CLOSE are separate.
This is a text budget, not a whole-process memory or whole-tree time limit.
Refusal discards the partial listing and awaits directory CLOSE, retaining the
original listing error if close also fails.

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

## Private transfer parts and close acknowledgements — 2026-10-03

The three OpenSSH baselines reproduced SFTP/SCP parts at mode `0644` and a
temporary SFTP upload accepting an occupied regular file. The local download
creation baseline also reproduced `0644`. These files belonged to disposable
private fixtures; this demonstrates unsafe defaults, not an observed disclosure.
An initial desktop baseline build exhausted disk space; after reclaiming only
obsolete repository compiler output, the unchanged baseline ran and failed its
mode assertion. Source and validation receipts were retained.

The desktop SFTP paths now use an exclusive temporary-upload API, leaving the
general library create/truncate upload API available for intentional direct
writes. SCP reserves its part through the same exclusive opener before its
legacy sink is invoked. A failed or unacknowledged create never establishes
cleanup ownership. Once creation is acknowledged, copy, close and cancellation
failures clean the owned part. Callers clean only a successfully handed-off part.
The editor uses the same private copy/close helper.

Both copy and reservation await close, then check cancellation before returning
ownership. A deterministic wire fixture reproduced cancellation queued by the
server's close handler being returned as success; the corrected methods report
cancellation and remove the owned part. Another fixture reproduced a known
close permission denial becoming generic SFTP I/O failure. The SDK now retains
structured errors through READ, FSTAT, WRITE, fsync and CLOSE; MobaRust maps the
status code while keeping server text out of its displayed error.

```text
cargo test --locked -p mobarust-ssh --test local_sshd transfer_parts_ -- --show-output
cargo test --locked -p mobarust-ssh --test remote_editor
cargo test --locked -p mobarust-ssh --test sftp_bounds
cargo test --locked -p mobarust ssh::tests::download_parts_are_private_and_exclusive -- --exact
cargo xtask check
```

The OpenSSH cases inspect mode during real 128 KiB copies, compare exact bytes,
refuse existing regular files, directories and file/directory/dangling links
through copy and reservation, preserve link targets/modes, and check queued
cancellation plus source-read failure cleanup. A reserved SCP copy promotes to
an existing mode-`0640` file without losing its mode. The local factory case
checks private creation and refusal without truncating an occupied file/link.
Scripted encrypted SSH/SFTP cases check read/write denial, close denial through
copy/reservation/editor Save, and cancellation during close through both transfer
methods; original bytes remain unchanged and no promotion rename is sent.

The final `cargo xtask check` exited successfully with 106 desktop tests,
42 SSH unit tests, 15 authenticated fixture tests (three manual probes ignored),
22 OpenSSH cases, seven scripted SFTP fault tests, 19 public packet/offset
checks, three metadata checks and five private SDK checks. OpenSSH completed
in 22.21 seconds. Workspace tests, Clippy, frontend tests/typecheck/lint/build,
release/isolated-launcher tests, RDP/VNC helper checks, package-layout checks
and fuzz-target compilation also passed. The real X11 server fixture explicitly
skipped because Xvfb or a safe system socket directory was unavailable; this
is not an external X server acceptance claim.

These are macOS ARM64 source/protocol and local-filesystem checks. All six
desktop transfer paths use the shared creation primitives, but full native
transfer/recursive workflow acceptance and new installers remain pending.
Windows inherits filesystem ACLs; this does not establish private Windows ACLs.
Servers must honor requested permissions/exclusivity. Legacy SCP reopens the
reserved pathname, and ancestor/final-path swaps remain possible between
requests. Lost create replies can leave an unconfirmed private part for manual
inspection; ambiguous ownership is never guessed merely to remove it. Published
v0.1.24 Mac and v0.1.12 Windows/Linux downloads are unchanged.

## Guarded downloads and acknowledged shutdown — 2026-10-03

Four authenticated wire baselines reproduced unsupported source/handle types
being read, cancellation during CLOSE returning success, and a READ failure
returning before the CLOSE acknowledgement. They exercised both public download
entry points. Source rejection now precedes OPEN; FSTAT checks the opened handle
before READ. A directory remains valid for native recursive planning, while
the file-copy API returns its typed directory error. Unknown types return an
actionable constant message without server text.

Tracing the sibling direct-upload API reproduced queued cancellation creating
or truncating its destination, cancellation during CLOSE returning success,
and WRITE failure returning before the close reply. It now checks cancellation
before CREATE and awaits close on copy failure. Its intentional CREATE/TRUNCATE
semantics remain: a failure after creation can leave partial direct output.
Native uploads continue to use their private, owned temporary parts.

A 128 KiB case then reproduced a lower-level shutdown gap with two failed WRITE
replies: the SDK returned at the next pending write error and relied on an
unawaited drop-close. Shutdown now drains outstanding replies, retains the
first error across polls and awaits CLOSE before returning it. An independent
in-memory SDK case supplies different errors for two writes and successful or
failed CLOSE replies; the first write status survives, the close occurs once,
and further nonempty writes refuse. No dependency version or feature changed.

```text
cargo test --locked -p mobarust-ssh --test remote_editor --test sftp_bounds
cargo test --locked -p mobarust-ssh --test local_sshd download_source_guards_keep_regular_files_and_file_symlinks_working -- --exact --show-output
cargo xtask check
```

Targeted checks pass: 13 authenticated SFTP fault cases and 20 bounded SDK
packet/state cases. Rejection checks cover 18 source-type/API combinations with
zero OPEN/READ, no progress and an untouched provided writer, plus eight changed
handle types with an acknowledged close and no READ. Gated READ/FSTAT/WRITE
denial cases prove the method stays pending until the close reply; cancellation
and permission-denied close cases return errors. Direct queued-cancel cases
preserve occupied bytes/mode or leave a new path absent. Existing editor and
private-upload cases still pass.

The real macOS ARM64 OpenSSH case byte-matches regular and file-link copies
through both APIs, checks progress and queued cancellation, and verifies typed
directory/directory-link, dangling-link and Unix-socket rejection. Original
bytes, modes and link targets stay unchanged. Keys, HOME and endpoints belong
to disposable loopback fixtures. The native download code now has one final
session-close attempt for both single-file and directory results; close errors
after committed data preserve that result rather than reporting a false failure.

The final `cargo xtask check` exited successfully with 106 desktop
tests, 42 SSH unit tests, 15 authenticated fixture tests (three manual probes
ignored), all 24 OpenSSH cases (23.13 seconds), 13 SFTP fault tests, 20 packet/state
checks, three metadata checks and five private SDK checks. Workspace tests,
Clippy, frontend tests/typecheck/lint/build, release/isolated-launcher checks,
RDP/VNC helper tests, unsigned package-layout checks and fuzz-target compilation
also passed. The real X11 server fixture explicitly skipped its unavailable
Xvfb/safe-socket prerequisite; external X11 acceptance remains unverified.

Native transfer/recursive workflow and Windows/Linux server acceptance remain
pending. STAT/FSTAT type metadata is now required; servers omitting it refuse.
Concurrent swaps before OPEN can still block that request, and FSTAT is not an
atomic source snapshot. Request timeouts, ambiguous replies and dishonest
servers remain limits. Close/copy errors may occur after bytes reached a
caller-owned writer; atomic cleanup belongs to the native temporary-file flow.
SCP download behavior and published v0.1.24 Mac/v0.1.12 Windows/Linux artifacts
are unchanged. Native acceptance and new installers remain pending.

## No-follow entry deletion — 2026-10-03

The native Delete branch used STAT and chose REMOVE/RMDIR from the target's
type. An extracted baseline with the same dispatch reproduced refusal of
directory and dangling links. The new shared `remove_path` method reuses the
existing LSTAT directory check: real directories use RMDIR, while other entries
use REMOVE. It neither follows a final symlink nor recurses into a directory.
The native branch awaits this result without an early metadata `?`, so its
existing SFTP close attempt also runs when classification fails. Confirmation
still pins the connection and requires the user's approval; it now describes
link deletion and the empty-directory limit.

```text
cargo test --locked -p mobarust-ssh --test local_sshd deleting_remote_entries_unlinks_links_and_preserves_their_targets -- --exact --show-output
cargo xtask check
```

The corrected real OpenSSH case passes on macOS ARM64: it removes a regular
file, an empty directory, file/directory/dangling links and an owned Unix socket.
After every operation, the file target's bytes/mode and the nonempty directory
target's child/mode remain unchanged. Nonempty directory deletion refuses,
missing paths retain their typed error and a dangling target is never created.
All paths, keys and processes belong to the disposable loopback fixture.

The final `cargo xtask check` exited successfully: 106 desktop tests, 42 SSH
unit tests, 15 authenticated fixture tests (three manual probes ignored), all
23 OpenSSH cases (53.53 seconds), seven SFTP fault tests, 19 packet/offset
checks and three metadata checks passed. Private SDK/workspace tests, Clippy,
frontend tests/typecheck/lint/build, release/isolated-launcher tests, RDP/VNC
helper checks, unsigned package-layout checks and fuzz-target compilation also
passed. The real X11 server fixture explicitly skipped its unavailable
Xvfb/safe-socket prerequisite, so external X11 acceptance remains unverified.

Native dialog/connection-close acceptance and Windows/Linux server evidence
remain pending. The LSTAT and removal requests are separate: concurrent path
replacement, ancestor links and a dishonest server remain limitations. This
does not add recursive deletion or change literal root-path validation. Published
v0.1.24 Mac and v0.1.12 Windows/Linux downloads are unchanged.

## Named mutations and explicit permission targets — 2026-10-03

The native guard previously refused only literal `/` and `.`. A regression
against the actual empty-session manager reproduced 85 aliases reaching session
lookup across rename source/destination, Delete, mkdir and chmod. No remote
request or real root mutation is needed to exercise that boundary.

The shared path helper now rejects empty/NUL paths, slash-only roots and a final
`.` or `..` component. It removes trailing slash separators before mutation,
preserving filenames with significant whitespace, Unicode, repeated interior
separators and relative parent prefixes. It intentionally does not collapse
`..`: ancestor symlink traversal belongs to the server's namespace. The native
operations all use this helper; library `remove_path` and `set_permissions` also
enforce it directly. Low-level SFTP rename/create/remove APIs retain their
caller-controlled semantics.

The chmod baseline changed a real file/directory symlink's target mode, including
directory links with `/` and `///` suffixes. Dangling links returned missing-path
instead of the link-specific refusal. The memory peer also recorded SETSTAT
requests for all eight missing/zero/link/unknown-type selections. The existing
Delete regression reproduced refusal for both directory-link slash suffixes.

Chmod now bounds the mode and validates the path before any request, performs
LSTAT, then sends SETSTAT only for known regular/directory/FIFO/character/block/
Unix-socket types. Missing or unknown types refuse; final links return an
instruction to select the target explicitly. The frontend gives that instruction
before its mode prompt when a listing identifies a link; Rust revalidates live
metadata regardless of the listing. Upload promotion's regular-part mode restore
uses the same guard. No file OPEN is required to repair an owned mode-000 entry.

```text
cargo test --locked --workspace ssh::tests::remote_mutations_require_named_paths_before_session_lookup -- --exact --show-output
cargo test --locked -p mobarust-ssh --test remote_editor remote_permission_guards -- --show-output
cargo test --locked -p mobarust-ssh --test local_sshd remote_permission_changes -- --show-output
cargo test --locked -p mobarust-ssh --test local_sshd deleting_remote_entries -- --show-output
cargo xtask check
```

The checks cover refusal before session lookup, zero SETSTAT/OPEN/READ for unsafe
metadata, preserved bytes/modes/links, mode-000 recovery, normal directory/socket
chmod and typed missing-path/invalid-mode refusals. In-memory positive controls
preserve all six supported type masks without opening an entry. Root aliases
never reach a real server; all OpenSSH paths belong to an owned loopback fixture.

The final full local `cargo xtask check` exited successfully on macOS ARM64:
107 desktop tests, 42 SSH unit tests, 15 authenticated fixture tests (three
manual probes ignored), all 25 OpenSSH cases (29.90 seconds), 14 SFTP fault
tests, 20 packet/offset checks, three metadata checks and five private SDK tests
passed. Workspace tests/Clippy, frontend tests/typecheck/lint/build,
release/isolated-launcher tests, RDP/VNC helper checks, unsigned package-layout
checks and fuzz-target compilation also passed. The first full attempt stopped
at a test-only octal-literal lint; the equivalent `0o000` correction passed the
rerun without relaxing assertions. The real X11 server fixture explicitly
skipped its unavailable Xvfb/safe-socket prerequisite; external X11 acceptance
remains unverified.

This is a final-entry policy, not a filesystem sandbox: a named path can itself
alias a directory through an ancestor link or server mount. LSTAT and SETSTAT/
REMOVE remain separate requests, so concurrent swaps and dishonest metadata are
not prevented. Native dialogue/connection lifecycle acceptance, Windows/Linux
metadata and updated installers remain pending. Published previews are unchanged.

## Bounded directory listings — 2026-10-03

The corrected baseline reproduced three aggregate gaps using authenticated
loopback replies within the existing 256 KiB packet limit: 10,003 filtered dot
entries were accepted, 129 long names accumulated, and replies arriving every
two seconds kept the listing pending beyond the test's 14-second outer bound.
An earlier single-batch dot fixture exceeded the packet cap; it was split into
legal batches before these results were recorded. An initial compile ran out
of disk space and produced no test result.

The shared reader now counts filtered wire entries and cumulative text before
allocating action paths, and bounds the complete READDIR phase. The regression
checks typed size/time refusals, exactly one acknowledged directory CLOSE, and
an ordinary Unicode listing on the same SFTP connection after each refusal.
A positive control preserves size, mode and exact regular-file classification.
The existing unit check covers inclusive boundaries and saturating overflow.

```text
CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true cargo test --locked -p mobarust-ssh --lib --test remote_editor
CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true cargo xtask check
```

The focused run passed all 42 SSH unit checks and 15 authenticated SFTP fault
checks. All fixture listeners bind only `127.0.0.1`, with generated keys and
in-memory files; personal SSH services, keys and configuration are unused.
The complete local `cargo xtask check` then passed: 107 desktop tests,
42 SSH unit tests, 15 authentication cases (three manual labs ignored), all
25 OpenSSH cases (20.89 seconds), 15 SFTP fault cases, 22 packet/state checks,
three metadata cases and five private SDK cases. Workspace tests/Clippy,
frontend tests/typecheck/lint/build, release/isolated-launcher tooling,
RDP/VNC helper fixtures, unsigned package-layout contracts and fuzz compilation
passed. The first full run stopped at a test-only `int_plus_one` lint; its
mathematically equivalent strict comparison preserves the packet boundary.
The real X11 server fixture explicitly skipped its unavailable Xvfb/safe-socket
prerequisite; no assertion, packet cap or existing deadline was relaxed.

These checks establish the shared protocol boundary, not native browser or
whole recursive workflow acceptance. External cancellation/drop, dishonest
servers and unacknowledged CLOSE replies remain limitations. The new guard is
source after v0.1.25 and is absent from the published Mac/Windows/Linux previews.
