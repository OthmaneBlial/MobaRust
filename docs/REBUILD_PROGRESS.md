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

## M3b — MobaXterm import from Windows exports (2026-11-19)

- Format checked against a public community study of `.mxtsessions` (MobaXterm 26.3). It showed that `109` is the user-editable icon, not the type. The type is now read from the first `%` field (0 = SSH, 7 = SFTP), and the `;  logout` prefix is accepted. Commits `9b3a94e` and `23416be`.
- Windows files decode on any OS: Windows-1252 (ANSI), UTF-8 with or without BOM, and UTF-16 with BOM. CRLF line endings and lowercase section names are accepted.
- Now imported: SFTP bookmarks, the login command (MobaXterm escapes decoded), the comment (into notes), SSH gateways as jump hosts, and key paths mapped to `~/.ssh/<file>`. The agent is kept when that file is missing, and `.ppk` keys get a puttygen hint. `<default>` login is treated as no username. RDP and VNC are named in the skipped list. `.moba` single-session files are accepted.
- Verified: store unit tests on a CRLF Windows-1252 fixture and its UTF-16LE/BE copies. A real-sshd test (`windows_mobaxterm_export_connects_through_its_gateway_to_local_sshd`) imports a Windows export and connects through a local bastion to a local target with the mapped key. It also proves the login command ran.
- Not verified: a file written by MobaXterm itself, and clicking through the native import dialog.

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

## M6 — Brand, README and site (2026-11-19)

- New original mark: graphite tile, aluminium prompt chevron, orange cursor and a row of status lights. `icon.svg` is the source. `icon.png` is now 1024 px (the release workflow derives every size from it) and `icon.ico` is multi-size. The same mark is used in the app top bar and as the favicon. Rasterized with macOS `qlmanage` and PIL; nothing was installed.
- README rewritten around the 2.0 workflow, with new screenshots. They are captured from the browser preview of `main` with the installed Chrome over CDP and labelled as such. The README states that the downloads predate these changes.
- `site/` redesigned in the Instrument Panel style. Docs share the stylesheet. Checked at 1440 and 390 px wide, with no horizontal scroll. Not deployed: Pages is served from another repository, and publishing needs approval.
- Fixes found from full-resolution captures: hover-only row actions so session names fit, the search shortcut no longer wraps, compact error banners, group-run spacing, and the light search field.

## M7 — Performance pass (2026-11-19)

- Measured bundle size, load, heap and session-tree scale in headless Chrome. Receipt: `benchmarks/2026-11-19-ui-scale.md`.
- Found that every app render rebuilt the whole session tree. At 5,000 sessions a tab click took 284 ms. With the tree memoized and folder counts computed in one pass, it now takes 6 ms, and first paint dropped from 952 to 520 ms.
- The Rust synthetic harness shows no regression against the October receipt.

## M8/M11 — Packaging, native receipt and report (2026-11-19)

- The full localhost protocol lab is green after the import work: 14 sessions connected in 558 ms, and no fixture `sshd` was left running.
- Built a local release DMG with the release config: 8.64 MiB, verified, arm64, ad hoc signed. The native app opens its window in 370 ms warm and 1.49 s on first launch. It idles at 0.1–0.2 % CPU and exits about 100 ms after SIGTERM, with no WebKit helpers left. Receipt: `benchmarks/2026-11-19-native-macos.md`.
- Final report: `docs/REBUILD_2_0_REPORT.md`.

## Website (2026-11-19)

- Rebuilt `site/` as the "freight manifest" world (see `docs/WEBSITE_DESIGN.md`), checked at 1440 and 390 px, and published to https://othmaneblial.github.io/MobaRust/ with the user's approval.

## Next steps

1. Native click-through and screenshots, once the testing tool has Screen Recording and Accessibility access.
2. Tagged release and Linux/Windows installers through the existing workflow, when authorized.
3. A Tabby UX comparison, if Tabby gets installed.
