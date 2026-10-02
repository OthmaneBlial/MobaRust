# Native remote desktop renderer — 2026-10-02

## What changed

The helper pipe already carried bounded binary RGBA, but the parent converted
pixels back into JSON numbers for the global WebView event bus. Each render also
reset canvas dimensions and copied the number array. The desktop now sends
control events and frame-ready metadata on a per-connection Tauri channel and
returns the latest validated binary frame through a raw invoke response.

The parent cache holds one pending frame and sends at most one outstanding
frame-ready notice. A pull includes the connection generation; stale pulls do
not consume the current frame. The frontend coalesces notices behind one fetch
and one animation callback, rejects malformed binary headers/geometry, ignores
replies after reconnect/close, and creates a typed view over the returned bytes.
This removes JSON pixel serialization and the JavaScript number-array copy;
it does not eliminate native decoding, IPC transport or canvas copies.

## Runnable checks

```sh
pnpm --dir apps/desktop test:unit
cargo test -p mobarust --locked pending_framebuffer_keeps_only_latest
cargo xtask check
```

The frontend check covers Full-HD header/pixel bounds, the shared ArrayBuffer,
10,000 notices before and during a fetch, stale reconnect success/failure,
bootstrap refresh, disposal, invalid generations and malformed replies. The
Rust check verifies latest-frame replacement and old-generation pull rejection.

The complete local `cargo xtask check` passed after this change: workspace
tests/Clippy, frontend unit/type/lint/build checks, release-asset tests, isolated
RDP checks with three local process cases, 17 Full-HD VNC process cases, package
layout checks and fuzz-target compilation. The pre-push payload audit passed.
These are local checks; the disabled GitHub workflows did not run.

## Controlled native observation

Machine: Apple M2, 8 CPU threads, 16 GiB RAM, macOS 26.6 (25G72), Rust 1.95.0.
A debug native app was built with the current production frontend:

```sh
pnpm --dir apps/desktop build
pnpm --dir apps/desktop tauri build --debug --bundles app \
  --config '{"build":{"beforeBuildCommand":""}}'
```

A copied, ad-hoc-signed app bundle used a portable marker, generated profile,
disposable HOME/XDG directories and no SSH-agent environment. The test profile
used VNC with no authentication on `127.0.0.1`, clipboard opt-in, balanced quality,
1920×1080 and three reconnect attempts. It accessed no personal SSH keys,
profiles or credentials. RDP remained excluded from the normal bundle.

Observed in the actual native app using accessibility actions and screenshots:

- Full-HD two-colour fixture pixels reached the canvas; the overlay reported
  1920×1080, clipboard availability, local scaling and unencrypted transport.
- Keyboard `r` reached the fixture (keysym 114) and changed the displayed palette.
  Pointer/wheel events also reached the fixture during the native session.
- A forced fixture disconnect led to a second Full-HD frame and connected UI.
  Another `r` reached the reconnected fixture and changed its displayed palette.
- Closing the VNC tab disconnected its helper and returned to the local terminal.
- Fullscreen was blocked by the desktop runtime and displayed an error. It is
  **not** accepted as working in that Tauri 2.11.5 observation; see the subsequent
  runtime recheck below.

The fixture reported 1,268 frames on the first connection before the recorded
keyboard check, and 39 on the second before the post-reconnect key. These are
server-send counters, **not** rendered-frame counts or an FPS measurement.

## Fullscreen recheck with Tauri 2.12.1

