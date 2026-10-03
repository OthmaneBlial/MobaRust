# Local SSH integration lab

Run from the repository root:

```bash
cargo xtask test-ssh
```

The command runs SSH transport unit tests, portable authentication-wire tests,
and the disposable Unix OpenSSH fixtures with an isolated HOME/XDG environment
and without an inherited SSH agent or askpass. It uses the existing Rust test
harness; Docker, system-service changes,
personal SSH files, and remote servers are unnecessary.

The [interrupted-transfer checks](interrupted-transfers.md) exercise 16 MiB
SFTP/SCP streams over an owned loopback relay, preserve originals on loss,
report unavailable remote cleanup and verify a full byte-matched retry.
The [routed IPv6 stream check](routed-ipv6-streams.md) concurrently drains 8 MiB
of real PTY output and transfers byte-matched files through two distinct bastions.

## OpenSSH fixture requirements (Unix)

- macOS or Linux, Rust, `ssh-keygen`, `ssh-agent`, `ssh-add`, and an installed OpenSSH `sshd` at
  `/usr/sbin/sshd` or `/usr/local/sbin/sshd`;
- an existing, unlocked, non-root OS account (`USER`) that can run its shell;
- `xauth` on PATH for the loopback X11-channel test;
- optional Xvfb and an existing real `/tmp/.X11-unix` directory for the real
  X11-server test. That test reports a skip when its prerequisites are absent.

