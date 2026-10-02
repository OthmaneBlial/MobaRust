# Text-input dialogue checks — 2026-10-02

This change is on `main` after v0.1.17. Published v0.1.17 installers do not
contain it yet. The integrated tunnel form in that release is separate.

All 19 former `window.prompt` callers now use the shared HTML `dialog` helper:
profile naming, explicit OpenSSH config paths, settings/session JSON imports,
clipboard fallbacks, upload destinations, remote mkdir/rename/permissions and
the remote editor's Save as path. Existing backend validation and overwrite
confirmation policies remain in place. File actions stop if the active SSH
session changed while waiting for text. Cancellation returns no value; a second
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
Its minimal DOM boundary tests promise ownership; it does not simulate native
rendering, focus or a protocol connection.

Type checking, ESLint and the frontend unit suite passed. The macOS ARM64
production app build also passed with locked Rust dependencies.

## Browser observations

An ignored loopback page imported the actual production helper and stylesheet.
Chrome checks observed:

- A prefilled path field received focus and accepted Unicode via Enter.
- Escape returned cancellation and restored focus to the opening button.
- Multiline JSON remained multiline on acceptance.
- A read-only copy field did not change when a character was typed.
- Modal accessibility state excluded the background controls.
- Light mode used the workstation palette with visible field/button focus.

These checks do not prove the complete native SSH/SFTP workflows. A separate
portable app with generated loopback fixtures was started, but computer-use
control could not acquire a native window. No new native GUI success is claimed.

## Next native acceptance gate

Use a separate portable app and disposable generated SSH/HTTP fixtures, as in
[the existing native runbook](native-workflow.md). Verify upload destination,
mkdir, rename, permission validation, Save as to a new path and an existing
target, then malformed/valid JSON imports. Verify Cancel and Escape do not
change files or persisted state. Verify focus return and light/dark rendering.
Stop the SSH session while a path dialogue is open and verify no file action is
performed after acceptance. Keep personal keys, accounts and configuration out
of the fixture.

The remaining `window.confirm` callers require their own native compatibility
audit; this text-input change does not assert that all confirmation workflows
have been verified.
