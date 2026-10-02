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

## OpenSSH fixture requirements (Unix)

- macOS or Linux, Rust, `ssh-keygen`, `ssh-agent`, `ssh-add`, and an installed OpenSSH `sshd` at
  `/usr/sbin/sshd` or `/usr/local/sbin/sshd`;
- an existing, unlocked, non-root OS account (`USER`) that can run its shell;
- `xauth` on PATH for the loopback X11-channel test;
- optional Xvfb and an existing real `/tmp/.X11-unix` directory for the real
  X11-server test. That test reports a skip when its prerequisites are absent.

Tests do not create users, set account passwords, install dependencies, or
enable Remote Login. A missing prerequisite is a lab setup failure; configure
a dedicated test runner rather than granting the tests system permissions.
On Windows, `test-ssh` runs portable unit and authentication-wire tests and
reports that the OpenSSH fixtures are skipped. The wire cases contain no
Unix-specific APIs; their execution has so far been verified on macOS ARM64.
The opt-in native labs and their metadata-permission regression are Unix-only.

## Portable authentication-wire fixture

```bash
cargo test --locked -p mobarust-ssh --test authentication
```

`tests/authentication.rs` starts a single-connection `russh` server on
`127.0.0.1:0` for each case, using generated memory-only Ed25519 keys and
zeroizing disposable credentials. The client pins the generated fingerprint.
There are no OS accounts, PAM changes, agent requests, credential files,
subprocesses, or new dependencies. Each test uses its own Tokio runtime; it
awaits server-session termination and rebinds the released listener address.
The server has no inactivity timeout, so that cannot satisfy the client-socket
cleanup assertion. Stalled authentication callbacks are explicitly released
after timeout/cancellation to let the server observe closure.

Eight tests cover:

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

Two native broker regressions cover bounded/one-shot answers, expired waiter
cleanup, close-before-drop refusal, reconnect cancellation and shutdown.
DOM boundary checks cover password masking, plain-text server labels, abort
cancellation and clearing the input; these are not native keyboard/focus proof.
The native receipts below cover saved profiles, Quick connect, controlled
reconnects, two-bastion prompt routing and two-session queued prompt ownership
on Mac debug. Native queued expiry/overflow, broader reconnect cases and
OpenSSH/PAM interoperability remain separate gates. The ask-each-challenge mode
is included in the v0.1.19 Mac
previews; Windows/Linux remain v0.1.12. The concurrent frontend queue is
post-v0.1.19 source work and is not in the published installers.

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

For a manual native GUI check on Unix:

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

## Limits and next interoperability gates

These tests exercise the installed OpenSSH version on the current machine.
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
