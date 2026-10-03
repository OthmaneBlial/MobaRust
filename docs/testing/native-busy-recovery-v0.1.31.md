# v0.1.31 ARM64 busy-session recovery — 2026-10-03

A disposable copy of the verified v0.1.31 ARM64 installer, runtime source/tag
`d9910c815508fcf51e074c89fcb4a1353220a05d`, exercised native directory-listing
and monitoring requests at the shared 32-worker limit. Its source executable
SHA-256 was `0a4e93f9e0cc23909d3de240c73a5416c47c9944c58d2c873abdf62d550a7e3a`
before the private bundle/environment guard and ad hoc re-signing.
This is published-artifact evidence, not acceptance of a later source fix.

## Observations

- Native controls started 32 local forwards to a loopback target. The app
  retained all 32 Stop controls and a badge of 32. Independent process/socket
  inspection matched 32 actual `127.0.0.1` listeners.
- Opening Files at capacity returned `invalid SSH request: SSH session is busy;
  wait for an operation to finish, then retry explicitly`. The listing showed
  `Unable to list directory`; Refresh remained enabled. File mutations and
  upload controls were disabled while the listing was unavailable.
- Opening Monitor at capacity returned the same busy refusal with a usable
  Refresh snapshot control. SSH remained connected.
- Native Stop on the oldest tunnel freed one worker; its port rebound and
  31 tunnels remained live. Reopening Files loaded 11 entries. A subsequent
  explicit Refresh also succeeded, but the old busy error remained visible.
- Explicit Refresh snapshot recovered monitoring after the slot was freed.
  The directory error also remained in the global error banner on Monitor.

These observations reproduce a **stale directory error** in v0.1.31. Backend
capacity recovery works; the displayed listing/global error does not recover
with it. Raw screenshots containing local account labels and generated-key
filenames were reviewed locally and are not public marketing assets.

## Isolation and cleanup

The actual private HOME/ZDOTDIR/XDG paths and empty SSH-agent settings were
verified before connecting and again before cleanup. Only generated key/trust
references were imported. The existing `native_file_editor_lab` supplied a
separate generated-key OpenSSH daemon; its config and actual listener were
checked as loopback-only. No personal SSH configuration, keys or agent were used.

Window observation failed before the planned Quit check. The process was
independently confirmed live; a Quit shortcut attempt did not end it. This run
therefore **does not prove normal Quit**. Only the owned app was terminated
with SIGTERM. Its local PTY and sshd session children ended; all 31 remaining
listener ports rebound before the fixture stop marker was created.

The fixture harness then passed in 337.81 seconds (337.83 with its wrapper),
removed its private HOME/generated keys/trust/files, ended its daemon and
released its port. No Remote Login, firewall/router or public listener changes
were made. The separate [counter acceptance](native-tunnels-v0.1.31.md) retains
its own previously verified normal-Quit result.

## Source correction and remaining gates

The subsequent source correction keeps directory errors in listing state,
clears that state when a new request starts, and displays it only for the
active listing session. It leaves unrelated global errors intact. Existing
latest-request/session fences continue to reject stale replies. This avoids
both a stale Files alert after successful Refresh and a listing error leaking
into Monitor, terminals or another session's Files view.

The corrected source passed frontend type checking and the full local
`cargo xtask check` in 191.96 seconds on macOS ARM64, including frontend unit
tests/type checking/lint/build, 114 desktop tests, workspace tests/Clippy,
loopback protocol/helper fixtures, package contracts and fuzz compilation.
The optional real Xvfb test retained its missing-prerequisite skip. No extra
state helper, dependency or backend admission change was added.

Native acceptance of the corrected candidate remains pending. This run does
not exercise transfer Retry, all finite mutations, queue saturation, sustained
workloads, many sessions, Intel GUI or Windows/Linux. The 57/76 roadmap
checklist remains unchanged.
