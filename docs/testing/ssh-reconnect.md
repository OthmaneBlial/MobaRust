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

## Terminal size across reconnects on main

The native worker reopened SSH shells with the original connect dimensions.
Resizes during backoff were rejected because the old command queue had closed;
the frontend does not refit solely because SSH emits `connected`. A regression
against `9f82da5` reproduced `Closed` when updating geometry during backoff.

Terminal geometry now uses one per-session Tokio watch value. It coalesces
updates independently of the command queue, including while authentication or
reconnect is pending. Initial and replacement shell setup share the same method
for reading current dimensions. Changes arriving after that snapshot remain
pending for the shell pump, which sends the latest window-change request through
the existing output-aware, cancellable operation path. Zero dimensions retain
the previous transport behavior of clamping to one column/row.

This preserves terminal size, not old actions: retired input and remote file
commands remain rejected. Close, application shutdown, missing sessions and a
terminated size receiver refuse further resize updates.

```sh
cargo test --locked -p mobarust terminal_resize_survives_retired_command_queue
cargo test --locked -p mobarust blocked_shell_input_keeps_output_and_cancellation_live
cargo xtask check
```

The manager regression checks updates before/after queue retirement, 1,000
coalesced resizes, fresh queue isolation, clamping and refusal after closure or
shutdown. The existing encrypted, memory-only loopback fixture additionally
observes an initial `80×24` PTY, a replacement `132×41` PTY on the same transport,
then a `155×53` window change submitted while the server's replacement PTY
handler is paused. It also retains exact-once input and blocked-output/close
checks. Listener addresses are `127.0.0.1` with OS-assigned ports; generated
in-memory credentials and pinned host keys are used, and listener release is
checked after transport/worker cleanup.

The complete local `cargo xtask check` passed on macOS ARM64, including all
104 desktop tests, workspace tests/Clippy, frontend tests/type/lint/build,
protocol fixtures, unsigned package-layout contracts and fuzz compilation.

This correction follows the v0.1.20 tag and is included in both verified
v0.1.21 Mac DMGs. Windows/Linux installers remain v0.1.12.
Native GUI resize/reconnect acceptance and Windows/Linux runtime evidence remain
open. Replacing a shell on one authenticated transport is not evidence of a
complete transport restart or sustained GUI responsiveness.

## Shell request acceptance on main

**2026-10-03.** The pinned russh channel's `request_shell(true).await` enqueues
a request; it does not wait for the server's reply. MobaRust previously treated
that enqueue as completed setup and its output reader ignored a subsequent
failure. A memory-only loopback regression against `9ddf74c` reproduced a
successful `open_shell` result after the server explicitly denied shell access.

