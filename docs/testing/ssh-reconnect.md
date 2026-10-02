# Native SSH retry-budget and closure receipt — 2026-10-02

## Corrected behavior

After an exhausted reconnect budget, the Rust SSH manager emits `failed`, then
removes the session and emits `disconnected` and `closed` with the failure reason.
The terminal previously replaced its error status with `closed` in both final
handlers. It now keeps abnormal SSH closure as `error`; ordinary `closed` /
`closed by application` reasons still produce `closed`.

The workspace context indicator, LIVE badge and active-transport callout now
follow the selected terminal's state. An unavailable transport is labelled with
its current state and gives an explicit route to another connection. Telnet and
serial retain their existing resumable-failure lifecycle. No automatic retry,
trust policy, credential handling or remote command replay was added.

This correction follows the v0.1.18 tag on `main`; the published v0.1.18 DMGs
have not been replaced and do not contain it.

## Native lab and isolation

macOS ARM64 / Apple M2, with a debug native app and production frontend based
on `2c326f4` plus this correction. Two successive builds were used: the first
verified error preservation, retry exhaustion, shell recovery and active SSH
shutdown; the second added and verified the state-dependent workspace labels.

The app was a copied portable bundle with a unique identity, disposable
HOME/ZDOTDIR/XDG directories and generated profiles. Its actual native PID and
HOME/ZDOTDIR were checked before and after selecting it through CUA. Copied
bundle launch metadata provided the same isolated environment as a safeguard.
No personal app, SSH configuration, keys, agent or Keychain entries were used.

A disposable OpenSSH daemon listened only on `127.0.0.1:52966`. A repository-local
TCP relay listened only on `127.0.0.1:52967`; listener addresses were verified
with `lsof`. Generated Ed25519 client/host keys and explicit generated
`known_hosts` authenticated the selected OS account with server-side
HOME/ZDOTDIR set to the temporary fixture. User SSH rc/environment, password
and interactive authentication, forwarding and X11 were disabled. This is an
application-level fixture boundary, not an OS sandbox.

The app settings allowed **two** reconnect attempts with a 500 ms connection
timeout. The owned relay could drop its existing sockets and reject subsequent
connections, without changing trust, touching the system SSH service, enabling
Remote Login or changing router/firewall rules.

## Observed checks

1. Selecting the saved profile opened a real authenticated SSH shell.
2. Disabling the owned relay interrupted its established transport. Exactly
   two subsequent connections were rejected; the app stopped retrying.
3. The final tab and statusbar stayed `error`, with the last failure visible
   in the terminal. After the final label correction, the context indicator
   was red, the session badge read **ERROR**, and the callout read **SSH error**
   with guidance to reopen a profile or use Quick connect.
4. Restoring the relay did not initiate another automatic connection. Explicitly
   selecting the saved profile opened a fresh authenticated session while
   retaining the old error tab. A native terminal command wrote the exact
   `recovery_verified` line into the disposable HOME; file-byte verification
   passed. Generated trust still matched the expected fixture entry.
5. Native menu Quit exited the first app with code zero and released its active
   local zsh child and the fixture's SSH session/remote shell processes. The
   relay observed disconnection while its listeners remained available.
6. The rebuilt final candidate repeated the interruption: exactly two more
   rejections, then ERROR/SSH error. Explicit profile recovery returned the
   selected terminal to connected/LIVE/SSH transport active.
7. Entering `exit` in the new SSH shell produced `closed` / **CLOSED** / **SSH
   closed**, with no new reconnect attempt. Normal shell exit was not turned
   into an error by this change.
8. Final native Quit returned code zero and released its local shell. The
   owned relay/daemon were stopped and reaped; both recorded ports refused
   connections. Generated client and host keys were removed from the lab.

UI actions used native CUA controls and screenshots, without page JavaScript
injection. The relay's final totals were four accepted connections and four
rejected reconnect attempts over the two controlled failure cycles.

## Runnable regression and remaining gates

```sh
pnpm --dir apps/desktop test:unit
pnpm --dir apps/desktop check
pnpm --dir apps/desktop lint
cargo xtask check
```

The complete final local `cargo xtask check` passed: workspace tests/Clippy,
frontend tests/type/lint/build, release-asset checks, isolated RDP checks, all
17 VNC process cases, package layouts and fuzz compilation. Both native
candidates built successfully; the pre-push payload audit passed.

The existing terminal-close test checks both final SSH event classifications,
repeat-short-shell failure reasons, control/whitespace normalization and normal
closure, while preserving local/Telnet/serial classification. Existing Rust
checks separately verify bounded failures, last-error propagation, first-success
return, cancellation and the short-shell retry counter. These are independent
of the native observation above.

To repeat the native acceptance check, create a fresh isolated app/profile and
loopback SSH/relay fixture with generated trust. Configure two attempts, connect,
disable only the owned relay, inspect its rejected count and the final UI, then
restore it and prove no new connection occurs until selecting the profile.
Verify shell execution with a fixture file marker and test a normal `exit`.
Record actual app/session PIDs before Quit, verify that they exit, then stop
only the owned fixtures and probe their ports for closure.

This does not prove zero/maximum retry budgets in the GUI, repeatedly flapping
short-lived shells, a real daemon restart, password/PAM/MFA, larger or interrupted
transfers, Windows/Linux GUI behavior, clean installation or sustained use.
Those gates remain open. CI stays disabled; local evidence is not a CI run.