The previous runtime compiled Wry's macOS element-fullscreen preference behind
an opt-in feature. Tauri 2.12.1 / Wry 0.57.0 enable it without that opt-in;
macOS 12.3+ uses WebKit's public `setElementFullscreenEnabled` API. The
[upstream correction](https://github.com/tauri-apps/tauri/commit/c9a3cb892e901e39ca46aad2ff6b14aac21fea0d)
and installed source identify the runtime boundary. The app's existing
`requestFullscreen` / `exitFullscreen` handlers required no rewrite.

The frontend API, CLI and Rust runtime were aligned to 2.12.1. This runtime
requires Rust 1.90; contributor instructions and workspace metadata now match.
The debug app bundle and full local `cargo xtask check` passed on Rust 1.95.0.
Dependency metadata declares no resolved package MSRV above 1.90; compilation
on the minimum compiler itself was not repeated in this check. The refreshed
workspace audit has no reported vulnerability and two transitive warnings.

A newly copied portable app used a generated profile and the same loopback-only
fixture, on ephemeral port 64429. An initial launcher attempt lost its temporary
HOME and was stopped before connecting. The accepted run used a live foreground
process with HOME/ZDOTDIR and XDG paths pointing to the disposable directory;
the actual native PID's HOME/ZDOTDIR were checked before and after selecting it
for UI automation. SSH-agent variables were removed. A copied bundle's
`LSEnvironment` also recorded the disposable environment as a launch safeguard.
Do not assume a detached process's environment survives a macOS app relaunch.

Observed in the native app, without injecting page JavaScript:

- Enter fullscreen replaced the workspace with the aspect-preserving Full-HD
  canvas and an **Exit fullscreen** button, without a blocked-runtime notice.
- Focusing the canvas and pressing `r` reached the fixture as keysym 114 and
  changed the visible blue/orange palette to green/purple.
- Exit fullscreen returned to the normal workspace. Focusing the canvas and
  pressing `r` again reached the fixture and restored the blue/orange palette.
- A second fullscreen entry followed by Escape returned to the normal window.
  The helper remained connected and continued to report 1920×1080 throughout;
  VNC server-side resize is still not claimed.
- Closing the VNC tab disconnected the helper. The native test process and
  fixture were terminated after the check, and port 64429 refused connections.

This establishes macOS ARM64 debug fullscreen behavior against the controlled
fixture. Windows/Linux fullscreen, release installers, external VNC servers and
long-session rendering metrics remain unverified. No RDP production gate changed.

## v0.1.18 release-bundle recheck

On the same Apple M2/macOS machine, both v0.1.18 DMGs passed disk-image checks,
read-only package layout, strict ad hoc signatures, app/helper architecture,
CLI version and SHA-256 verification. The native ARM64 test copied its app from
the verified DMG; only that disposable copy received the portable marker,
generated profile, isolated HOME/ZDOTDIR/XDG launch environment and a unique
bundle identity. Its actual PID environment was checked before and after CUA
selection. Clipboard was disabled; no personal profiles, keys or agents were used.

The loopback-only fixture used ephemeral port 51705. Observed in the real app:

- The binary frame rendered the two-colour 1920×1080 canvas.
- Focused keyboard `r` reached the fixture as keysym 114 and changed the palette.
- An owned-fixture SIGUSR1 disconnect produced a second Full-HD frame and a
  connected workspace. Another `r` reached that connection and changed pixels.
- Enter fullscreen exposed a WebKit fullscreen accessibility surface containing
  Exit fullscreen and 1920×1080. CUA window selection returned to the original
  workspace; fullscreen input and explicit exit are therefore not accepted by
  this release run. The debug receipt above remains a separate result.
- Settings rejected duplicate Mod+N shortcuts, focused the visible inline error
  and preserved persisted settings byte for byte.
- Native menu Quit exited the app with code zero and released both its active
  local zsh PTY and VNC helper. The fixture observed disconnection, was stopped,
  and a connection probe confirmed port 51705 was closed. Owned mounts were detached.

Fixture counters at the two keyboard checks were 1,127 and 32 server sends,
not rendered-frame counts or a sustained-performance benchmark. This is a
short portable-copy release-bundle test, not clean installation, Intel GUI,
external-server or Windows/Linux evidence. Both Mac app/helper cohorts contain
wire version 2; older v0.1.17 binaries do not.

## Reproduce without exposing a listener

```sh
python3 tools/local-vnc-renderer-fixture.py
```

The stdlib-only fixture binds **only `127.0.0.1:0`** and prints its ephemeral port.
Use that port in a disposable native VNC profile. It offers a controlled RFB 3.8
no-auth handshake and two Full-HD colour panels; it exposes no desktop, shell or
filesystem. Frames are paced at a maximum of five sends per second. Press a key
or click the canvas to alternate palettes. Send SIGUSR1 to the fixture's own
PID to disconnect its client for reconnect QA; stop it with Ctrl+C or SIGTERM.
No wildcard/LAN listener, router/firewall change, macOS Remote Login or public
tunnel is required. Keep this fixture local.

The recorded lab port was 63094. Its app, helper and fixture were stopped;
a connection probe confirmed the port was closed after cleanup.

## Remaining acceptance gates

The debug and ARM64 release-copy receipts use a controlled RFB fixture.
They do not establish external VNC-server compatibility, Windows/Linux behavior
or clean installation. No sustained rendering throughput or end-to-end input latency was
measured. Record those in release builds under a defined workload, verify
fullscreen on other platforms, exercise longer sessions, and rebuild matching
wire-version-2 app/helper installers for Windows/Linux. The v0.1.18 Mac
cohort contains this renderer change; older v0.1.17 installers do not.
GitHub workflows stay disabled.
