# Text-input dialogue checks — 2026-10-02

## Saved startup command review on main

**2026-10-03, after the published v0.1.20 Mac preview.** Saved local profiles
already asked before sending startup input, but the shared SSH connection
handler sent configured startup commands without asking. This included profiles
from a session JSON import and sessions opened by a macro.

Both paths now use the shared startup review dialogue. It displays the complete
command as literal text and names its destination. SSH also explains repetition
after automatic reconnect when enabled. Cancel, Escape, closure, page teardown,
an unavailable dialogue or an already-pending review prevents startup approval.
SSH captures the request and connection settings before waiting and does not
create its authentication event channel or invoke `ssh_connect` until approval.
The local path likewise waits before opening a terminal.

Both handlers capture the owning macro, if any, before review and recheck that
it is still running and has not been cancelled after approval. A later Continue
cannot revive an emergency-stopped macro's pending session startup. Approval is
once per explicit connection; the native reconnect path retains the approved
configuration. Profiles without a startup command have no additional dialogue.

Frontend unit tests, TypeScript checks, ESLint and the production build passed
on macOS ARM64. The existing `text-prompt.test.mjs` exercises the production review helper with
the existing DOM boundary: full Unicode/markup-looking text, all 16 KiB of a
bounded command, initial Cancel focus, cancellation/closure/page teardown,
concurrent/unavailable review refusal, explicit approval, and ownership stopped
before or during review. This is helper/control-flow evidence, not a native GUI
acceptance receipt. The [disposable native setup lab](ssh-lab.md#repeatable-native-shell-setup-lab-on-main)
provides generated pinned profiles for the next native check. Updated installers
and native approval/focus/emergency-stop observations remain pending; published
v0.1.20 downloads do not contain this change.

## Earlier native dialogue baseline

These changes follow v0.1.17 and are included in the v0.1.18 Mac preview.
Windows/Linux installers remain v0.1.12. The detailed native observations below
used isolated ARM64 debug copies; the [v0.1.18 release-copy recheck](remote-desktop-renderer.md#v0118-release-bundle-recheck)
adds settings rejection/focus and PTY/VNC-helper shutdown evidence.

All 19 former `window.prompt` callers now use the shared HTML `dialog` helper:
profile naming, explicit OpenSSH config paths, settings/session JSON imports,
clipboard fallbacks, upload destinations, remote mkdir/rename/permissions and
the remote editor's Save as path. Existing backend validation remains in place.
File actions stop if the active SSH session changed while waiting for text. Cancellation returns no value; a second
concurrent request is declined rather than replacing or queuing an action.

## Automated checks

```bash
pnpm --dir apps/desktop check
pnpm --dir apps/desktop lint
pnpm --dir apps/desktop test:unit
```

The new `text-prompt.test.mjs` exercises submitted Unicode text, close-event
ordering, Cancel/Escape/explicit close/page exit, empty input versus cancellation,
concurrent requests, read-only multiline values, modal-open failure and recovery.
It also exercises explicit confirmation and all three overwrite results.
Its minimal DOM boundary tests promise ownership; it does not simulate native
rendering, focus or a protocol connection.

`terminal-paste-approval.test.mjs` checks cancellation, unavailable destinations,
changed selections, added/removed targets, replacement IDs and reconnects that
retain the same SSH ID. `macro-recording.test.mjs` checks native-ID and lifecycle
generation pinning before subsequent writes. No live network is needed by these
regression checks.

Type checking, ESLint, the frontend unit suite, frontend production build and
`cargo xtask check` passed. The latter includes Rust workspace tests, Clippy,
helper/fixture checks, package-layout checks and fuzz-target compilation. The
macOS ARM64 production app build passed with locked Rust dependencies; this is
an ad hoc signed development build, without notarization.

## Browser observations

An ignored loopback page imported the actual production helper and stylesheet.
Chrome checks observed:

- A prefilled path field received focus and accepted Unicode via Enter.
- Escape returned cancellation and restored focus to the opening button.
- Multiline JSON remained multiline on acceptance.
- A read-only copy field did not change when a character was typed.
- Modal accessibility state excluded the background controls.
- Light mode used the workstation palette with visible field/button focus.

The approval helper was also observed in Chrome: Cancel received initial
focus; Enter and Escape cancelled, explicit Continue approved, and overwrite
choices returned three distinct results. A controlled lifecycle mutation
invalidated pending paste approval despite an unchanged native SSH ID; an
unchanged target retained approval. This harness does not prove an actual SSH
reconnect or shell write.

## Native macOS ARM64 observations

Native window control became available on the subsequent check. A fresh copy of
the production app used `portable.flag`, a separate HOME/data directory, generated
SSH keys and a loopback SSH/SFTP fixture. No personal SSH agent or configuration
was used. Through the actual application UI:

- Saved-profile deletion focused Cancel. Enter cancelled and the profile remained.
- A cancelled multiline local-terminal paste produced no marker file. Explicit
  approval inserted the reviewed command in bracketed-paste mode; a separate
  Enter created the marker with exact expected bytes.
- Unicode mkdir and rename produced the expected directory on the SFTP fixture.
- Escape cancelled SFTP directory deletion; the directory remained intact.
  The native confirmation was also inspected in light mode with visible Cancel
  focus, readable text and background modality.
- Invalid octal mode `888` was rejected. Mode `640` required explicit approval
  and the fixture's filesystem mode matched afterwards.
- Save as with Create only created a new file with byte-identical contents.
- Escape during an existing-target overwrite choice left the original intact.
- Create only on an existing target displayed the collision error and left its
  original bytes intact. Explicit Replace then wrote exactly the edited bytes.

The computer-use clipboard operation reported a timeout while the native paste
dialogue was open; the UI had received the paste. The test inspected the pending
dialogue and its result without repeating a potentially approved action. These
observations do not establish Windows/Linux WebView compatibility.

### Follow-up native acceptance and emergency-key repair

A second isolated portable app used the `d941ced` baseline, then a production
build with the emergency-key/import-error fixes after `7862f18`. A disposable
TCP relay interrupted only the generated loopback SSH connection; its listener
remained available. The actual app automatically reconnected, corroborated by
new relay connections and successful-connection audit events. No personal
keys, agents, configuration or servers were used.

- Paste approval stayed open across a real reconnect. Continue refused the
  stale approval; a subsequent Enter did not create its marker file.
- A two-action macro approved its first action and cancelled its second.
  Only the first marker contained the expected bytes.
- Reconnect while either the initial macro approval or a per-action approval
  was pending refused execution. Neither marker was created. The backend's
  reconnect loop retains the native SSH ID; its generation changes invalidate
  the frontend approval.
- Cancelled and malformed multiline session JSON imports preserved all four
  original profile IDs. A valid import added exactly one profile. A subsequent
  malformed import followed by a valid empty import retained all five profiles
  and cleared the old error, displaying only the successful import notice.
- A local startup command created no tab or marker after Escape. Explicit
  Continue opened its local terminal and created the exact `startup` marker.
- A native upload picker, Unicode destination and Create only choice completed
  a 33-byte SFTP upload whose contents matched the source byte for byte.
- With three ready terminals and only two selected, an approved paste from
  the unselected third terminal, followed by Enter, appended exactly two
  distinct shell PIDs to the marker. The source did not receive the command.

The original bubbling key handler failed to stop broadcast with Escape while
xterm had focus, or with a custom emergency shortcut while a modal was open.
The shared emergency handler now runs in window capture, synchronously
invalidates active broadcast/macro execution, and leaves ordinary shortcuts
in the bubbling handler. The rebuilt native app verified:

- Escape from a focused terminal disabled broadcast.
- Configured `Mod+Shift+X` during paste approval disabled broadcast. Continue
  then refused the pending paste; its marker remained absent after Enter.
- Escape during paste approval both cancelled the dialog and disabled
  broadcast; its marker remained absent.
- With broadcast/macros inactive, a raw one-byte SSH stdin probe still
  received Escape as `0x1b`.

The full local `cargo xtask check` and locked macOS ARM64 app build passed with
these runtime fixes. The fixture app, SSH/HTTP servers and relay were stopped
after acceptance. These checks do not establish server-restart recovery,
retry exhaustion, all transfer policies or other operating systems.

### Repeatable native emergency regression check

Use the disposable portable setup in [the native runbook](native-workflow.md).
Choose a custom emergency shortcut distinct from the other configured keys,
for example `Mod+Shift+X`. Use a fresh absolute path inside the fixture HOME
for each marker; never a personal or production path.

1. Open three ready local/loopback SSH terminals. Select exactly two for
   broadcast and focus the unselected third terminal.
2. Paste `echo $$ >> "<fixture-home>/broadcast-pids.txt"` followed by a newline.
   Explicitly approve, then press Enter. Assert exactly two distinct numeric
   lines in the marker. Press Escape and verify the broadcast banner disappears.
3. Re-enable broadcast and paste a multiline marker command. While approval
   is open, press the custom emergency shortcut, then Continue and Enter.
   Assert that the marker is absent and broadcast is disabled.
4. Repeat with a fresh marker and Escape instead of the custom shortcut.
   Assert that the modal closes, broadcast is disabled and the marker is absent.
5. With broadcast/macros inactive, run a raw one-byte stdin probe and press
   Escape. Assert that the received byte is `0x1b`.

For steps 3–4, use `printf marker > "<fixture-home>/paste-emergency.txt"`
(then `paste-escape.txt`) with a trailing newline. The step 5 probe is:

```bash
python3 -c 'import sys,tty,termios; a=termios.tcgetattr(0); tty.setraw(0); b=sys.stdin.read(1); termios.tcsetattr(0,1,a); open("<fixture-home>/escape-byte.txt","w").write(repr(b))'
```

Check disposable marker contents without relying solely on UI notices:

```python
from pathlib import Path

home = Path("<fixture-home>")  # replace with the owned disposable directory
pids = (home / "broadcast-pids.txt").read_text().splitlines()
assert len(pids) == 2 and all(pid.isdigit() for pid in pids)
assert len(set(pids)) == 2
assert not (home / "paste-emergency.txt").exists()
assert not (home / "paste-escape.txt").exists()
assert (home / "escape-byte.txt").read_text() == repr("\x1b")
```

## Approval and connection ownership

All 20 former `window.confirm` callers now use the same HTML modal. Cancel,
Escape, page exit, an unavailable modal or a concurrent second request decline
approval. Confirmations initially focus Cancel. Overwrite actions distinguish
Cancel (`null`), Create only (`false`) and Replace (`true`); cancellation never
starts a create-only transfer or save.

Native paste captures its source, selected destination set, native IDs, xterm
instances and connection generations before awaiting approval. It checks them
afterwards and sends nothing if they changed. Each destination retains its own
bracketed-paste setting. All selected writes settle before delivery errors are
reported; a failed write can still mean other targets received input.

Macros pin the same connection generations and recheck before each terminal
write, including after per-action approval. Cancellation is checked again after
approval; opening a saved session awaits its startup-command approval. File
delete and chmod also recheck the active SSH session after confirmation.

## Remote file connection generations

Checking only the active SSH ID is insufficient: automatic reconnect retains
that ID. On the `520748c` native baseline, a folder-path dialog left open across
a controlled relay interruption created its folder after reconnection and
acceptance. The disposable `stale-before-fix` directory reproduced the gap.

File actions now reuse the macro/paste lifecycle binding. Upload/download,
transfer retry, mkdir, rename, delete and chmod pin the original workspace,
native ID and connection generation before a picker or approval. Before native
dispatch they require that exact binding to remain connected. Existing active
session guards still apply to file-browser actions. A changed binding stops the
action with a notice asking the operator to reopen it.

The remote editor retains the binding captured before reading its document,
including across successful saves. Save and Save as refuse writes after that
connection changes, even if the path dialog or overwrite confirmation remained
open. Content revision checks remain a separate backend protection.

The rebuilt macOS ARM64 app after `520748c` verified real relay interruptions,
each followed by a new transport and a new authenticated connection-success
audit event:

- Pending mkdir, rename, delete and chmod approvals were refused. The new
  paths remained absent; the protected original retained its name, bytes and
  mode `0644`. A freshly reopened mkdir created its Unicode directory.
- Pending upload destination, download overwrite and transfer-retry approvals
  were refused. No stale upload/download target was created, and retry did
  not replace the original target.
- Create only upload on an existing target failed without changing its bytes.
  A new explicit retry/replacement completed with byte-identical source data
  (35 bytes). A fresh Create only download matched its source (38 bytes).
- Editor Save as approval across reconnect and a subsequent ordinary Save
  both refused writing, retained the dirty draft and displayed an inline
  reopening instruction. The original file stayed unchanged and the new path
  remained absent. After reopening, Save and then Save as both wrote the exact
  expected bytes on the unchanged connection.
- Native upload/download picker cancellation and upload overwrite Cancel
  queued no transfers. Dirty-editor Escape preserved the draft; explicit
  discard closed the editor without changing remote bytes.

Frontend tests, TypeScript, ESLint, production frontend build and the locked
native app build passed. The full local `cargo xtask check` reached the VNC
fixtures but did **not** pass: the initial run had six failures, a serial
targeted rerun had five, and a full `RUST_TEST_THREADS=1` rerun after the app
build had the same five. They reported missing framebuffer events or an
unexpected reconnect while waiting for a resize diagnostic. VNC source was
unchanged in this correction; the cause remains under investigation. Do not
attribute these failures solely to contention or describe this candidate's
global check as green. These observations do not establish other OS WebViews,
recursive transfers or cancellation of an already running transfer.

Subsequent VNC work identified the fixture deadline failure and restored the
ordinary local global check. The later binary-frame correction also passed
that check with Full-HD VNC fixtures; see the [codec receipt](../../benchmarks/2026-10-02-framebuffer-ipc.md).
These later results supersede the validation blocker above without expanding
the native file-dialogue acceptance scope. Mac packaging was subsequently
updated in v0.1.18; Windows/Linux installers remain older.

### Repeat the remote-file reconnect check

1. Use a separate portable app, generated keys/trust, loopback SSH and an owned
   TCP relay. Prepare a fresh marker path and a file with known original bytes.
2. Open New folder and enter the marker path, leaving Continue pending. Shut
   down only the relay's established sockets, keeping its listener available.
   Wait for both a new accepted transport and an app connection-success event.
3. Accept the old path. Assert that the marker is absent and a connection-change
   notice appears. Reopen the action and verify a fresh approval succeeds.
4. Repeat across upload destination, download overwrite, delete confirmation
   and editor Save as. Assert no new transfer/file or changed original bytes.
5. Exercise an unchanged connection with explicit upload/download Create only,
   existing-target collision and Replace choices; compare exact file bytes.

For the filenames used in this receipt, the final filesystem regression check
is runnable with the standard library after completing the native actions:

```python
from pathlib import Path
import stat

home = Path("<fixture-home>")
remote = home / "workshop"
for name in ("stale-after-fix", "stale-upload.txt", "stale-rename.txt", "stale-editor.txt"):
    assert not (remote / name).exists(), name
assert not (home / "stale-download.csv").exists()
original = remote / "delete-proof.txt"
assert original.read_bytes() == b"keep this remote file\n"
assert stat.S_IMODE(original.stat().st_mode) == 0o644
assert (remote / "collision-upload.txt").read_bytes() == (home / "upload-fixture.txt").read_bytes()
assert (home / "fresh-download.csv").read_bytes() == (remote / "report.csv").read_bytes()
assert (remote / "fresh-editor.txt").read_bytes() == (remote / "app.conf").read_bytes()
```

These are checks for connection changes observed before native dispatch. They
do not roll back a job already dispatched or establish atomic transactions over
a disconnect occurring after dispatch. Keep in-flight transfer cancellation
and recovery checks separate.

## Settings error visibility and normal shutdown — 2026-10-02

Settings save/reset/import/export, diagnostic export and portable-vault errors
previously went to the workspace error banner behind the open settings form.
They now use a settings-specific alert near its action buttons. An error
receives focus so scrolling cannot hide it; a new operation or reopening the
form clears the previous settings error without importing unrelated SSH errors.
The first native inspection showed why placement matters: an alert above the
fields was outside the viewport after using the footer's Import button.

A rebuilt macOS ARM64 debug bundle with this correction after `c40d349` used a
copied portable app, disposable HOME/ZDOTDIR and generated local settings.
The actual app PID environment was checked before and after automation selected
it. These GUI checks started no protocol server or network listener:

- Entering multiline settings JSON and pressing Escape kept the original dark
  theme/font 14; the persisted file matched its baseline byte for byte.
- Malformed JSON was rejected with a visible, focused error in dark mode.
  The persisted file still matched that original baseline.
- Valid multiline JSON imported light mode/font 18. The form closed, its old
  error disappeared, and reopening displayed the imported values. Persisted
  values matched; the session store remained byte-identical and no vault was
  created.
- Importing font size 99 was rejected. The error was visible and focused in
  light mode; the persisted file remained byte-identical to the valid import.
- Closing after that error and reopening through Terminal options cleared the
  previous error and retained the imported preferences.
- Setting Quick connect to `Mod+N`, already used by New terminal, then Save
  produced a visible, focused collision error. The rejected draft remained in
  the form; persisted settings and session bytes were unchanged.
- After cancelling the draft, the native application's Quit menu exited with
  status 0. The recorded app PID and its active local zsh PTY child both exited.
  The same menu cleanup had passed on the preceding candidate as well.

The store regression now checks malformed JSON, unknown root/nested fields,
unsupported schema and out-of-range settings against both in-memory state and
exact persisted bytes. The full local `cargo xtask check` and final
`cargo xtask package-check` passed. This verifies a debug Mac bundle with the
production frontend; it is not a new release installer or Windows/Linux GUI
result. Native Quit with SSH, helpers or an in-flight transfer remains a
separate lifecycle gate. General focus return after Cancel is not established
by these error-focus checks.

## Remaining native acceptance gate

Use a separate portable app and disposable generated fixtures, as in
[the native runbook](native-workflow.md). Still verify:

- Upload/download picker → destination → all overwrite choices, including Cancel.
- Repeat settings JSON import, rejection and error-focus checks on Windows/Linux;
  macOS observations do not establish other WebViews.
- Native broadcast destination changes while approval is open, macro emergency
  stops, and retry exhaustion/server restart while approval is pending.
- Remote dirty-editor discard and credential deletion, then focus return and
  light/dark rendering across supported WebViews.

Stop the SSH session while a path dialogue is open and verify no file action is
performed after acceptance. These remaining workflows are not marked complete
by unit tests, a successful build or the narrower observations above.
