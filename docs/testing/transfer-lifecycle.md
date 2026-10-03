# Native transfer cancellation and SSH-close receipt — 2026-10-02

## Reproduced problem and correction

Closing an SSH tab during an upload disconnected its transport before signalling
the detached transfer worker. In the native lab, the original destination stayed
intact, but a 5,373,952-byte hidden part remained. The transfer eventually reported
the existing temporary-file cleanup error.

Each SSH session now owns its transfer workers in a `JoinSet`, reaps completed
workers while reading the shell, and cancels/drains them before disconnecting or
reconnecting. Explicit Close signals cancellation immediately. Workers finish
their existing part cleanup or a promotion/rollback already in its critical
section; they are not aborted midway through file replacement.

Recursive traversal and directory preparation also check cancellation between
directories and before reporting completion. A closed control channel counts as
cancellation, including for an empty upload tree. Earlier committed files and
created directories are not rolled back.

This is a post-v0.1.18 correction on `main`. Published installers are unchanged.

## Native evidence

macOS ARM64 / Apple M2, debug native app with the production frontend, based on
`167e358` and then rebuilt with this correction. A copied portable bundle had a
unique identity and disposable HOME/ZDOTDIR/XDG paths. Its actual PID/environment
was verified after CUA selected it. Generated Ed25519 keys and explicit generated
trust authenticated a disposable OpenSSH daemon; personal SSH configuration,
agent, keys and Keychain were not used.

The daemon and throttled TCP relay listened only on `127.0.0.1:55529` and
`127.0.0.1:55530`, verified with `lsof`. Source files and original destinations
were generated in separate fixture directories. Each source contained 32 MiB.

Observed on the first candidate:

- Native Download → destination picker → Replace started real SFTP streaming.
  Cancel at approximately 2.6 MiB produced `cancelled`, preserved the original
  destination bytes, and removed the local part.
- Retry required a new explicit approval, retained the cancelled row, and created
  a new job. With throttling removed, it completed all 33,554,432 bytes and matched
  the remote source exactly. SHA-256:
  `e09320c5b00b34bb704802136c599a95b3996332ba84d7c7f21112b6231b6bd0`.
- Native Upload → generated source picker → destination → Replace started SFTP
  streaming. Cancel at approximately 4.5 MiB preserved the original remote bytes
  and removed the remote part. The manager exposed Retry.
- Retrying that upload and closing its SSH tab reproduced the leftover described
  above. The error was visible in the global manager; it did not report success.

Observed on the rebuilt corrected candidate:

- The same upload was running with 4,325,376 remote part bytes before Close.
  Closing its SSH tab produced `cancelled` and zero active jobs. Original remote
  bytes were unchanged, the part was absent, no upload backup remained, and the
  relay observed transport disconnection.
- Native menu Quit exited the owned app with status zero after the transfer had
  finished cancelling. The owned daemon/relay were stopped and reaped, both ports
  refused new connections, and the generated key/trust directory was removed.

UI actions used native CUA controls and accessibility/screenshots, without page
JavaScript injection. Transfer rates from this throttled lab are not benchmarks.
No system SSH service, Remote Login, router or firewall change was made.

## Runnable regressions and validation

```sh
cargo test --locked -p mobarust session_transfer_drain
cargo test --locked -p mobarust recursive_empty_upload_scan
cargo xtask check
```

The drain regression verifies cancellation is scoped to the closing session,
part removal/original preservation, and that the session waits until its worker
finishes cleanup. It deliberately holds the worker pending after cancellation;
the other session's control remains live. The empty-tree regression checks both
explicit cancellation and a closed sender.

Both targeted checks, both native candidate builds and the full final local
`cargo xtask check` passed. The full check includes workspace tests/Clippy,
frontend tests/type/lint/build, release assets, isolated RDP/VNC fixtures, package
layouts and fuzz-target compilation. CI remains disabled.
The pre-push Apache-2.0/private-key-marker audit and documentation link checks
also passed.

## Remaining acceptance gates

This verifies single-file SFTP Cancel, download Retry, and active-upload SSH-tab
closure on macOS ARM64. The application-exit cases below add Mac menu Quit and
window-close evidence. [Queue retirement regressions](queued-ssh-commands.md)
cover the native queue/control cutoff; GUI queue-race acceptance, recursive/multi-file
recovery, native SCP, interrupted network
cleanup, larger workloads and Windows/Linux behavior remain open. A lost transport
can still prevent remote cleanup; the explicit cleanup error remains necessary.
Metadata/cleanup retain their request timeouts. There is no immediate-cancellation
or whole-directory transaction guarantee. See the [promotion boundary](transfer-cancellation.md).


## Application shutdown correction — 2026-10-02

The desktop previously let the runtime exit without awaiting active SSH session
workers. Normal Quit and window-close requests now defer exit, announce a closing
state, and call the SSH manager's all-session shutdown. The manager signals every
session and its transfer controls before waiting for their completion
acknowledgements. It reuses the session drain described above rather than aborting
workers during promotion or rollback. The frontend stops macro/broadcast activity
and remote-monitor polling and displays a closing status while cleanup runs.

Shutdown also rejects new SSH commands and connection registrations. Pending
connection/shell setup observes a shared shutdown signal. Transfer registration
rechecks that signal under the control-registry lock, preventing a caller that
obtained its sender earlier from inserting a job after the cancellation sweep.
An already-closed session checks its close flag before another shell-loop turn.

```sh
cargo test --locked -p mobarust app_shutdown
```

