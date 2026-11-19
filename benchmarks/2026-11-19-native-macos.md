# Native macOS receipt — 2026-11-19

Machine: Apple M2, macOS 26.6. Build: `pnpm tauri build --bundles dmg --config src-tauri/tauri.release.conf.json -- --locked` at commit `4fffdb2` (version string still 0.1.31). The app was run from a copy of the DMG's `MobaRust.app` and was not installed.

## Artifact

| Item | Value |
| --- | --- |
| DMG | `MobaRust_0.1.31_aarch64.dmg`, 8.64 MiB, `hdiutil verify` VALID |
| SHA-256 | `d74780302fe2dad56c9c3caffc1d9828a2aa45d9293257dc90a5f40ea3b56a76` |
| Bundle | arm64 thin Mach-O, ad hoc signature, not notarized; contains LICENSE, NOTICE and helpers |
| Note | Local DMG packaging needs `CI=true`. Without it, Tauri's `bundle_dmg.sh` runs a Finder AppleScript that this session may not automate. GitHub runners set `CI` already. |

## Measurements

The time to the window is measured from process spawn until CoreGraphics reports an on-screen window wider than 400 px owned by that process. Polling runs every 10 ms.

| Metric | Result |
| --- | --- |
| First launch (fresh copy, includes first-run code checks) | 1,489 ms |
| Warm launches | 371, 370, 372 ms |
| Idle CPU, main process (ps / top, after about 30 s) | 0.1–0.2 % |
| Idle memory, main process | 105 MB RSS; 31 MB physical footprint (top MEM) |
| WebKit helpers (WebContent, Networking, GPU) | 43 + 16 + 78 MB RSS |
| SIGTERM to exit | about 100 ms; no WebKit helper left running |

## Not measured here

- Screenshots of the native window. `screencapture -l` returned "could not create image from window" because this terminal has no Screen Recording permission, and changing security settings is out of scope. UI captures come from the browser preview of the same React build (see `2026-11-19-ui-scale.md`).
- Terminal input latency and resize smoothness in the native window, because no input automation was available.
