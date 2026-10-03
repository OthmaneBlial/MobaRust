# v0.1.31 ARM64 native tunnel counters — 2026-10-03

A disposable copy of the verified ARM64 installer, runtime source/tag
`d9910c815508fcf51e074c89fcb4a1353220a05d`, passed the short workflows below.
The packaged executable SHA-256 was
`0a4e93f9e0cc23909d3de240c73a5416c47c9944c58d2c873abdf62d550a7e3a`
before the lab bundle identity, private environment guard and ad hoc signature.
It matched the read-only mounted package inventory. Shipping bundles contain
no lab environment metadata. See [release checks](../release/v0.1.31.md).

## Workflows observed

Each client sent 32,768 binary bytes and received an exact echo, matching
SHA-256 `e11360251d1173650cdcd20f111d8f1ca2e412f572e8b36a4dc067121c1799b8`.
Each roundtrip therefore forwarded 65,536 payload bytes across both directions.

| Native workflow | Observed counter and cleanup |
| --- | --- |
| Local, completed then active client | The completed client received EOF. A second client stayed open; the running row showed two connections and **128.0 KB** (131,072 bytes), counting the completed copy once. |
| SOCKS5, active client | Running row showed **64.0 KB** (65,536 bytes) while the byte-matched client remained open. SOCKS framing was excluded. |
| Server-allocated remote forward, active client | Running row showed **64.0 KB** while the byte-matched client remained open; the actual remote listener bound loopback. |
| Native Stop, all three modes | Stopped rows retained **128.0 KB / 64.0 KB / 64.0 KB**. Open clients received EOF; each listener rebound while SSH stayed active. |
| SSH-tab Close | A new local forward showed **64.0 KB** before Close. The open client received EOF, its listener rebound, the badge returned to zero and the app had no TCP sockets. The local zsh terminal remained live. |
| Normal menu Quit | After reconnecting, three new local/SOCKS5/remote rows each showed **64.0 KB** with an open byte-matched client. Quit ended all clients and removed the app, local PTY and owned sshd session children. All three ports rebound before stopping the fixture daemon. |

Eight distinct clients passed. The echo server recorded eight EOF closures,
32,768 received bytes per connection, no remaining clients on stop, joined
workers and a released/rebound listener. The native screenshot was reviewed
locally; raw lab screenshots containing local account details are not public media.

## Isolation and cleanup

The app's actual HOME/ZDOTDIR/XDG paths and empty SSH-agent settings were
verified before connection and again before Quit. Portable profiles/settings
were private. Only generated key references and trust files were imported,
using [the existing lab tool](../../tools/prepare-macos-ui-lab.mjs).

The existing `native_file_editor_lab` supplied a generated-key OpenSSH daemon
on an OS-assigned `127.0.0.1` port. Its configuration and actual listener were
checked; PAM, user RC and user environment files were disabled. The echo
server and tunnel listeners also bound loopback. No personal SSH configuration,
keys or agent were used. Remote Login and firewall/router rules were unchanged.

```sh
cargo test --locked -p mobarust-ssh --test local_sshd native_file_editor_lab -- --ignored --exact --nocapture
```

The harness passed in 472.03 seconds (475.74 seconds including its wrapper).
Its stop marker was created only after normal Quit and owned child cleanup.
The private HOME, generated keys/trust/files and daemon were removed and its
port rebound. All owned client/server/process handles reached terminal success.
The harness result alone does not prove GUI behavior; the native observations
and independent byte/EOF/rebinding checks above supply that evidence.

## Scope and remaining gates

These are short ARM64 installer-copy workflows. They validate the displayed
live and stopped totals after the [v0.1.30 counter defect](native-tunnels-v0.1.30.md).
They do not repeat v0.1.30's 21-row retention or shared 32-job admission checks.
Backend regression tests separately cover accepted partial bytes after a
copy error; this native check does not exercise saturated writes or error totals.

Sustained traffic, larger transfer lists, finite-action saturation/Retry,
uncertain forwarding failure dialogues, keyboard/focus/resize, many-session
pressure, Intel hardware/GUI, Windows/Linux and clean install/uninstall remain
open. Earlier receipts keep their original artifact scope. The roadmap remains
57/76; no broad platform or production-readiness claim follows from this check.
