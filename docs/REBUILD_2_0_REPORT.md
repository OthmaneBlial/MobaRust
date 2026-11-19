# MobaRust 2.0 rebuild report (2026-11-19)

Scope: the MobaRust 2.0 goal (terminal-first workspace, MobaXterm import, one-click groups, redesign, hardening, performance, packaging, brand). Everything ran on one Mac (Apple M2, macOS 26.6) with no new tools, servers, VMs or containers. Each item has an evidence column: "Real local" means real protocol traffic on loopback. "Browser preview" means the real React UI without the native runtime. Evidence per area is in [`TEST_MATRIX.md`](TEST_MATRIX.md).

## Implemented and tested

| Area | Evidence | Commits |
| --- | --- | --- |
| Terminal-first workspace (resizable or collapsible sidebar, focus mode, tab menu, reliable close) | Browser preview; desktop manager tests (117 passed) | `78e72c9` |
| MobaXterm import: `.mxtsessions`, `.moba`, `MobaXterm.ini`, Windows exports on any OS (Windows-1252, UTF-8, UTF-16, CRLF). SSH and SFTP bookmarks with folders, host, port, user, login command, SSH gateways and key file names. Preview with add, replace or skip, then one atomic write | Unit and fixture tests. Real local: an imported Windows export connects through its gateway to local `sshd` and runs its login command | `78e72c9`, `9b3a94e`, `23416be`, `4fffdb2` |
| One-click groups: nested folders, open all or selected, 4 connections at a time, a status light per host, cancel and retry, pinned groups, opt-in tab restore, `--open-group` | Real local: 14 concurrent SSH sessions connected in 558 ms. Browser preview for the picker, pins and restore | `78e72c9`, `dc7837a`, `8d788cf` |
| Instrument Panel redesign: tokens, dark and light themes, lamps, group hues, focus, contrast | Browser preview: scripted contrast audit of every view and dialog in both themes (4.5:1 minimum, 10.5 px minimum) | `4327650`, `dc7837a` |
| SSH, SFTP, SCP, tunnels, jump hosts, X11, reconnect, remote editing | Real local: `./tools/local-protocol-lab.sh` all green, no fixture `sshd` left | `b183dd7` and earlier |
| Performance: memoized and virtualized session tree, deferred search | Browser preview: tab switch 6 ms at 5,000 sessions. The project target is hundreds of sessions | `4794419`, `1d5dd8f` |
| Brand: original mark, icons, README | README screenshots labelled as browser preview | `a4bc954` |
| Website: "freight manifest" redesign, SEO meta, OG card, sitemap | Rendered at 1440 and 390 px; live at https://othmaneblial.github.io/MobaRust/ (all pages and assets return 200) | `9fb3070` |
| macOS packaging | Local release DMG (8.64 MiB, verified, arm64, ad hoc signed). Native window in 370 ms warm; idle CPU 0.1–0.2 %; clean SIGTERM exit. See [`benchmarks/2026-11-19-native-macos.md`](../benchmarks/2026-11-19-native-macos.md) | this report |

## Partially tested

- **Native GUI flows.** The native release app launches, idles and quits cleanly, as measured. Clicking through the native window and taking native screenshots were not possible in this session: there is no Screen Recording or input-automation access, and granting it is a security setting the user must change. This affects the import dialog, Open all, tab restore and pins, which were verified in the browser preview and by Rust tests instead.
- **MobaXterm field meanings.** They follow a public community study of the format (MobaXterm 26.3), not vendor documentation. No file written by MobaXterm itself was available on this Mac.
- **Telnet, VNC.** These were tested against fixture servers written for the tests, not real daemons. **RDP** has helper unit tests only.

## Not imported from MobaXterm (by design or unverified)

Passwords (credential sections are never read), X11 options, proxy settings, terminal, font and colour settings, and RDP, VNC, Telnet, serial and other bookmark types (listed as skipped). PuTTY `.ppk` keys are not converted; the session notes give the `puttygen` command.

## Blocked or awaiting the user

- **Native GUI testing.** Re-checked on 2026-11-19: `screencapture` returns a black frame and System Events times out, so Screen Recording and Accessibility are still not granted. Changing those settings is the user's step.
- **Linux and Windows builds.** The Quality workflow is disabled on GitHub at the maintainer's request, and the Release workflow runs only on tags or a manual dispatch. No remote CI or release was started. Windows was not run on this Mac, by rule.
- **Public release.** Not created. The downloads in the README are still 0.1.31 (macOS) and 0.1.12 (Windows, Linux), and predate this work.
- **Tabby comparison.** Tabby was not installed at the time of this report.

## Remaining work

1. Native click-through of the import dialog, Open all, restore and pins once Screen Recording and Accessibility access is granted to the testing tool.
2. Version bump and a tagged release when authorized, to produce Linux and Windows installers through the existing workflow.