The deterministic manager regression holds two synthetic session workers pending,
checks both close signals and transfer cancellations before either cleanup is
released, and verifies that shutdown waits for both acknowledgements. It also
checks rejection of further SSH commands and repeated shutdown with no sessions.
These checks do not exercise the native runtime callback or a real active transfer.

The isolated baseline app was alive, but CUA could not select its window
(`cgWindowNotFound`), so no baseline Quit-during-transfer outcome was observed.
A process signal used to stop that owned baseline for replacement is lab cleanup,
not native Quit evidence.

CUA could select the first rebuilt candidate. Native SFTP upload started against
an existing generated destination, and its remote part contained 5,046,272 bytes
before menu Quit. Quit exited with status zero and disconnected the relay but left
the part behind. The original destination remained unchanged. This disproved the
assumption that the runtime exit callback alone covered macOS menu Quit.

The installed `muda` macOS menu implementation maps predefined Quit to Cocoa's
`terminate:`. That bypasses Tauri's cancellable `ExitRequested` route. MobaRust now
replaces that one item in the default macOS menu with a regular item retaining its
text and Command-Q shortcut, whose handler invokes `app.exit(0)`. Window-close and
that menu action then share the deferred exit path. A changed upstream menu layout
fails explicitly instead of silently restoring direct termination. Other platform
menus are unchanged.

On the final rebuilt candidate, CUA observed the Quit item as `customAction:`.
The same upload was active with 2,621,440 remote part bytes before menu Quit.
The app exited with status zero, the relay observed disconnection, original remote
bytes were unchanged, and no remote part or backup remained.

A fresh final-candidate process then downloaded the generated 32 MiB `large.bin`
into an existing generated local destination. CUA approved the picker and explicit
Replace policy. With 2,162,688 local part bytes present, clicking the native window
close button exited with status zero, preserved the original local bytes, removed
the part, and disconnected the SSH relay. Neither exit check used a process signal
or a subsequent CUA observation that could relaunch the app.

Both final processes had verified disposable HOME/ZDOTDIR, native production
frontend assets, and generated key/trust references. The daemon and relay listened
only on `127.0.0.1:57118` and `127.0.0.1:57119`, verified with `lsof`. These are
single-file macOS ARM64 debug checks, not all-platform installer acceptance.
The closing status view is implemented; it was not separately captured during the
short cleanup interval.

If an OS termination source bypasses `ExitRequested`, the final `Exit` callback
also waits for the manager's native cleanup before returning. The event loop is
already ending on that fallback, so it cannot provide the same responsive status
view. OS-originated/Dock Quit, restart, simultaneous real transfers, native SCP,
recursive recovery and Windows/Linux exit acceptance remain pending.

There is no whole-directory rollback, immediate-exit deadline, crash/force-kill
cleanup guarantee, or Tauri restart acceptance claim. Metadata/cleanup retain their
request timeouts; final exit waits rather than forcibly interrupting replacement.
A dropped session acknowledgement logs a static warning and cannot establish
successful cleanup. This transfer receipt did not cover remote editor mutations
or other finite operations; the later backend correction is recorded below.


The final native bundle build and `cargo xtask check` passed, including all 93
desktop tests, frontend tests/type/lint/build, workspace Clippy/tests, protocol
fixtures, package contracts and fuzz-target compilation. The pre-push audit also
passed. After the native checks, the owned supervisor and daemon were stopped and
reaped, both loopback ports refused new connections, and generated key/trust files
were removed. GitHub CI remains disabled; published v0.1.18 installers are unchanged.

## Accepted file/editor operations during session cleanup — 2026-10-03

After v0.1.22, source inspection found that editor Save/Save as, file mutations,
directory listing, opening text and monitor collection still ran as detached
tasks. The session drain awaited transfers only, so Close, shell exit, reconnect
or normal app shutdown could disconnect while an accepted save was still running.

These six finite operations now join the existing session-owned worker set.
Cleanup keeps the authenticated transport alive until their replies settle;
transfer cancellation remains cooperative. Accepted editor saves and file
mutations finish under their existing operation deadlines rather than being
aborted during replacement or rollback. Commands still waiting in the queue
remain subject to [queue retirement](queued-ssh-commands.md), and are not replayed
after reconnect. This change adds no whole-directory transaction or force-kill
cleanup guarantee.

```sh
cargo test --locked -p mobarust finite_session_operations_settle_before_transport_cleanup
cargo test --locked -p mobarust session_transfer_drain
cargo xtask check
```

The new encrypted wire regression dispatches each of the six production commands
against generated credentials on an OS-assigned `127.0.0.1` port. It pauses the
peer before replying, verifies that the real session drain stays pending, then
releases the peer, observes the operation's error reply and completes transport
and listener cleanup. SFTP replies are deliberately unsupported; the fixture
never writes a remote file or runs a monitor command. The regression failed on
the detached implementation with `save was detached from session cleanup`.

The corrected regression and complete local `cargo xtask check` passed on macOS
ARM64 / Apple M2, including all 105 desktop tests, workspace tests/Clippy, frontend
checks, release tooling, protocol fixtures, package contracts and fuzz compilation.
GitHub CI remains disabled. Both [v0.1.23 Mac previews](../release/v0.1.23.md)
subsequently packaged this correction; their package checks are separate from
native editor Save/Quit acceptance.

This is backend worker-ownership evidence. Native Quit/Close during an editor
save, actual promotion/rollback failures and Windows/Linux acceptance remain
pending. Published v0.1.22 Mac and v0.1.12 Windows/Linux installers do not contain
this later source correction.
