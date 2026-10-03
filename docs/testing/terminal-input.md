# Ordered, bounded terminal input

## Source change after v0.1.31

Local `terminal_write` previously called blocking PTY `write_all` on Tauri's
main thread. A child that stopped reading could fill the PTY input buffer and
prevent the UI from processing Close. Tauri documents the main-thread behavior
of synchronous commands in its [command guide](https://v2.tauri.app/develop/calling-rust/#async-commands).

The command is now async and reserves its terminal's writer before delegating
the blocking write to a worker. There is at most one input worker per local
terminal; overlapping direct IPC writes are rejected rather than accumulating
workers behind a blocked child. Explicit Close uses the child handle separately
from the writer, so it can stop and reap the child during a blocked write.

Saved local startup commands are deferred until attachment. Spawn returns the
terminal ID without writing saved input; attachment first releases the output
reader, then submits the command and its trailing carriage return through the
same input worker. The saved command is taken once, so another attachment
cannot replay it. A failed startup write closes its session. The renderer keeps
the pane in Starting until attachment completes and rejects normal input while
it is unready. A terminal that closes during startup stays closed instead of
being overwritten with Connected when the attachment response arrives.

The frontend shares one FIFO per native terminal across keyboard input, paste,
broadcast and macros. Only one IPC write is in flight for each destination.
Each FIFO retains at most 128 requests and 1 MiB of UTF-8 input, including the
in-flight request. Other terminals have independent queues. Failure or backlog
overflow cancels queued followers: sending the tail of a partially delivered
command would be unsafe. Already dispatched input may have reached the child;
the visible error asks the operator to inspect the terminal before retrying.

Queued input rechecks its pinned native ID and lifecycle generation before
dispatch. Paste also rechecks all approved destinations; macro actions recheck
all selected targets and Cancel. Closing or reconnecting does not reroute old
input into the replacement session, including SSH reconnects that retain an ID.

## Local terminal cleanup at Quit

The native Quit paths now start local PTY cleanup alongside the existing SSH
cleanup and await both before proceeding. This covers the app's
`ExitRequested` handler and its `Exit` fallback; Tauri distinguishes those
events in its [runtime event documentation](https://docs.rs/tauri/latest/tauri/enum.RunEvent.html).
It does not depend on the renderer unmounting terminal panes.

Starting shutdown immediately seals the local manager. A child created before
Quit cannot be published afterward: publication refusal also reaps that child.
All shutdown callers share completion, so a repeated request cannot mistake an
emptied registry for finished cleanup. A dedicated cleanup thread avoids
waiting behind blocked input workers in the runtime's blocking pool; if that
thread cannot start, the same cleanup runs in place.

Explicit Close and reader EOF retain the registered child until reaping
finishes, allowing concurrent Quit to await the child lock. Closing terminals
reject further input. Cleanup recovers owned controls from poisoned registry
or child locks, cancels unconsumed startup commands and attempts every registered
child even if another cleanup reports an error. Cleanup failures are reported
with redacted native diagnostics.

These checks cover directly owned PTY children. They do not certify every
descendant process tree, native OS termination path or Windows/WSL lifecycle.

## Focused regression checks

```sh
node --experimental-strip-types apps/desktop/test/terminal-input-queue.test.mjs
cargo test --locked -p mobarust terminal::tests:: -- --test-threads=1 --nocapture
```

The frontend regression covers FIFO ordering, independent panes, failure
propagation, generation changes, queued macro cancellation, UTF-8 byte pressure,
request-count pressure and recovery after a drained failed queue.

The Unix native regression starts a disposable PTY child with raw input and
echo disabled. After its readiness marker, it stops reading. A 1 MiB write must
remain blocked while an async timer stays responsive; a second write must be
rejected as busy. Close must reap the child, release the blocked writer with an
I/O error and remove the session. Assertions follow cleanup, and the fixture
also has a ten-second lifetime. It opens no network listener.

The same fixture also exercises attachment with a valid 16 KiB startup command.
It preloads only its disposable PTY input with nonblocking writes, then restores
the original descriptor flags so different Unix buffer capacities do not mask
input pressure. The output-start signal must be released while startup input
is still blocked, repeated attachment must be refused, and Close must release
and reap the fixture as it does for normal input.

On 2026-10-03, the 16 native terminal regressions passed on macOS ARM64, with
zero failures or ignored tests; 99 unrelated desktop tests were filtered out.
The installed bash, zsh and fish PTY fixtures ran. The test process used a
disposable HOME, blank SSH-agent settings and sanitized shell startup hooks.
The focused frontend queue regression and TypeScript check passed as well.
Validation was kept to this change rather than repeating the full workspace
and platform suites.
The startup follow-up also passed those 16 terminal checks; after strengthening
the fixture to preload the PTY input, the single input-pressure regression
passed again. Frontend type checking and lint passed on the updated source.

The Quit follow-up passed 17 terminal regressions on macOS ARM64, with 99
unrelated desktop tests filtered out. Its new manager case holds a child lock
to verify that repeated shutdown cannot acknowledge early, then checks actual
exit/reaping before fallback test cleanup. It covers multiple children,
late-publication refusal, unconsumed startup cancellation and poisoned controls.
The real input-pressure fixture also checks shutdown while a write is blocked.

This is production-manager and frontend routing evidence, not native GUI
observation. Windows ConPTY behavior under input pressure remains unverified.
Native process creation, resize and child cleanup remain separate synchronous
paths; these changes do not establish that every terminal operation is
nonblocking. Existing published installers are unchanged. GitHub workflows
remain disabled and roadmap completion is unchanged.

## Native macOS startup, split input and Quit — 2026-10-03

Source `dc8235dd114ab3f00e323c7e6f66151da5c8ba80` passed
`cargo xtask package-check` on macOS ARM64. The resulting debug app was copied
to a disposable lab, given a lab-only ad-hoc signature, and launched with a
private HOME, shell startup file and portable session store. SSH-agent settings
were blank. This was a source-candidate check, not the published v0.1.31 DMG.

Through the native UI, a saved local zsh profile displayed its startup review.
After approval, the terminal showed `MOBARUST_STARTUP_GUI_OK` and its shell PID.
A split was opened; synthetic input produced visible output in both panes.
Each shell then used `exec /bin/sleep 300`, leaving two directly owned native
children running for the Quit check. A PID-scoped check found no TCP listeners.
No SSH server, personal credentials or remote connection was used.

Choosing **Quit** from the native application menu exited the app and removed
both recorded children. Process checks confirmed all three PIDs absent, and
`lsof` found no open files in the lab before its disposable copy was removed.
No signal-based fallback cleanup was needed. The attempted Cmd+Q shortcut did
not visibly initiate Quit in this automation pass; menu Quit is the observed
result. An accessibility click also left keyboard focus in the other pane;
clicking the visible right pane established its input target.

This short check establishes saved local startup, visible split input and menu
Quit cleanup for this Mac source candidate. It does not establish blocked-input
GUI responsiveness, shortcut/focus acceptance, arbitrary descendant cleanup,
Windows/Linux behavior, sustained output or installer acceptance. Those gates
remain open; the focused manager regressions above retain their separate scope.

### Pane targeting follow-up on main

Terminal groups and xterm inputs now include the pane's displayed name in their
accessible labels. Name changes update the input label without recreating the
terminal. A theme-aware outline follows actual keyboard focus inside each pane,
making the input destination visible when working with splits. TypeScript and
lint checks cover this source change; updated native focus acceptance and
published installers remain separate.

### Remote attachment closure ordering on main

SSH, Telnet and serial attachment now preserve a terminal close event received
before the attachment IPC reply. A late successful reply cannot mark the closed
pane connected or request a resize on it. Local attachment already checks the
same close flag. Focused checks: TypeScript, component lint and the existing
remote-close status regression. The latter verifies close-to-status mapping,
not IPC/event ordering. Native reproduction of that race and new installers
remain pending.