Shell setup now checks the server's success/failure reply before returning a
shell or sending configured startup input. Optional X11 forwarding is checked
first, so its success cannot be mistaken for shell acceptance. This follows
[RFC 4254 sections 5.4 and 6.5](https://www.rfc-editor.org/rfc/rfc4254.html#section-5.4).
PTY and environment requests retain their existing no-reply behavior.

Channel open, X11, shell acceptance and startup input share the original setup
deadline. A denied request, early closure or missing reply returns an explicit
error. Failure/timeout cleanup retires the reader before enqueueing channel
close, with a separate one-second maximum for that cleanup enqueue.
Stdout/stderr arriving before success are buffered in order and delivered before
live output in both split and unsplit readers. The 1 MiB setup budget accounts
for data and queue entries, including empty/tiny packets.

```sh
cargo test --locked -p mobarust-ssh --test authentication shell_setup_requires_server_acceptance
cargo test --locked -p mobarust blocked_shell_input_keeps_output_and_cancellation_live
cargo xtask check
```

The authentication fixture checks shell rejection, X11-success followed by shell
rejection, missing shell reply, early channel closure, pre-acceptance output
overflow, X11 rejection, and ordered fragmented Unicode/stdout/stderr in both
reader modes. Failed setups send no startup command; rejection, timeout and
overflow close the channel before transport teardown. The desktop fixture also
pauses replacement PTY handling, proves the open future is still pending, then
releases it and checks the latest geometry on the wire.

The complete local `cargo xtask check` passed on macOS ARM64, including all
104 desktop tests, workspace tests/Clippy, frontend tests/type/lint/build,
protocol fixtures, unsigned package-layout contracts and fuzz compilation.

The memory-only fixtures use generated credentials, pinned host keys and
`127.0.0.1` listeners with cleanup assertions. They do not access an OS account,
personal SSH state, agent, Keychain or X server. Native GUI rejection/timeout
acceptance and wider server/platform coverage remain open. The correction is on
main after v0.1.20 and is included in both verified v0.1.21 Mac DMGs.
Windows/Linux installers remain v0.1.12.

## Startup input and output backpressure on main

**2026-10-03.** After shell acceptance, configured startup input was written
without reading output until the write finished. A peer with a small receive
window and enough output to fill the bounded channel queue could stall the SSH
actor before it processed further input window credit. The startup write then
hit its setup deadline despite a healthy authenticated transport.

The `startup_input_keeps_shell_output_draining` regression reproduced this
timeout on `279d918`: the peer grants a 1 KiB input window, accepts the shell,
then emits 256 packets of 1 KiB while the client sends an 8 KiB startup command.

Shell setup now splits the existing reader/writer before startup input. A single
pinned write remains alive while incoming output is consumed into the existing
bounded setup buffer. Output events never recreate or replay the partial write.
The unsplit public shell API delegates to these same halves. Shell acceptance
and startup input retain one setup deadline; the shared 1 MiB output budget
includes queue entries. Timeout, early exit and overflow use the same path that
retires the reader before bounded channel-close cleanup.

```sh
cargo test --locked -p mobarust-ssh --test authentication startup_input_keeps_shell_output_draining
cargo test --locked -p mobarust-ssh --test authentication cancelling_pending_startup_releases_the_owned_transport
cargo xtask check
```

Both split and unsplit reader checks compare every output byte in order,
including the initial burst, banner and echoed startup input. The peer's bounded
input receipt equals the complete startup command plus newline exactly once.
Separate zero-window cases verify setup timeout, EOF/exit/close before input
completion and output overflow; no input is accepted in those cases, and failed
startup channels are closed before transport teardown. Fixture workers finish
and their loopback listener ports can be rebound.
The cancellation check aborts the task owning the connection while setup is
pending after server shell acceptance with zero input credit. No startup input
is received; the server task finishes and its listener port can be rebound.

The complete local `cargo xtask check` passed for the runtime correction on
macOS ARM64, including all 104 desktop tests, workspace tests/Clippy, frontend
tests/type/lint/build, protocol fixtures, package-layout contracts and fuzz
compilation. The additional cancellation regression and final workspace Clippy
also passed after that suite.

The fixture uses generated memory-only credentials and pinned host keys on
`127.0.0.1`; it does not execute an OS shell or inspect personal state. This is a
bounded protocol regression, not sustained native rendering or GUI startup
acceptance. The correction follows v0.1.20 and is included in both verified
v0.1.21 Mac DMGs. The later [v0.1.27 release-copy partial receipt and current-source native setup check](ssh-lab.md#v0127-arm64-release-copy-partial-acceptance--2026-10-03)
add a connected exact-once startup, explicit tab close, shell refusal and
zero-credit timeout on Mac ARM64. Full native output ordering, sustained
rendering and updated Windows/Linux installers remain open; package checks
alone do not establish those gates.

## Startup timeout diagnosis on main — 2026-10-03

The current-source native zero-credit check displayed only “SSH connection
timed out”, even though authentication and the shell request had succeeded.
The shared `open_shell` path now returns `SshError::StartupInputTimeout` when
the startup-input write reaches its existing setup deadline. Its static message
names that phase and advises checking the remote session and startup settings
before reconnecting, because some input may already have reached the server.
It includes no command text, credentials, destination or raw server detail.

Initial connections and automatic reconnects already use this shared path, so
both receive the same diagnosis. The write, output-draining loop, deadline,
channel-close cleanup and approval policy are unchanged. Timeout before shell
acceptance remains a separate connection/setup failure; the new error does not
claim that authentication or connectivity failed.

The existing real-loopback startup regression failed on the generic message
before the correction and passed afterward. It checks the typed startup timeout,
exact message and absence of configured startup text, alongside zero accepted
input and channel close before transport teardown. The same regression retains
its split/unsplit ordered-output and exact-once success checks, early-exit and
buffer-overflow cases. The disposable native endpoint regression now expects
the specific timeout as well. Published v0.1.27 installers retain the earlier
message; the correction currently belongs to main.

The full local `cargo xtask check` passed on macOS ARM64, including workspace
tests/Clippy, frontend tests/type/lint/build, protocol fixtures, fuzz compilation
and package-layout contracts. The real X11 server case was skipped for missing
prerequisites; native observation of the revised message remains a separate
check.

The corrected source `59c531a` also passed `cargo xtask package-check`: the
Mac debug bundle, VNC resource, checksum manifest and isolated `--version`
probe passed. A freshly prepared, ad hoc signed copy was launched with
verified disposable HOME/XDG paths and empty SSH-agent settings. The UI tool
could not observe its window on two attempts against the same confirmed live
process, so no SSH fixture was started and no revised-message GUI result is
claimed. Only the confirmed owned runtime received SIGTERM; it and its local
zsh child exited. Normal native Quit was not observed in this attempt. The
observation failure's cause remains unproven; it does not establish an SSH
regression. The new message still needs native acceptance.

## v0.1.28 ARM64 release-copy startup timeout — 2026-10-03

A disposable copy of the verified ARM64 DMG runtime from source/tag
`51f637d9b31c204072d30d706c171aa79248fe28` passed native startup review
and the revised timeout diagnosis. Before re-signing its unique lab bundle,
the runtime hash matched the mounted installer. HOME/ZDOTDIR/XDG paths were
verified as disposable and SSH-agent variables were empty. Generated Ed25519
host keys were pinned; all three fixture endpoints listened on `127.0.0.1`
with OS-assigned ports. No fixture executed an OS shell.

The 8 KiB startup review kept its heading, destination, reconnect warning and
Cancel/Continue controls visible. Cancel held initial focus; Shift-Tab and
Page Down focused/scrolled the command region while the surrounding context
stayed visible. Escape returned to the workspace with no established fixture
connection. After explicit Continue and distinct generated password/OTP
answers, the zero-credit endpoint produced the exact new message:

> SSH startup input timed out; some input may have reached the server. Check the remote session and startup settings before reconnecting.

No failed SSH tab was created; the local PTY stayed active. The fixture reported
one shell request, zero startup bytes and no exact-once success receipt. No
fixture connection remained established after the error. Entered factors were
absent from the app's owned persistence and final accessibility state.

Normal native menu Quit released the app and local zsh child without a signal.
The same fixture completed its five-minute deadline with exit 0, removed its
private metadata and disposable HOME, and released all three ports; explicit
loopback rebind checks passed. This run exercised the stalled endpoint only.
Successful startup, shell refusal and full output ordering retain their earlier
source/test receipts; they are not new v0.1.28 GUI claims. Intel GUI,
Windows/Linux and wider server/load acceptance remain separate gates.

Both v0.1.28 Mac DMGs and their manifests passed local package verification and
anonymous public-download byte comparison. [Release checks and limits](../release/v0.1.28.md).

## Uncertain startup delivery stops reconnect retries on main — 2026-10-03

After v0.1.28, the desktop reconnect attempt preserves typed SSH failures until
the retry decision. Previously it converted shell-setup errors to strings and
retried every failure, including startup-input timeouts: a three-attempt
regression observed attempts `[1, 2, 3]` when it should stop after `[1]`.

`StartupInputTimeout` now stops the reconnect loop immediately. Other failures
while writing configured startup input are wrapped as `StartupInputFailed`,
retain their native source, and stop the loop too. This includes a peer closing
the shell, a transport/write failure, and output-buffer overflow during the
write. The static display warning advises checking the remote session and
startup settings before reconnecting and contains no configured command or
server detail. These classifications also cover configured startup directories.

Authentication, channel-open and shell-request failures before startup writing
retain the existing bounded retry policy. Successful reconnects still send the
reviewed startup input; a previously successful startup is not an exactly-once
operation across a later connection loss. Approval continues to explain that
automatic reconnect can rerun the command. After uncertain delivery the user
must explicitly open a fresh connection and review its startup configuration.

Runnable regressions:

```sh
cargo test --locked -p mobarust reconnect_policy_
cargo test --locked -p mobarust-ssh --test authentication startup_input_keeps_shell_output_draining
cargo test --locked -p mobarust-ssh --test authentication shell_setup_requires_server_acceptance
cargo xtask check
```

The retry regression checks timeout, shell closure, overflow and transport
failures both on the first attempt and after a retryable connection refusal.
Existing cases retain bounded failure, first-success and in-flight cancellation
checks. The packet fixture exercises a peer accepting a strict prefix of a
large startup command before closing, alongside zero-credit timeout, early
exit and overflow. It checks the typed phase, private display, exact accepted
prefix, one shell request, socket/worker teardown and listener release. Ordered
256 KiB output and successful exact-once input remain separate cases in the
same test. Every endpoint is loopback with generated memory-only credentials
and pinned host keys; no OS shell or personal SSH state is used.

The targeted retry-policy and startup packet regressions passed locally on
macOS ARM64 / Apple M2. The full local `cargo xtask check` also passed, including
109 desktop tests, workspace tests/Clippy, frontend tests/type/lint/build,
protocol fixtures, helper checks, package-layout contracts and fuzz compilation.
The real Xvfb case retained its prerequisite skip. An initial full-check attempt
failed because this run’s disposable wrapper omitted `USER`; restoring the
fixture-required account name resolved it without changing production code,
test assertions, deadlines or isolation of HOME/keys/agents.

This correction is on main after the v0.1.28 tag; published installers have not
been replaced. The macOS ARM64 debug timeout retry-stop check below passed; native acceptance
of other startup-write failure types, a controlled OpenSSH peer restart,
release-copy acceptance and Windows/Linux runtime checks remain open. The v0.1.28 native
timeout receipt above proves initial-connection behavior, not this new
reconnect decision. CI remains disabled.

## Native reconnect startup retry-stop — 2026-10-03

A current-source macOS 26.6 / ARM64 / Apple M2 debug bundle of `1b856c5` passed
`cargo xtask package-check`. An executable-matched, ad hoc signed disposable
copy used a unique identity, portable data, temporary HOME/ZDOTDIR/XDG paths
and empty SSH-agent settings. Its actual process environment and native window
were verified before starting the fixture. This is a main-source debug result;
it is not a published-installer or release-copy result.

The [repeatable reconnect-startup lab](ssh-lab.md#repeatable-native-reconnect-startup-lab-on-main)
used one OS-assigned `127.0.0.1` listener, generated in-memory host key and
password/OTP factors, private metadata and a secret-free exported profile. It
executes no OS shell. The GUI showed the unchanged policy: reconnect enabled,
three attempts, 12,000 ms timeout.

Observed sequence:

1. The imported profile displayed the full startup review, destination and
   reconnect warning, with Cancel focused. Escape cancelled before any fixture
   connection was accepted or established.
2. Explicit Continue followed by the generated password and distinct OTP
   connected the SSH terminal. Its status was connected/LIVE/SSH transport
   active. The fixture accepted exactly 8,193 startup bytes once.
3. A new private empty `interrupt` marker targeted only that fixture's first
   transport. The GUI entered RECONNECTING and presented password/OTP prompts
   for the replacement at the same pinned endpoint.
4. After answering them, the zero-credit replacement timed out during startup.
   The terminal displayed “SSH failed: SSH startup input timed out; some input
   may have reached the server. Check the remote session and startup settings
   before reconnecting.” Its tab, brief badge and callout stayed ERROR/SSH error.
5. Logs recorded exactly two accepted connections, one shell request each,
   first startup 8,193 bytes/exactly once and replacement zero bytes. No third
   connection or new authentication dialogue appeared for more than 53 seconds
   after the error, while the same listener remained live and two retry attempts
   remained available. No established fixture sockets remained.
6. The two generated factors were absent from both owned persisted app files.
   Final native accessibility state had no authentication fields or dialogue.
   Normal native menu Quit released the confirmed app runtime and local zsh
   child. The SSH transport had already failed; this is not active-SSH Quit proof.

The same manual fixture completed its 300-second test lifetime with exit 0.
Its runtime and disposable HOME were absent, private metadata/control were
removed, and the recorded localhost port had no listener and could be rebound.
The complete local `cargo xtask check` passed: 109 desktop tests, 16 authentication/
fixture tests with four opt-in labs ignored by default, workspace tests/
Clippy, frontend tests/type/lint/build, protocol/helper checks, package-layout
contracts and fuzz compilation. The optional real Xvfb case retained its
prerequisite skip. The manual GUI fixture was run separately and passed; the
other ignored lab cases were not silently counted as executed.

The new fixture contract separately checks successful ordered startup, explicit
interruption, same-key/factor replacement timeout, secret-free export, worker
completion, port rebinding and removal of private metadata/control. It shares
the existing endpoint/profile helpers; no new dependency or public test service
was added. The existing setup fixture remains three endpoints with its original
three-second automated lifetime.

Native overflow/peer-closure/transport-write failure classification, release-copy
retry-stop acceptance, real OpenSSH/PAM restart and Windows/Linux checks remain
open. Earlier successful startup can still run again on a later approved
reconnect. No public installer or historical tag was replaced; CI stays disabled.
