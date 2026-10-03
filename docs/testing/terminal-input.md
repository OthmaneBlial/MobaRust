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

This is production-manager and frontend routing evidence, not native GUI
observation. Windows ConPTY behavior under input pressure remains unverified.
Native process creation, resize and child cleanup remain separate synchronous
paths; these changes do not establish that every terminal operation is
nonblocking. Existing published installers are unchanged. GitHub workflows
remain disabled and roadmap completion is unchanged.