On Apple Silicon, prefer native `arm64` xauth on PATH. The fixture selects the
first `xauth` it finds. The [Homebrew xauth formula](https://formulae.brew.sh/formula/xauth)
provides a standalone native client tool; it does not start an X server. For an
explicit runner setup:

```sh
brew install xauth
command -v xauth
lipo -archs "$(command -v xauth)"
```

Tests do not create users, set account passwords, install dependencies, or
enable Remote Login. A missing prerequisite is a lab setup failure; configure
a dedicated test runner rather than granting the tests system permissions.
On Windows, `test-ssh` runs portable unit and authentication-wire tests and
reports that the OpenSSH fixtures are skipped. The wire cases contain no
Unix-specific APIs; their execution has so far been verified on macOS ARM64.
The opt-in native labs and their metadata/profile/cleanup regressions are Unix-only.

## Portable authentication-wire fixture

```bash
cargo test --locked -p mobarust-ssh --test authentication
```

The portable cases in `tests/authentication.rs` start a single-connection `russh` server on
`127.0.0.1:0` for each case, using generated memory-only Ed25519 keys and
zeroizing disposable credentials. The client pins the generated fingerprint.
There are no OS accounts, PAM changes, agent requests, credential files,
subprocesses, or new runtime dependencies. Each test uses its own Tokio runtime; it
awaits server-session termination and rebinds the released listener address.
The server has no inactivity timeout, so that cannot satisfy the client-socket
cleanup assertion. Stalled authentication callbacks are explicitly released
after timeout/cancellation to let the server observe closure.

Authentication coverage includes:

- Password acceptance/rejection, connected lifecycle state, explicit disconnect
  and session/socket cleanup.
- Keyboard-interactive acceptance/rejection, zero/one/eight non-echo prompts,
  and a two-round challenge with the same response in each prompt.
- Echo-enabled and nine-prompt challenge refusal before any response is sent.
- Host-key rejection before password or keyboard-interactive callbacks run.
- Timeout and task cancellation after each authentication method has started,
  followed by session/socket cleanup.
- Distinct generated password/OTP acceptance in one two-prompt round and two
  separate rounds; wrong-OTP rejection and static-response rejection.
- Interactive responder cancellation/drop/server disconnect, trust-before-callback, response
  count/size rejection and a 17-round server refused after 16 responses.
- Two distinct bastions and a target, each with its own generated password/OTP
  and host key. Fifteen chain cases check ordered prompt ownership, success,
  wrong OTP, host-key rejection before a prompt, cancellation and dropping an
  unanswered responder at each endpoint. Success includes a byte-matched UTF-8
  echo-channel round trip through both bastions, explicitly labelled
  **no OS shell**. Unconfigured forwarding host/port requests are refused before
  TCP/DNS; all reached sessions close and all fixture ports are rebound after
  the attempt.

Shell setup regressions additionally cover actual request acceptance, ordered
stdout/stderr before acceptance, X11/shell rejection, deadline and output-budget
failure, startup input under output backpressure, and cancellation of an owned
pending startup transport. See [startup setup evidence](ssh-reconnect.md#startup-input-and-output-backpressure-on-main).
The Unix native-endpoint smoke test below also writes and removes private lab
metadata; that metadata is separate from the portable memory-only cases.

These are actual SSH handshakes and encrypted authentication packets, with the
same Rust stack at both ends. They do not establish OpenSSH password/PAM/MFA
interoperability. The existing static keyboard-interactive credential repeats
one secret for all non-echo prompts and rounds; distinct password-plus-OTP or
user-selected responses are not supported by this path.

Verified 2026-10-02 on macOS ARM64 with Rust 1.95.0 and the
[repository-local `russh` 0.63.3 patch](../../vendor/russh/MOBARUST_PATCH.md):
the local SSH suite passes eight automated wire tests and a Unix-only native
lab directory-permission/cleanup regression; both opt-in native fixtures are
ignored by default. The earlier static-response receipt
had 41 unit, five wire and 12 OpenSSH tests, with loopback IPv6 executed and the
real Xvfb case skipped for missing prerequisites.
The ordinary `cargo xtask test-ssh` and workspace suite include this fixture.

### Repeatable native shell setup lab on main

```bash
cargo xtask package-check
node tools/prepare-macos-ui-lab.mjs
# After checking the disposable app environment and native window:
cargo test --locked -p mobarust-ssh --test authentication native_shell_setup_lab -- --ignored --exact --nocapture
```

The Unix-only lab runs for five minutes with three OS-assigned `127.0.0.1`
listeners, generated in-memory host keys and distinct generated password/OTP
factors. Its printed private directory contains `startup.json`, `rejected.json`
and `stalled.json` with connection metadata and disposable factors. Files are
`0600` inside a `0700` temporary directory; do not publish them. `profiles.json`
is a secret-free session export: generated host-key pins, ask-each-challenge
authentication, and an 8,192-byte startup command (`fixture-startup-` repeated
512 times). No credential reference, agent, Keychain or OS account is involved.
The echo server does **not** execute this text as an OS command.

Import `profiles.json` through **Import MobaRust session export** in the isolated
app. Select each profile, approve its configured startup input, then answer the
password/OTP prompts with that endpoint's generated factors:

- **SSH setup startup:** a 1 KiB peer input window plus 256 KiB of output in
  256 packets; expect a connected terminal, the no-OS-shell banner and the
  startup echo. Close the SSH tab before further input if checking the exact
  startup receipt. The server retains at most 16 KiB of received input.
- **SSH setup rejected:** authentication succeeds, but the shell request is
  rejected. Expect a useful error and no successful shell or startup input.
- **SSH setup stalled:** authentication and the shell request succeed, but the
  peer gives no input credit. Expect a setup timeout, not a connected shell.

Completed server sessions report only shell-request/input counts and whether
the input exactly matches the 8,193-byte command plus newline. Entering other
terminal input changes that equality result. At the deadline, owned workers
are cancelled/joined, listener addresses are rebound, and the temporary directory
is removed. This is a fixture deadline, not proof of native app Quit.

The ordinary test
`native_shell_setup_endpoints_are_isolated_and_cleanup` exercises the same
endpoint/profile preparation with a three-second fixture lifetime. It verifies
private file modes, valid secret-free profiles and their matching host-key pins,
real two-factor authentication, ordered burst/banner/startup echo, shell refusal,
startup timeout, completed workers, port rebinding and metadata removal.
It uses the workspace's existing `serde_json` as a dev dependency; application
dependencies and shipped runtime behavior are unchanged.

On **2026-10-03**, `cargo xtask check` passed on macOS ARM64, including all
13 automated authentication-fixture tests; three native manual labs remained
ignored. Workspace tests/Clippy, frontend checks, 17 VNC cases, package-layout
contracts and fuzz compilation also passed. A separate native
attempt built current source `545bd1c`, verified the owned app's disposable
HOME/ZDOTDIR/XDG paths and empty agent socket, and observed the window plus
Quick connect through native accessibility and a screenshot. Subsequent native
observation reported `cgWindowNotFound`, including after selecting the live app
by its bundle ID and resetting UI control. No fixture credentials were entered
and no SSH profile was connected. The owned app PID and zsh child were stopped
with SIGTERM; the five-minute fixture completed, removed its metadata, and all
four recorded app/shell/cargo/fixture PIDs were absent. This does **not** establish
GUI startup/backpressure/rejection acceptance or normal native Quit. Those
gates remain open, as do updated installer and Windows/Linux observations.

#### v0.1.27 ARM64 release-copy partial acceptance — 2026-10-03

A fresh disposable copy used the verified ARM64 DMG runtime from release source
`8384eb5`. Before importing the three generated pinned profiles, the actual
app process was checked for isolated HOME/ZDOTDIR/XDG paths and an empty SSH
agent socket. The fixture used only its three OS-assigned `127.0.0.1` listeners,
in-memory host keys and disposable factors; it ran no OS shell.

The native review named the startup endpoint and explained automatic reconnect
repetition. Escape cancelled the first review, and no established connection
to any of the three fixture endpoints existed afterward. On a new explicit
connection, Continue was followed by separate masked password and OTP prompts.
The connected SSH terminal and startup echo were visible. The fixture reported
`shell_requests=1, input_bytes=8193, startup_exact_once=true`.

Native window observation then failed with `cgWindowNotFound` while the app was
still running, before the shell-refusal and stalled-input cases were operated.
Rebinding the live app and resetting UI control did not restore observation.
The owned app was stopped with SIGTERM and its local PTY child was confirmed
absent. The fixture reached its five-minute deadline with exit 0, removed its
private metadata and released all three ports for rebinding. Its counter receipt
does not establish whether native close, shutdown or inactivity ended the SSH
session. Normal native Quit was not verified in this attempt.

A second isolated release copy lost window observation before any fixture or
SSH connection was started; its owned app and child were also cleaned up.
The observation failure's cause is unproven. This partial receipt closes neither
native shell-refusal/stalled-input acceptance nor broad reconnect/emergency-stop
acceptance. The long review exposed a [layout issue corrected on main](text-input-dialogs.md#long-startup-review-layout-on-main),
which the subsequent current-source check below exercises.

#### Current-source native shell setup acceptance — 2026-10-03

`cargo xtask package-check` passed for source `0485c22`, including the Mac ARM64
debug bundle layout, shipped VNC helper and checksum manifest. A fresh disposable
copy used the production frontend, private portable state and isolated
HOME/ZDOTDIR/XDG paths with no SSH agent. The preparation tool checked runtime
byte equality before ad hoc signing the originally unsigned debug copy; signing
changed its signature bytes. This was not the public v0.1.27 DMG runtime.

The existing five-minute lab supplied three generated pinned profiles, private
distinct password/OTP factors and only OS-assigned `127.0.0.1` listeners. Native
UI observations and the fixture's bounded input receipts established:

- Long startup review kept its destination, reconnect warning and controls in
  view in dark and light modes. Cancel initially had focus. Enter cancelled
  before any fixture connection; Shift+Tab/Page Down scrolled the command
  region, and Escape also cancelled before networking.
- After explicit Continue and distinct password/OTP prompts, shell refusal
  displayed “SSH server rejected the shell request; check account permissions
  and server configuration”. No SSH tab was created; the server recorded one
  shell request and zero input bytes.
- With no peer input credit, setup displayed “SSH connection timed out” and
  created no SSH tab. The server recorded one shell request and zero input
  bytes. This timeout text does not identify the stalled setup phase.
- A new explicit startup connection succeeded. Closing its SSH tab returned
  to the local terminal; the server recorded one shell request and exactly
  8,193 startup bytes once. All three endpoints had no established connection
  after that close.
- Native menu Quit retired the app and its active local zsh PTY. The process's
  isolation was rechecked before Quit, and generated factors were absent from
  the owned app's persisted state. No forced signal cleanup was needed.

The fixture then completed its existing five-minute deadline with exit 0.
Its private metadata and disposable HOME were removed, the fixture/app/PTY
processes were absent, and all three ports had no listener and could be rebound.

These checks establish the stated Mac debug workflows, not full native ordering
of all 256 KiB of rendered output, OpenSSH/PAM behavior, queued expiry/overflow,
macro emergency-stop/restart acceptance, Windows/Linux WebViews or updated
public installers. The earlier v0.1.27 release-copy receipt remains separate.
GitHub workflows remain disabled.

### Ask each challenge on main

Quick connect and saved SSH profiles have a separate **Keyboard-interactive ·
ask each challenge** option. It does not require or save a vault reference.
Existing **vault response** profiles still repeat their one stored secret.
The new mode also applies to saved jump hops and prompts again during reconnect.
Cancelling a reconnect challenge stops that session's remaining retries.

The native transport verifies each hop's host key before prompting. Each
challenge includes that hop's configured address and username, followed by
plain-text server-provided name/instructions and prompt text. The password
field masks each response. Responses necessarily cross WebView IPC transiently;
they are not saved in profiles, the vault, audit history or diagnostics. The
dialogue clears its input on submission/cancellation, and the native owned
responses use zeroizing buffers. JavaScript strings and third-party SSH packet
buffers do not provide a whole-process zeroization guarantee.

Both modes refuse echo prompts, more than eight prompts, more than 16 rounds,
or challenge text fields above 4096 bytes. The ask mode additionally checks
exact response count and a 16 KiB limit per response. Its total authentication
deadline is 120 seconds; the configured connection timeout still bounds
network/key-exchange setup. A one-shot request ID binds answers to that
challenge. Completion, cancellation, timeout, session close and app shutdown
retire the waiter; late/duplicate answers are refused and the dialogue is
cancelled. SSH transport loss also drops an unanswered responder immediately,
instead of waiting for its 120-second deadline; the real-wire regression failed
before this correction and passed afterward. The packet reader now handles
answers through its ordinary message loop, with one pending challenge per
connection. At most 32 native challenge waiters can exist simultaneously.
On main after v0.1.19, SSH challenges share one active dialogue and at most
31 queued challenges across IPC channels. A queued closure or expiry removes
that challenge immediately; page shutdown aborts active and queued challenges.
Queue time counts toward the unchanged 120-second authentication deadline.
Ordinary file/approval dialogues still refuse overlap rather than queueing
potentially stale decisions. An SSH challenge arriving during an ordinary
dialogue is cancelled without replacing it.

The production per-channel handler in `ssh-authentication.ts` is exercised by
the shared DOM-boundary test. Independently owned channels check password/OTP
completion before the next connection's labelled prompt opens, with responses
bound to their own request IDs. Other checks cover active cancellation,
queued expiry, late closure, middle-waiter removal before a third channel,
overflow refusal/capacity recovery, page shutdown, ordinary-dialogue ownership
and sanitized IPC failure followed by the next challenge. Aborted inputs are
cleared and retired requests cannot send late answers. The queue regression
failed against the previous fail-closed handler and passed after the fix.
Frontend unit tests, type checking, lint/build and the rebuilt Mac debug
`cargo xtask package-check` passed. No dependency or native IPC payload changed.
These are production-handler/DOM-boundary checks, not native simultaneous-window
acceptance or a whole-process secret zeroization guarantee.

Three native broker regressions cover bounded/one-shot answers, expired waiter
cleanup, close-before-drop refusal, reconnect cancellation and shutdown. The
concurrency regression holds 32 independently owned IPC contexts, refuses the
33rd without emitting a challenge, retires a middle waiter without closing its
peers, and accepts a replacement immediately. All 32 remaining contexts receive
their own distinct-length response pair, reject duplicate answers and release
their registry entries. It uses the production broker with test IPC channels;
it does not establish 32 simultaneous SSH transports or native GUI overflow.

The DOM-boundary check also follows separate challenge rounds: A's password,
B's held password, then A's queued OTP. Closing the queued OTP preserves B's
label, typed value and recorded focus; B can submit that value and answer its
own next OTP without opening A's retired field. Focus here is tracked by the
minimal test DOM, not observed through an OS accessibility API. These checks
run with `cargo xtask check-rust` and `pnpm --dir apps/desktop run test:unit`.

DOM boundary checks cover password masking, plain-text server labels, abort
cancellation and clearing the input; these are not native keyboard/focus proof.
The native receipts below cover saved profiles, Quick connect, controlled
reconnects, two-bastion prompt routing and two-session queued prompt ownership
on Mac debug. Native queued expiry/overflow, broader reconnect cases and
OpenSSH/PAM interoperability remain separate gates. The ask-each-challenge mode
is included in the v0.1.20 Mac previews, including the concurrent frontend queue.
Windows/Linux remain v0.1.12. Native acceptance of that queue in the v0.1.20
release copies remains pending; the debug and earlier release receipts below
must not be treated as observations of the new installers.

### Concurrent authentication queue follow-up — 2026-10-02

The queue change on top of `42c8a75` passed the frontend checks and native
package check above. An isolated, unsigned debug app copy retained the built
main executable's exact bytes, with a separate application ID and disposable
HOME/portable data. Two independently generated Rust authentication fixtures
and two disposable relays listened only on `127.0.0.1`. Fixture metadata
directories/files were verified as `0700`/`0600`; no personal SSH files, agents,
accounts, Keychain credentials or system SSH service were used.

Both saved profiles completed their distinct masked password/OTP logins in the
native app. With the second session selected, dropping the first relay opened
the first session's focused, masked reconnect password field. After entering
that generated password, the second relay was dropped. Native window
observation then failed with `cgWindowNotFound`; process/socket checks showed
the owned app and both reconnect transports still alive. The overlap outcome,
terminal echo/focus and normal Quit were not observed, so this attempt did not
establish native concurrent acceptance. The observation failure does not
establish a cause in MobaRust.

Both five-minute Rust fixture tests completed normally. The executable-matched
app and both relays were stopped with SIGTERM; the recorded app, local shell,
server and relay PIDs were absent, all four loopback ports could be rebound,
and the transient fixture/relay response metadata was removed. A fresh isolated
app attempt also failed native window observation before any fixture listeners
started; its owned app and shell were stopped and verified absent. This cleanup
receipt establishes owned-process/listener release, not graceful native Quit
or a scan proving response absence throughout the app's persisted data.

### Native overlapping reconnects and shutdown — 2026-10-02

The Mac ARM64 debug bundle built from `f513ead` includes both the bounded SSH
challenge queue and startup replay correction. A fresh isolated app copy kept
the built main executable byte-for-byte, with its own application ID, disposable
HOME/ZDOTDIR and portable data. Native accessibility and screenshot observation
worked before starting the fixtures. Two five-minute Rust authentication labs
and two disposable relays bound only to `127.0.0.1`, with independently generated
host keys/passwords/OTPs and pinned, secret-free saved profiles. Fixture metadata
directories/files were checked as `0700`/`0600`. No personal SSH files, agents,
accounts, Keychain credentials or system SSH service were used.

Observed in the native app:

- Both profiles completed their distinct masked, focused password/OTP logins.
- With session B selected, dropping relay A opened A's reconnect password
  field. After typing A's generated password without submitting it, dropping
  relay B preserved A's label, masked value and focus. B waited.
- Submitting A's password opened B's empty, focused password field. The next
  labelled fields were A's OTP and B's OTP. Supplying each server's own distinct
  responses recovered both sessions. B's selected-session focus returned to
  its terminal without a click; its typed marker was echoed. Selecting and
  focusing A's terminal also produced its own echoed marker.
- Dropping both transports again, entering disposable text in A's active
  field and pressing Escape cancelled A while immediately showing B's empty
  password field. B completed password/OTP authentication and echoed another
  marker; A remained closed. No remaining A retry prompt appeared before the
  explicit profile reopen.
- Reopening A created a new setup password prompt. Holding its masked response
  while B reconnected again preserved that field. Native menu Quit released
  the original app and local zsh PIDs; both relay transports disconnected about
  252 seconds after fixture startup, before the servers' 300-second deadline.
  No process exit code was captured for the LaunchServices-launched app.

A subsequent bound accessibility observation returned a fresh, local-only
workspace, and process checks found a new app PID. It was closed through native
menu Quit again, with PID checks performed
outside UI afterward. The app's two persisted JSON files (`sessions.json` and
`audit.json`) contained none of the four generated responses or the cancellation
test text. This exact-value scan is not a whole-process zeroization guarantee.

Both Rust labs completed normally with exit code 0. Both app/shell cohorts,
both servers and both relays were verified absent; all four loopback ports
could be rebound and transient fixture/relay response metadata was removed.

This establishes the stated two-session Mac debug ownership, success,
cancellation handoff, focus/input and pending-authentication shutdown workflows.
Native queued expiry/overflow, additional concurrency topologies, the queue in
release copies, Windows/Linux and OpenSSH password/PAM remain separate gates.

### Queued expiry attempt — 2026-10-02

A fresh isolated ARM64 debug copy with the same `f513ead` executable used two
new five-minute, loopback-only authentication fixtures and relays. Both pinned
saved profiles completed their distinct password/OTP logins. Reconnect A was
interrupted first; reconnect B followed 27.25 seconds later. Submitting A's
password opened B's password field while A's OTP waited behind it. B's generated
password was entered without submitting; native accessibility and a screenshot
showed B's label, masked value and focus.

A's second relay connection disconnected 120.04 seconds after it opened,
consistent with the client's whole-authentication deadline. An owned watcher
stopped relay A only after observing that disconnect, to prevent a fresh retry
from obscuring the stale-challenge check. Native observation then failed with
`cgWindowNotFound` for both screenshot/accessibility and accessibility alone.
No further responses were entered. B's field preservation, retirement of A's
queued OTP and B's subsequent successful authentication therefore remain
**unverified**; this attempt does not close the native queued-expiry gate.

Both Rust fixtures and both relays completed with exit code 0. The still-running
isolated app was terminated with SIGTERM after observation failed; this is not
native Quit acceptance. All six owned app/shell/server/relay PIDs were absent,
no test-port listeners remained, and all four loopback ports could be rebound
with `SO_REUSEADDR`. Transient response metadata was removed. Exact-value scans
found none of the four generated responses in persisted `sessions.json` or
`audit.json`; they do not establish whole-process zeroization.

### Native password/OTP check — 2026-10-02

On macOS ARM64, an isolated copy of the debug bundle used a disposable HOME,
portable data directory and a pinned generated Ed25519 host key. The fixture
listened only on `127.0.0.1`; its channel echoed input and explicitly announced
that it provided no OS shell. No local account, personal SSH file, agent or
Keychain credential was used.

Observed through the native app:

- A saved ask-each-challenge profile opened the password field with focus;
  Enter advanced to a separately focused, masked OTP field.
- Escape cancelled the password challenge and displayed the cancellation error.
- A correct generated password followed by a wrong OTP was rejected, with no
  new SSH terminal created.
- Distinct correct password/OTP responses opened a connected SSH terminal.
  After clicking the terminal, a non-secret input marker was echoed back.
- A further unanswered password challenge closed with the timeout error when
  checked 123 seconds after opening; the configured authentication limit is
  120 seconds. The existing authenticated terminal remained connected.
- The three persisted profile/settings/audit files contained neither generated
  response. A fresh run authenticated again, then native menu Quit exited with
  code 0 and released its local zsh child and SSH socket while the fixture's
  loopback listener was still running. This does not establish Quit-shortcut
  timing or other OS-originated exit routes.

These observations establish the stated Mac debug workflows against the
same-stack Rust fixture. Alone, they do not establish OpenSSH/PAM
interoperability, jump-hop/reconnect dialogues, automatic terminal focus after
login, Windows/Linux acceptance or behavior of the published installers.
Quick connect, keyboard cancellation focus and controlled reconnects were
checked separately below.

### Native Quick connect and cancellation focus — 2026-10-02

A fresh macOS ARM64 debug bundle built from `d3b4fb3` passed
`cargo xtask package-check`. Its isolated app copy used disposable HOME/portable
data and the five-minute Rust authentication lab described below. The generated
Ed25519 server listened only on `127.0.0.1`; personal SSH files, agents, Keychain
credentials and the system SSH service were not used.

Observed through native keyboard/mouse actions:

- Quick connect selected **Keyboard-interactive · ask each challenge**, with an
  empty explicit known-hosts path. Submitting without a pin rejected the host
  key and showed its observed fingerprint, without opening a password prompt.
- After entering the fingerprint from the owned fixture metadata, a masked,
  focused password prompt opened. Escape cancelled authentication, left Quick
  connect open and showed the cancellation error.
- With **Connect SSH** reached by Tab and activated by Enter, Escape from the
  challenge restored focus to **Connect SSH**. Enter then retried successfully.
  A separate keyboard-initiated saved-profile attempt restored focus to its
  original profile button after Escape. Existing native HTML dialogue behavior
  handled both cases; no custom focus restoration code was needed. A
  mouse-initiated attempt returned focus to the HTML content, so this does not
  claim button focus after mouse activation.
- Correct, distinct generated password and OTP responses advanced through
  separate masked fields, then opened the optional profile-name prompt. Saving
  a new profile retained `keyboardInteractivePrompt`, the host/port and pin,
  with no credential reference or response in its authentication definition.
- The SSH terminal displayed the **no OS shell** fixture banner and, after a
  terminal click, echoed the non-secret marker `QUICK_CONNECT_AUTH_OK`.
  Automatic terminal focus after login was not established.
- All three persisted profile/settings/audit JSON files were checked against
  both generated responses; neither response appeared. Native application-menu
  Quit exited with code 0. The owned app process was absent and the fixture had
  only its listening socket, with no established SSH socket, before its deadline.
- The lab completed normally after 300 seconds, removed its private metadata
  and released its loopback port.

These observations cover Mac debug Quick connect, saving its challenge-mode
profile, trust-before-prompt and keyboard focus after challenge cancellation.
They do not establish jump-hop/reconnect challenge ownership, OpenSSH/PAM
interoperability, other platforms or updated published installers. Controlled
reconnects were checked separately below.

### Native reconnect password/OTP check — 2026-10-02

The same Mac ARM64 debug source bundle (`d3b4fb3`) was copied into a fresh app
with disposable HOME/portable data, pinned generated trust and a two-attempt
reconnect budget. The Rust authentication server and a TCP relay both listened
only on `127.0.0.1`. The relay interrupted its owned sockets while keeping its
listener available; it did not restart the server or change its host key.
An initial relay configuration error was corrected before the observations
below; that failed harness attempt is excluded from the connection counts.

Observed through the native app, with relay event timestamps/counters:

- A saved challenge-mode profile accepted separate, masked password and OTP
  responses and opened the fixture's **no OS shell** echo channel.
- Interrupting its transport opened a new focused password challenge, followed
  by a separate masked OTP challenge. Correct responses restored connected/LIVE
  state in the existing SSH tab. After a terminal click, the non-secret marker
  `RECONNECT_PASSWORD_OTP_OK` was echoed after the new fixture banner.
- A second interruption opened another password challenge. Escape closed the
  session and left its tab/status/callout **closed / CLOSED / SSH closed**.
  The relay count remained at three accepted connections when checked 13.9
  seconds after disconnection, with only its listening socket remaining.
  Cancellation therefore stopped the remaining configured retry.
- Explicitly selecting the saved profile started connection four and prompted
  for both factors again. Successful recovery opened a fresh connected SSH tab
  while retaining the previous closed tab.
- A third interruption started connection five and opened a password prompt.
  Native application-menu Quit while that prompt was unanswered exited with
  code 0. The recorded app and local zsh child were absent; both relay/server
  had only listeners and no established SSH sockets before fixture shutdown.
- Neither generated response appeared in the three persisted profile/settings/
  audit JSON files. The owned relay was stopped/reaped, its generated credential
  metadata removed and its port rebound successfully. The Rust lab passed after
  300.02 seconds, removed its private metadata and released its port. All recorded
  app, zsh, server and relay PIDs were absent after cleanup.

This covers one successful reconnect, user cancellation, explicit recovery and
Quit with a pending reconnect challenge on Mac debug. It does not prove
jump-hop prompt routing, concurrent sessions, stale-answer UI races, changing
OTP policies, all retry budgets, a real daemon restart, OpenSSH/PAM, other
platforms or behavior of published installers. The existing one-shot broker
and responder-disconnect regressions remain separate automated evidence.

Before a manual Mac GUI check, prepare a fresh application copy:

```bash
cargo xtask package-check
node tools/prepare-macos-ui-lab.mjs
```

The tool prints its owned paths, unique application ID and unchanged executable
SHA-256. It copies the clean debug bundle into a private directory under
`target/`, adds empty portable session data and a **separate launcher bundle**
that clears inherited environment variables before setting disposable HOME/ZDOTDIR,
XDG paths and empty SSH agent/askpass values. Open the printed **`launcherApp`**
first, then select the running native app by its printed **`bundleId`**. The
launcher replaces itself with the native runtime; its own bundle ID can disappear
from the running-app inventory. Check the actual native PID/environment before
interacting with it. Opening `app` or its `mobarust` binary directly bypasses
the environment launcher. Both generated bundles are unsigned and are not
distributable builds.
The tool refuses bundle symlinks and existing portable state, preserves the
source bundle, and starts neither an app nor a listener. An optional argument
selects another clean built Mac app bundle.

Do not rely on `LSEnvironment` alone: during a fresh launch preflight on
2026-10-02, the running app retained the usual HOME and agent socket despite
its disposable bundle settings. It was stopped before starting SSH fixtures.
An explicit environment launcher was then verified on an owned app PID with
disposable HOME/ZDOTDIR/XDG paths and an empty agent socket. Native observation
still reported `cgWindowNotFound`; the owned app and shell were stopped, with
no fixture started. This is isolation evidence, not queued-expiry acceptance.

The preparation regression executes a harmless child through the generated
launcher: inherited HOME/agent/test variables are dropped, paths containing
spaces/apostrophes/shell metacharacters and arguments remain literal, source
bytes stay unchanged, private modes are checked, and unsafe source copies are
refused without deleting previous labs. A second regression compiles a tiny
Foundation metadata probe with the installed Apple compiler. It launches no GUI
and verifies the native main bundle identity through the isolated launcher,
including a competing embedded Mach-O Info.plist. Run them with
`node --test tools/prepare-macos-ui-lab.test.mjs`; it also runs in
`cargo xtask check` and explicitly skips on other platforms.

On 2026-10-03, a harmless Foundation probe launched directly through
LaunchServices reproduced the isolation limitation: disposable ZDOTDIR/XDG
paths were retained, but HOME and SSH_AUTH_SOCK were replaced. The attempted
metadata-only hardening was removed. Native observation can relaunch a stopped
app directly, so do not select or observe it after Quit. Verify the actual PID
and environment again after any unexpected process change; stop only the owned
lab processes before restarting through `launcherApp`.

### Disposable native file-editor lab — 2026-10-03

```sh
cargo test --locked -p mobarust-ssh --test local_sshd \
  native_file_editor_lab -- --ignored --exact --nocapture
```

The opt-in fifteen-minute fixture prints private metadata and a secret-free
profile import file. It generates keys and three editor files, binds sshd only
to `127.0.0.1`, and creates its root and files directory with mode `0700`.
The shared OpenSSH fixture now explicitly starts internal SFTP in that root:
setting shell HOME alone left SFTP's default directory in the account home.
The existing isolation regression checks canonical SFTP `.` and a relative
listing/download as well as shell HOME/ZDOTDIR; it failed before that correction.
This is a working-directory boundary, not a filesystem sandbox or separate user.

Prepare and verify the isolated app first. Import only the generated profile,
operate only on generated files, and record GUI results separately. Close the
owned app normally and verify its children/connections are gone before creating
the metadata's `stop` marker. The harness then reaps its daemon, removes its
directory and verifies its loopback port can be rebound. A passing harness does
not establish editor or native-shutdown acceptance.

### Native lab bundle identity correction — 2026-10-03

An isolated copy of the verified v0.1.21 ARM64 runtime retained the published
executable SHA-256 `60c8a89adb51452890435311164bd79954f15c28b99c5e5ae11bc50545e34128`.
Its actual process had disposable HOME/ZDOTDIR/XDG paths and an empty agent.
Native selection failed with `cgWindowNotFound`, while a process sample showed
AppKit's event loop running. The sample reported identifier `mobarust` and
version 0 instead of the generated lab ID and version 0.1.21.

The earlier tool named its shell wrapper as the native bundle's main executable
but then executed the differently named `mobarust` binary. Apple's
[executable-key documentation](https://developer.apple.com/library/archive/documentation/General/Reference/InfoPlistKeyReference/Articles/CoreFoundationKeys.html)
and [Core Foundation source](https://github.com/apple-oss-distributions/CF/blob/main/CFBundle.c#L686)
describe that metadata relationship and the embedded-info fallback when names
differ. A controlled copy kept `mobarust` as the native main executable and
launched it through a separate environment wrapper bundle. Its sample then
reported the correct generated identifier and `0.1.21 (0.1.21)`; executable
bytes and disposable environment stayed unchanged.

The preparation tool now preserves that native executable/metadata relationship
and exposes `launcherApp` separately. The Foundation regression reproduces the
old helper's fallback to `fixture.embedded` and passes with the corrected helper.
This is a lab-tool correction on main after v0.1.21, not a shipped runtime change.
Both preparation regressions, the complete local `cargo xtask check` and
`cargo xtask pre-push-check` passed on macOS ARM64 with the corrected helper.

The control still failed native window observation. No approval, profile or
authentication action was operated and no SSH listener was started. The startup
marker stayed absent. Owned app/zsh PIDs were confirmed absent after SIGTERM,
then only the generated control directory was removed. Correct bundle identity,
environment isolation and signal cleanup do not establish GUI startup review,
authentication, setup-output behavior or native Quit acceptance.

Before entering credentials or starting fixtures, verify the owned process's
disposable environment and working native accessibility/screenshot observation.
Keep responses out of receipts and shared process-environment dumps. After
Quit, verify owned PIDs outside UI before removing only that lab's generated
directory; a subsequent native observation may relaunch a stopped app.

For a manual native SSH fixture on Unix:

```bash
cargo test --locked -p mobarust-ssh --test authentication native_authentication_lab -- --ignored --nocapture
```

This opt-in lab listens only on `127.0.0.1:0` for five minutes. The printed path
points to private, generated fixture metadata under `target/authentication-native-lab`;
no private key is serialized. It offers two password/OTP rounds and a bounded
echo channel labelled **no OS shell**, with no SFTP or command execution. Use a
disposable app HOME/data directory, and remove only its generated fixture
metadata after stopping the owned test process. Never use the operator's SSH
files, account password, agent or system SSH service.

For a two-bastion native challenge check:

```bash
cargo test --locked -p mobarust-ssh --test authentication native_jump_authentication_lab -- --ignored --nocapture
```

This opt-in lab starts three independently keyed endpoints, listening only on
`127.0.0.1:0`. The printed private metadata paths are `bastion1.json`,
`bastion2.json` and `target.json`, under a disposable directory in
`target/jump-authentication-native-lab`. Each endpoint has distinct generated
password/OTP values. Configure a disposable saved target profile with two
`jump_host_profiles` in that order; use `keyboardInteractivePrompt` and the
matching generated pin at every endpoint. No credential reference is needed.

Each bastion permits `direct-tcpip` only to the next generated loopback
endpoint, with at most eight active forwarding workers per session. It never
resolves a requested hostname or forwards to an arbitrary port. Completed
workers are reaped; dropping their server session aborts remaining workers.
There are at most eight SSH sessions per endpoint, a 180-second inactivity
timeout and a five-minute lab lifetime. Bastions offer no shell; the target
offers only the labelled echo channel. Generated response metadata is mode
`0600` inside a mode `0700` temporary directory, removed on normal lab exit;
private keys are memory-only. The serialized response buffer is zeroizing.

The Mac ARM64 lab run on 2026-10-02 passed after 300.01 seconds. Its three
loopback listeners were observed, the private metadata directory disappeared
on exit and all three ports were rebound successfully. The GUI lab process
was terminated separately because native window observation was unavailable;
this run did not establish native menu Quit or jump-dialogue acceptance.

### Native two-bastion password/OTP check — 2026-10-02

A fresh run restored native window observation in the isolated macOS ARM64
debug app, with disposable HOME/ZDOTDIR and portable data. It used source
`e006aab` for the lab and the debug bundle built from `d3b4fb3` for the unchanged
authentication runtime. Two secret-free saved profiles configured independently
pinned bastion 1, bastion 2 and target endpoints on generated loopback ports.

- The first bastion's password and OTP fields were masked and focused, with
  its configured address in the label. Enter advanced each response. The next
  password prompt showed the second bastion's address; Escape cancelled it,
  displayed the cancellation error and created no SSH terminal. Socket
  inspection then showed only the three fixture listeners.
- With correct factors at both bastions and a deliberately wrong target pin,
  the app displayed host-key rejection with the actual target fingerprint
  before any target password prompt. No SSH terminal was created.
- With all three correct pins, six separately focused masked prompts showed
  the expected hop/target addresses in order. Distinct generated password and
  OTP values opened a connected target terminal. After clicking its input,
  `JUMP_PASSWORD_OTP_OK` was echoed below the **no OS shell** banner. Socket
  inspection showed the three loopback transport connections through the chain.
- Native application-menu Quit removed the recorded app and local zsh PIDs.
  All chain connections were gone while all three fixture listeners remained.
  The app was launched through native UI control, so no process exit-code
  receipt is claimed. None of the six responses appeared in the three
  persisted profile/settings/audit JSON files.
- The Rust fixture passed after 300.01 seconds; its recorded PID disappeared,
  its metadata directory was removed and all three ports rebound successfully.

This run also exposed a metadata-directory permission discrepancy: the default
temporary directory was `0755`, despite the earlier `0700` documentation.
Each generated response file was already `0600`. The owned live directory was
restricted to `0700` before importing profiles. Both native labs now use
`tempfile::Builder::permissions` to request `0700` at creation. The shared
directory regression failed with `0755` before the fix, then passed with
`0700` and verified removal on drop.

These observations cover sequential native hop routing, second-hop cancellation,
target trust-before-prompt, successful authentication and active-chain Quit on
Mac debug. The follow-up focus receipt below covers keyboard input after login
and reconnect. Concurrent dialogue ownership and stale-answer UI races,
OpenSSH/PAM, Windows/Linux and published-installer behavior remain separate
acceptance gates.

### SSH terminal focus after login/reconnect — 2026-10-02

The earlier authentication receipts required clicking the connected terminal.
The frontend now requests focus after the selected SSH session becomes ready
and again after Quick connect's optional save dialogue closes. A deferred
request checks the same terminal instance, current selection and connected
status; removed/hidden panes, open modal dialogues and another active text
field refuse the request. Background reconnects do not select their terminal.
The DOM boundary regression checks deferred ownership changes, hidden/removed
hosts, modal/editable-field refusal and the already-focused terminal input.

A newly built, unsigned debug bundle on macOS ARM64 (base `7f0e5f2` plus this
focus change) passed `cargo xtask package-check`. An executable-matched isolated
copy used disposable HOME/ZDOTDIR and portable data, default settings, and the
generated two-bastion Rust fixture with `0700` directory/`0600` metadata files.
The target offered only the labelled **no OS shell** echo channel.

Observed through native UI control:

- Saved two-bastion login kept all six password/OTP prompts masked and focused.
  After the final Enter, accessibility reported the selected SSH terminal input
  as focused. `AUTO_FOCUS_CHAIN_OK` was typed without clicking the terminal and
  echoed by the target.
- Direct Quick connect used the target's generated pin and ask-each-challenge
  mode. Its save dialogue kept the name field selected while open. Saving
  `Quick focus lab` then focused the new terminal automatically;
  `AUTO_FOCUS_QUICK_OK` was typed/echoed without a terminal click. The saved
  profile had no credential reference or response value.
- A dedicated `127.0.0.1` TCP relay interrupted only its owned SSH sockets.
  Fresh password/OTP authentication restored the same selected terminal and
  focused its input. `AUTO_FOCUS_RECONNECT_OK` was typed/echoed without clicking.
  A second interruption happened with the local zsh terminal selected and
  focused; after the background SSH login completed, the local terminal kept
  both selection and input focus. The relay accepted exactly three connections.
- None of the six generated responses appeared in the two persisted
  profile/audit JSON files. Default settings had no persisted file. Native
  application-menu Quit removed the recorded app/zsh PIDs and all SSH/relay
  connections while the three server listeners and relay listener remained.
  The UI-launched app has no process exit-code receipt. The owned relay was
  subsequently stopped and reaped.
- The Rust lab passed after 300.02 seconds, removed its metadata directory and
  released all three endpoint ports. Those ports and the relay port rebound
  successfully; all recorded app, zsh, server and relay PIDs were absent.

This establishes saved-profile/Quick-connect and one selected/background
reconnect focus path on Mac debug. Other editable/modal focus guards remain
automated DOM-boundary evidence; arbitrary window/tab races, other platforms,
PAM/OpenSSH factors and published-installer behavior are not established.
Frontend unit tests, type checking, lint/build and the full local
`cargo xtask check` passed with this change. Published v0.1.18 installers remain
unchanged.

### Native concurrent-challenge follow-up — 2026-10-02

An isolated copy of the `44a1c41` debug bundle opened two independent sessions,
each with a generated host key/password/OTP and an owned loopback TCP relay.
Both initial native logins succeeded. After interrupting session A, its relay
accepted the reconnect, but native window observation returned
`cgWindowNotFound`; reselecting the confirmed-live app and resetting the UI
binding did not restore observation. No concurrent prompt outcome is claimed.
Native simultaneous prompt ownership and stale-answer UI races remain pending.

The owned app was terminated separately, so this attempt is not a native Quit
receipt. Its recorded app/zsh PIDs and both relay PIDs disappeared. Both Rust
labs passed at their five-minute deadlines (300.00/300.03 seconds), removed
their private generated metadata, and released their ports. All four endpoint/
relay ports rebound; all recorded server PIDs were absent. None of the four
generated responses appeared in the three persisted profile/settings/audit
JSON files. The production-handler regression above proceeds independently of
this missing GUI evidence.

## OpenSSH coverage

| Fixture | Assertions |
| --- | --- |
| Local workstation | Generated Ed25519 host/client keys, rejection of unknown hosts, fingerprint inspection, PTY resize, environment/startup input, SFTP operations/editor conflicts/cancellation, SCP, direct and remote forwarding, concurrent idempotent disconnect |
| Idle connection | Completed SFTP traffic, silence longer than the setup deadline with keepalives disabled, then actual shell execution and a successful exit status |
| Credentials | Encrypted Ed25519 key authentication; missing/incorrect passphrase and unauthorized-key rejection; a successful fresh connection after failure |
| Dedicated Unix agent | Empty-agent and unauthorized-identity rejection, followed by successful signing and shell execution with a generated authorized key |
| IPv6 loopback | Connection to `::1`, an explicit bracketed IPv6 known_hosts entry, and actual shell execution; reports a skip if IPv6 loopback is unavailable |
| Separate jump servers | Two bastions and a target with distinct keys and ports; rejection of an incorrect fingerprint at each hop; target known_hosts mismatch without modifying trust; shell execution and Unicode file round-trip over the entire chain |
| Stalled setup | A TCP peer that never sends an SSH banner; connection timeout and cancellation each close the client socket |
| Interrupted transport / server restart | Cut an owned loopback bridge after authentication and PTY setup; observe channel loss within a deadline, reject a connection while the daemon is stopped, then authenticate and execute a shell after restarting it with unchanged keys and trust |
| Upload commit cancellation | Real SFTP part upload; create-only/overwrite cancellation before preparation and during metadata; closed control channel; preserved original bytes and cleaned parts/backups. [Boundary and limits](transfer-cancellation.md) |
| Session isolation | Remote HOME/ZDOTDIR match the temporary fixture; personal SSH rc/environment files are disabled; PID and X11 authority files stay in the fixture |
| X11 | Native channel bridge to a loopback display; optional real Xvfb setup |

Shell markers are assembled by the remote command, so echoed input cannot
satisfy their assertions. Readers must receive exit status before completing
these command checks. Shell output is bounded and has a total deadline.

Every server gets a temporary directory, generated keys, an explicit
`known_hosts` file, and a loopback port. Its destructor kills/reaps the daemon
and removes the temporary directory. The fixture's server-side `SetEnv`
overrides the OS account home before the shell starts; the X11-only xauth
wrapper forces a disposable authority file even if sshd derives an account
path. This remains an application-level test boundary, not an OS sandbox.
The encrypted-key fixture uses a public test passphrase and one bcrypt round
to keep interoperability checks fast under CPU contention. This is only for
disposable test keys and does not change how operator keys are generated or
loaded.
The agent fixture starts its own foreground `ssh-agent` on a temporary socket.
Only a child test process receives that socket through its environment; the
parallel test harness never changes its own `SSH_AUTH_SOCK`. The child loads
only generated fixture keys, and the parent kills/reaps its agent on success
or child-test failure. No operator agent or identities are queried.
The restart fixture closes only its owned TCP sockets and kills/reaps only its
own daemon. The generated keys, selected port, server configuration, and
`known_hosts` remain unchanged across the restart. Recovery creates a fresh
transport explicitly; it does not exercise the desktop's automatic reconnect
loop or its retry budget.

## SFTP subsystem acceptance on main — 2026-10-03

After v0.1.22, an encrypted loopback regression reproduced a server's explicit
SFTP refusal waiting beyond a two-second observation deadline. Both the main
file channel and the separate directory-listing channel had sent the subsystem
request without consuming its acceptance/failure reply before starting SFTP.
[RFC 4254 sections 5.4 and 6.5](https://www.rfc-editor.org/rfc/rfc4254.html#section-5.4)
define the requested channel reply and subsystem request.

Both paths now use the same acceptance guard. Refusal returns the static,
actionable `SFTP subsystem` rejection error; early EOF/close returns the distinct
closed-before-acceptance error. No SFTP INIT is sent to an unaccepted subsystem.
The existing 12-second setup budget covers channel opening, acceptance and
version negotiation. A pending-channel guard uses russh's stream Drop cleanup on
refusal, timeout or caller cancellation: it retires the reader before scheduling
best-effort Close. The desktop delegates to this owned setup instead of cancelling
it with a second outer timer. Cleanup depends on a live Tokio runtime/transport;
it is not a force-kill guarantee.

Early protocol data stays ordered through a Tokio reader/writer join, with the
existing 1 MiB setup-output bound. Extended data does not enter SFTP or error
messages. There is no new dependency or server-selected error text.

```sh
cargo test --locked -p mobarust-ssh --test authentication sftp_setup_requires_server_acceptance_on_both_channels
cargo xtask check
```

The wire regression exercises refusal, early Close, silence, caller cancellation, setup-output flood,
ordinary acceptance and fragmented version bytes before acceptance on both
channels. It verifies failed-channel cleanup before transport disconnect, no
premature INIT, static errors, worker completion and loopback listener release.
Cancellation waits for the peer to receive the subsystem request before dropping
setup; a fresh echo shell verifies that the same transport remains usable.
Its generated password/host key are memory-only; its tiny SFTP peer has no
filesystem and deliberately refuses OPENDIR. It does not run OS shell commands,
access personal SSH state or listen outside `127.0.0.1`.

The corrected regression and final complete `cargo xtask check` passed on macOS
ARM64 / Apple M2: 105 desktop tests, 42 SSH unit tests, 15 automated authentication
tests (three manual labs ignored), 16 OpenSSH cases, workspace Clippy, frontend
checks, release tooling, helper fixtures, package contracts and fuzz compilation.
Real Xvfb remained an explicit prerequisite skip.

An earlier full run stopped at the X11 bridge fixture's five-second channel wait,
after shell input was echoed. The same-source isolated case passed in 2.32 seconds,
and the unchanged full rerun passed. Its cause is unproven; no X11 assertion or
deadline was relaxed, and this is not an X11 correction or native GUI receipt.

During v0.1.23 preparation, the same five-second fixture wait failed again.
A diagnostic run recorded `xauth` starting without an exit marker before the
deadline; direct isolated xauth probes completed normally. This narrows the
observed setup stage without proving the cause. The fixture wrapper now relays
the original commands unchanged and records only start, command class, input
EOF and exit status in its private disposable directory. Failure output includes
these stages, never the display name or generated cookie. The diagnostic SSH
suite passed; this is observability evidence, not an X11 runtime fix. The
five-second channel and bridge deadlines and all bridge assertions are unchanged.
The final v0.1.23 source, without the temporary profiler, passed the complete
local `cargo xtask check`, including this X11 case. The intermittent cause is
still unproven and no native GUI acceptance is inferred from that pass.

During v0.1.24 preparation, the first full check failed at both the five-second
X11 wait (xauth had input EOF but no exit marker) and the 60-second frequent-rekey
8 MiB routed-transfer deadline. A same-default-runtime diagnostic SSH run passed.
It measured eight workers per test runtime and a sampled peak of 77 native test
process threads. Sampling only a test-owned xauth process showed Rosetta/dyld
startup; `/opt/X11/bin/xauth` here contains Intel architectures only. A two-worker
experiment reduced the sampled peak to 28 threads but still failed both gates;
it was removed. No thread cap, payload reduction, deadline extension or assertion
change remains in source.

A native `/opt/homebrew/bin/xauth` (arm64, 1.1.5) was then installed explicitly
with four small client-library dependencies. A private Unix-display add/remove
probe returned zero in 0.94 seconds with a generated, unrecorded cookie and a
0600 authority file in a disposable HOME. The test itself never installs tools.
This narrows an external-helper architecture/startup issue; it does not prove the
cause of every prior failure or represent an X11 product/runtime correction.

The version-aligned v0.1.24 source then passed the complete local
`cargo xtask check` with the original runtime scheduling, payload assertions and
deadlines. All 16 OpenSSH cases passed, including X11 and frequent rekey; the
OpenSSH suite took 58.84 seconds. This single full recheck does not establish
repeatable timing, sustained throughput or native GUI acceptance.

This is backend protocol evidence. Native file-browser/editor acceptance,
external server implementations and Windows/Linux runtime checks remain open.
Both [v0.1.23 Mac previews](../release/v0.1.23.md) include this later source
correction. Older Mac and v0.1.12 Windows/Linux installers do not contain it.
GitHub CI remains disabled.

## Limits and next interoperability gates

The OpenSSH fixtures exercise the installed server version on the current machine.
They do not prove Windows SSH-server compatibility, OpenSSH password or PAM/MFA
authentication (see the separate Rust-wire coverage above), Windows Pageant or
other agent implementations, routed IPv6,
RSA support, automatic GUI reconnect/retry-budget behavior, sustained
terminal/UI performance, or internet-host compatibility.
RSA remains disabled under the existing advisory policy.

Record the OS/architecture, OpenSSH version, Git commit, exact command, result,
and skipped optional fixtures when adding platform evidence. Use the
[hardware/interoperability matrix](hardware-interoperability.md) for external
servers and real-device checks.

## Separate native desktop retry receipt

The Rust restart fixture above does not drive the GUI. A separate isolated
macOS ARM64 app/loopback relay check observed a two-attempt budget exhaust,
explicit saved-profile recovery, accurate error/closed labels, and native Quit
releasing an active SSH session plus its local PTY. See the [native receipt and
remaining gates](ssh-reconnect.md). Zero/maximum GUI budgets, flapping shells,
actual daemon-restart GUI recovery and other platforms remain open.

## v0.1.19 ARM64 release-copy authentication — 2026-10-02

Both Mac installers were built locally from `084a3c1` after the complete
`cargo xtask check` passed. The ARM64 DMG passed integrity, read-only mounted
layout, strict ad hoc signature, app/helper architecture and CLI version checks.
A disposable app copy used a unique identifier, isolated HOME/ZDOTDIR and
portable data. Its changed launch metadata required an ad hoc test signature;
additional copies of the final-DMG and test executables were compared after
removing their signatures and were byte-identical. The original DMG was not
modified. This is an isolated release-runtime check, not clean installation.

Native UI control observed:

- A saved profile with a deliberately wrong generated pin displayed host-key
  rejection and the observed fingerprint before any password prompt.
- The correct-pin profile showed focused masked password and OTP fields.
  Escape at OTP displayed authentication cancellation and created no SSH tab.
  Socket inspection showed only the fixture and relay listeners afterwards.
- A fresh attempt with the generated factors connected successfully. The SSH
  input was focused automatically, and `V019_RELEASE_LOGIN_OK` was typed and
  echoed without clicking the terminal beneath the **no OS shell** banner.
- A controlled drop of the owned relay's sockets requested fresh password/OTP
  factors. The same selected tab retained its scrollback and focused input;
  `V019_RELEASE_RECONNECT_OK` was typed and echoed without a focus click.
- Native application-menu Quit removed the recorded app and local zsh PIDs.
  SSH connections disappeared while both owned listeners remained live.
  Generated password/OTP values were absent from the two persisted profile/audit
  JSON files. There was no saved settings file or credential reference.

The already-checked Rust authentication harness ran its opt-in
`native_authentication_lab --ignored --exact --nocapture` fixture with a
sanitized environment. It passed after 300.02 seconds, removed its private
metadata directory and released its listener. The owned relay exited normally
after SIGTERM, removed its response metadata and accepted four connections
(wrong pin, cancelled login, successful login and reconnect). All recorded
app, shell, server and relay PIDs were absent; both ports rebound successfully.
Every server/relay endpoint bound only `127.0.0.1`; no system SSH service or
personal SSH configuration was used. UI launch provides no process exit-code
receipt for the app.

This closes the release-copy gap for these direct Mac authentication/focus
flows. Concurrent native requests, release-copy two-bastion/Quick-connect
acceptance, OpenSSH password/PAM, Windows/Linux, sustained workloads and
clean-install/signing gates remain open. The earlier debug receipts and
production-handler DOM ownership tests are separate evidence.
