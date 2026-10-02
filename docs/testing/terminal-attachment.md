# Terminal output attachment

## Startup replay ordering on main after v0.1.19

SSH, Telnet and serial can receive text before the renderer attaches. Previously,
native attachment enabled live events and returned the retained startup chunks
in a separate IPC response. A newer event could reach xterm before those older
chunks, reversing text or splitting an ANSI sequence in the wrong order.

Attachment now emits retained chunks through the existing protocol output event
while holding the session lock, then enables live publication. The renderer
registers its listener and assigns the native terminal ID before invoking
attachment, and consumes all output through that listener. Attachment returns
only completion, so there is no second renderer replay path. Local PTY startup
already waits for its attachment signal and keeps that path.

The existing pre-attachment chunk bounds and eviction policy are unchanged;
this fixes the order of retained text, not unlimited startup retention or native
rendering throughput. No additional frontend queue or dependency was added.
The replay callback is synchronous and must not re-enter the session manager.

## Regression scope

Each production remote manager has an
`attach_replays_pending_output_before_live_output` regression. It supplies
fragmented ANSI and Unicode startup chunks, verifies exact emission order and
that the session lock excludes live publishers throughout replay, then checks
that the backlog is empty, attachment is enabled, repeated attachment does not
replay it and a missing session cannot emit.

```sh
cargo test -p mobarust attach_replays_pending_output_before_live_output
```

On macOS ARM64 on 2026-10-02, all three regressions and the full local
`cargo xtask check` passed, including native workspace tests/Clippy, frontend
checks, release-asset contracts, isolated helper fixtures, package-layout
contracts and fuzz compilation. The OpenSSH IPv6 case ran; the optional real
X11-server fixture was skipped because Xvfb or its safe system socket directory
was unavailable.
`cargo xtask package-check` also rebuilt the matching unsigned Mac debug
bundle and verified its native executable, VNC helper layout and checksum
manifest. Its `--version` process launches were CLI checks, not GUI startup or
rendering evidence.

The [two-session Mac debug authentication check](ssh-lab.md#native-overlapping-reconnects-and-shutdown--2026-10-02)
also exercised the matching frontend/native attachment API: both SSH terminals
displayed the startup fixture banner before their typed markers, including after
controlled reconnects. That observation does not establish the timing race under
sustained startup output, or native Telnet/serial acceptance.

The manager regressions do not establish GUI timing on every OS,
physical serial-device acceptance, or sustained output responsiveness. A native
follow-up should attach while a controlled server/device continues output and
verify the complete visible sequence. Published v0.1.19 Mac installers and
v0.1.12 Windows/Linux installers do not contain this source correction.
