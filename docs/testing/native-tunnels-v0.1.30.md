# v0.1.30 ARM64 native tunnel acceptance — 2026-10-03

A disposable copy of the verified v0.1.30 ARM64 installer, runtime source/tag
`e92f8bd16b5135379ca1d5005041f267791f58ba`, passed the short workflows below.
The packaged executable SHA-256 was
`d7896027a0ddaffc234a68b97b4d09fc08c92073a3093015d4e3743b9eecd748`
before the lab bundle identity, private environment guard and ad hoc signature.
That hash matched the mounted package inventory retained with the
[release receipt](../release/v0.1.30.md). This is installer-copy evidence,
not a newer source candidate.

## Workflows observed

- **Live history:** 21 simultaneous local forwards retained 21 native Stop
  controls and a workspace badge of 21. The app process owned exactly the
  corresponding 21 loopback listeners. The oldest row still worked: its
  byte-matched 32 KiB client received EOF after native Stop, the listener
  rebound, and the same SSH connection remained active.
- **Shared admission:** 30 local forwards, one SOCKS5 proxy and one explicit
  remote forward shared the 32-job limit. All 32 Stop controls remained.
  A further local start showed `SSH session is busy; wait for an operation
  to finish, then retry explicitly`; its provisional listener was released
  and rebound. SSH remained active. After Stop freed capacity, a new local
  start succeeded. This is new-start recovery, not a finite-file Retry test.
- **SOCKS5 and explicit remote Stop:** each open client completed an exact
  32 KiB binary echo. Native Stop closed the client and released its listener
  while SSH stayed connected. The explicit remote endpoint retained its
  requested nonzero port through the OpenSSH success reply and Stop.
- **SSH-tab Close:** with 31 local listeners and one byte-matched client still
  open, closing the SSH tab released all 31 listeners, ended the client and
  left no app TCP sockets. The local zsh terminal remained visible and live.
  The tunnel badge returned to zero; retained finished history was 20 rows.
- **Normal Quit:** after reconnecting, local, SOCKS5 and server-allocated remote
  forwards each carried an exact 32 KiB roundtrip and retained an open client.
  Native menu Quit ended all three clients, released their listeners and
  removed the app, local PTY and owned sshd session children. The fixture
  daemon remained available until its explicit stop marker was created.

Across these steps, seven distinct clients matched the binary payload's
SHA-256 `e11360251d1173650cdcd20f111d8f1ca2e412f572e8b36a4dc067121c1799b8`.
The echo server recorded seven EOF closures and 32,768 received bytes per
connection before server cleanup. Echoing those bytes produces 65,536 total
payload bytes across the two forwarding directions per connection.

## Known counter defect

The stopped local, SOCKS5 and remote rows displayed **0 B forwarded** despite
those verified roundtrips. The runners add `copy_bidirectional` totals only
when a child finishes successfully; cancellation shuts down workers without
retaining their partial totals. Active long-lived traffic also has no live
counter update. The next fix must retain successfully forwarded bytes during
traffic and after cancellation across all three runners. These receipts prove
traffic and cleanup independently; they do not validate the displayed byte count.

## Isolation and cleanup

The app's actual HOME/ZDOTDIR/XDG paths and empty agent settings were verified
before connection and again before Quit. Portable profiles/settings were private;
only generated key references and trust files were imported. The app copy was
prepared using [the existing lab tool](../../tools/prepare-macos-ui-lab.mjs).

The existing `native_file_editor_lab` exported the secret-free profile and
ran a separate generated-key OpenSSH daemon on an OS-assigned `127.0.0.1`
port. Its config and actual listener were checked. HOME/ZDOTDIR and shell
startup environment were disposable; PAM, user RC and user environment files
were disabled. No personal SSH configuration, keys or agent were used.

```sh
cargo test --locked -p mobarust-ssh --test local_sshd native_file_editor_lab -- --ignored --exact --nocapture
```

The harness completed successfully in 478.17 seconds, after the native app
closed and its owned stop marker was created. Its private HOME, generated
keys/trust/files and daemon were removed; the loopback port rebound. The
bounded echo server also joined all workers and released its loopback port.
This did not enable Remote Login, change firewall/router rules or expose an
Internet-facing listener. Passing the fixture harness alone is not GUI evidence.

## Remaining gates

The byte-counter defect remains open. Larger transfer lists, finite-action
saturation/Retry, queued approvals, uncertain remote-forward failure dialogues,
keyboard/focus/resize, sustained traffic, many-session pressure, Intel hardware,
Windows/Linux and clean installation/uninstallation remain separate gates.
Earlier startup/editor receipts retain their original versions. The roadmap's
57/76 checklist is unchanged; no broad platform claim follows from these short
ARM64 workflows.
