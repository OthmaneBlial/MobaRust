# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

(A Tauri desktop app rendering a web UI on macOS first, then Linux and Windows.)

## Users

Sysadmins and DevOps engineers who live in SSH all day across many servers: they open whole groups of hosts at once, run commands, move files over SFTP, and keep tunnels up. Many are former MobaXterm users who moved to macOS or Linux and lost their tool.

## Product Purpose

MobaRust is an open-source, MobaXterm-class remote administration suite: tabbed and split terminals, a session tree with nested groups, one-click group opening, SFTP/SCP, SSH tunnels, multi-exec, macros, Telnet, serial and experimental RDP/VNC. Success is a professional using it for hours without fighting wasted space, lost tabs or unsafe defaults.

## Positioning

Open, native and safe: open-source, cross-platform with macOS first, Rust-native transports, secrets kept in the OS vault and never in session files, and MobaXterm's breadth without Windows lock-in or clutter. It imports MobaXterm sessions so switching costs little.

## Operating Context

Long sessions on laptops and large monitors, often 10+ tabs, groups of 10–20 servers, frequent switching between terminal, files and tunnels, keyboard-heavy use, light and dark environments.

## Capabilities and Constraints

- Terminal (xterm.js) is the primary surface and must get most of the window.
- Session data is secret-free; credentials are references into the OS vault.
- Host keys are always verified; security must not be weakened for convenience.
- RDP/VNC and some protocols are experimental and must be labelled as such.
- MobaXterm import is partial (SSH host/port/user/folders); passwords are never imported.

## Brand Commitments

The name "MobaRust" is kept. Logo, colors and typography may be redesigned freely. Do not copy MobaXterm or Tabby branding, icons or assets.

## Evidence on Hand

Real test evidence in `docs/TEST_MATRIX.md` and `docs/REBUILD_PROGRESS.md`. No customer testimonials, usage numbers or benchmarks beyond those measured there; do not invent any.

## Product Principles

1. The terminal comes first; chrome earns its pixels.
2. Common actions are one click or one shortcut; advanced options are disclosed progressively.
3. Safety is visible but quiet: verified hosts, no stored secrets, explicit broadcast.
4. Honest labels: experimental and partial features say so.
5. Keyboard-first, with clear focus everywhere.
