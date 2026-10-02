# Text-input dialogue checks — 2026-10-02

This change is on `main` after v0.1.17. Published v0.1.17 installers do not
contain it yet. The integrated tunnel form in that release is separate.

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

## Remaining native acceptance gate

Use a separate portable app and disposable generated fixtures, as in
[the native runbook](native-workflow.md). Still verify:

- Upload/download picker → destination → all overwrite choices, including Cancel.
- Malformed/valid multiline settings JSON imports; cancellation must
  leave persisted state unchanged.
- Native broadcast destination changes while approval is open, macro emergency
  stops, and retry exhaustion/server restart while approval is pending.
- Remote dirty-editor discard and credential deletion, then focus return and
  light/dark rendering across supported WebViews.

Stop the SSH session while a path dialogue is open and verify no file action is
performed after acceptance. These remaining workflows are not marked complete
by unit tests, a successful build or the narrower observations above.
