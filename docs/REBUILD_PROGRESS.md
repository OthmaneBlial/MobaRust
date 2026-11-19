# MobaRust 2.0 rebuild progress

Working log for the MobaRust 2.0 effort (MobaXterm-class SSH suite, macOS first).
Each entry lists what changed, how it was verified, and what is still open.
"Verified" means the check below was actually run; anything else is marked.

## M0 — Baseline audit (2026-11-19)

- Repository: Tauri 2 + React 19 + xterm.js desktop app, Rust crates for core,
  SSH (vendored russh), SFTP/SCP, Telnet, serial, network diagnostics, vault,
  store and RDP/VNC helpers. 683 commits, v0.1.31.
- Baseline checks on this Mac (macOS 26, Apple Silicon):
  - `pnpm check`, `pnpm lint`, `pnpm test:unit`: pass.
  - `cargo test` for core, store, ssh, telnet, serial, network: pass,
    including 26 tests against a throwaway loopback `/usr/sbin/sshd`.
- Main user pain confirmed in the real UI: the terminal was capped at
  `min(53vh, 505px)` inside a padded, scrolling page with a large heading,
  context strip, info cards, a fixed right rail and a footer. At 1440×900 the
  terminal used roughly half the window.
- Architecture notes: `apps/desktop/src/App.tsx` is ~6k lines and holds most UI
  state; new UI logic goes into small tested modules next to it rather than a
  rewrite. Session folders were flat strings with no nesting, no "open all",
  no MobaXterm importer.

## M1 — Terminal-first workspace (2026-11-19)

Changed:
- Full-height flex layout: the terminal card now fills the window. The big
  heading, context strip, info cards and footer were removed; version moved to
  the terminal status bar; topbar reduced to 44 px with Quick connect /
  Commands / Macros.
- Session sidebar width is draggable (200–480 px, keyboard arrows, double-click
  reset) and remembered; collapse is unchanged (`Mod+Shift+B`).
- Session-details rail is hidden by default and toggled from the terminal
  toolbar (remembered).
- Focus mode (`Mod+Shift+Enter` or toolbar button) hides topbar, sidebar, view
  tabs and rail so only tabs, terminal and status bar remain.
- Tabs: middle-click close, right-click menu (Close, Close other tabs,
  Reconnect / restart, Reopen closed tab), `Mod+Shift+T` reopens the last of up
  to 10 closed SSH/local tabs.

Verified:
- Browser-preview rendering of the real React UI at 1440×900 and at the
  window minimum 1100×680: terminal fills the content area; focus mode and the
  tab menu render and work (keyboard shortcut and Escape).
- `pnpm check`, `pnpm lint`, `pnpm test:unit` (new `workspace-layout` test).

Not yet verified: native macOS window screenshots. The shell running this work
has no Screen Recording permission, so `screencapture` fails; the native debug
app was built and launched but not visually inspected. xterm fit/PTY resize on
these new containers relies on the existing ResizeObserver path and still needs
a native check.

## M3 — MobaXterm import, partial (2026-11-19)

- New `mobarust-store::mobaxterm` parser for `[Bookmarks]` / `[Bookmarks_N]`
  sections of `.mxtsessions` exports and `MobaXterm.ini`.
- Imports SSH bookmarks (type code 109) with name, nested folder (`SubRep`),
  host, port and username. Everything else is reported, not guessed. Password
  and other sections are never parsed. UTF-8 (with BOM) and Latin-1 input.
- Preview → per-row Add / Replace / Skip, bulk duplicate decisions, then one
  atomic write. A failed or invalid batch leaves the store unchanged; Replace
  keeps the existing session's id, favorite and last-used time.
- Desktop: "Import MobaXterm sessions" button in the session list opens a
  native file picker (webview `<input type=file>`, no new dependency), preview
  table, warnings and skipped list.

Verified: fixture-based unit tests (nested folders, unsupported types, invalid
port, missing host, duplicate detection, credential section never serialized,
garbage and Latin-1 input) and a store test for add/replace/skip and rollback.
Not yet verified: the dialog against a real MobaXterm export (none available
on this Mac) and native UI inspection.

## M4 — One-click session groups (2026-11-19)

- Session folders render as a nested, collapsible tree (`A/B/C`).
- Each folder has "Open all" for every SSH session in its subtree, opened
  with at most 4 handshakes in flight, with per-session progress, cancel of
  queued sessions, and retry of failed/cancelled ones.
- `mobarust --open-group "My Servers"` (or `--open-group=Prod/Web`) opens a
  group at launch. Resolution: exact path, then case-insensitive path, then a
  unique last segment; ambiguous names show every match instead of guessing.
  Only a group name is accepted on the command line.

Verified:
- `workspace-layout` unit test: bounded runner never exceeds 4 in flight for
  14 items; cancellation; group resolution including ambiguity.
- Rust test for `--open-group` argument parsing.
- Real local SSH: `fourteen_session_group_opens_with_bounded_handshakes_and_cleans_up`
  opens 14 sessions to a throwaway loopback `sshd` with the same bound of 4,
  then runs, resizes and exits all 14 PTYs concurrently. Measured on this Mac:
  all connected in 209 ms (slowest handshake 63 ms), all shells done in
  496 ms; the daemon accepted a fresh session afterwards; no fixture `sshd`
  processes remained.

Not yet verified: the "Open all" button driving the native app end-to-end, and
`--open-group` in a running native build (a second launch on macOS starts a new
instance; no single-instance forwarding yet).

## M2 — Instrument Panel redesign (2026-11-19)

- Picked the direction with the impeccable workflow (PRODUCT.md, decision sketches, user choice: "Instrument Panel"). Documented in `DESIGN.md`.
- Every hard-coded colour now comes from a token. Dark and light have their own graphite and aluminium ramps, and new xterm palettes match them.
- Lamp states on tabs, the tree and the group-open meter. Group hues from `groupHue()` (unit-tested). Aluminium top strip. Removed the decorative "operations deck" and "LOCAL CORE ONLINE" copy.
- Sans labels, larger minimum text (10.5px), orange focus rings, themed selection and scrollbars.
- Checked: `pnpm check`, `pnpm lint`, `pnpm test:unit`, `impeccable detect` (0 findings). Browser preview at 1440×900 in dark and 1100×680 in light.
- Not checked: native screenshots, which are blocked because the shell lacks Screen Recording permission.

## M4b — Open selected, tab restore, contrast pass (2026-11-19)

- Group picker next to "Open all": tick sessions and open them, 4 at a time, with the same progress, cancel and retry.
- Opt-in "Restore tabs on next launch" in the tab menu. It stores only saved-session ids and plain local shells, never hosts or credentials. Saved sessions reopen through the normal path, so startup-command confirmation still applies.
- The browser preview now has sample saved sessions, including a 6-host Production/Web group, so group features can be exercised there.
- Scripted contrast and text-size audit across all views and dialogs in both themes. Fixed `--faint`, the light orange and green, 10px text and label hints.

## Next steps

1. Native UI inspection once Screen Recording / computer-use access is
   available; otherwise keep using browser-preview plus native logs.
2. Extend localhost integration coverage across protocols (Telnet fixture,
   serial PTY pairs, tunnels and remote editing already have sshd tests) and
   record results in `docs/TEST_MATRIX.md`.
3. Session explorer polish: multi-select "open selected", group favorites,
   workspace restore.
4. README, logo and site refresh with real screenshots.
