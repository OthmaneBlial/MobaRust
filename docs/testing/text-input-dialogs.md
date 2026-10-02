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
observations do not establish Windows/Linux WebView compatibility or complete
native broadcast/macro coverage.

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
- Malformed/valid multiline settings and session JSON imports; cancellation must
  leave persisted state unchanged.
- Native broadcast destination changes and emergency Escape during paste approval.
- Per-action macro cancellation and connection loss/reconnect while approval is
  pending, including an SSH reconnect retaining the same native ID.
- Remote dirty-editor discard, credential deletion and local startup-command
  approval, then focus return and light/dark rendering across supported WebViews.

Stop the SSH session while a path dialogue is open and verify no file action is
performed after acceptance. These remaining workflows are not marked complete
by unit tests, a successful build or the narrower observations above.
