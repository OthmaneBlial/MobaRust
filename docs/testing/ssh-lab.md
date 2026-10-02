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
reports that the OpenSSH fixtures are skipped. The new fixture contains no
Unix-specific APIs; its execution has so far been verified on macOS ARM64.

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

Seven tests cover:

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

These are actual SSH handshakes and encrypted authentication packets, with the
same Rust stack at both ends. They do not establish OpenSSH password/PAM/MFA
interoperability. The existing static keyboard-interactive credential repeats
one secret for all non-echo prompts and rounds; distinct password-plus-OTP or
user-selected responses are not supported by this path.

Verified 2026-10-02 on macOS ARM64 with Rust 1.95.0 and the
[repository-local `russh` 0.63.3 patch](../../vendor/russh/MOBARUST_PATCH.md):
the workspace suite passed all seven automated tests; the opt-in native fixture
is ignored by default. The earlier static-response receipt
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
Concurrent UI dialogues fail closed rather than replacing an active prompt.

Two native broker regressions cover bounded/one-shot answers, expired waiter
cleanup, close-before-drop refusal, reconnect cancellation and shutdown.
DOM boundary checks cover password masking, plain-text server labels, abort
cancellation and clearing the input; these are not native keyboard/focus proof.
The remaining native GUI coverage, jump-hop/reconnect challenge acceptance
and OpenSSH/PAM interoperability remain separate gates. This mode is post-v0.1.18
source work; the published installers do not contain it.

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

This establishes these Mac debug workflows against the same-stack Rust fixture.
It does not establish OpenSSH/PAM interoperability, jump-hop/reconnect dialogues,
automatic terminal focus after login, Windows/Linux acceptance or behavior of
the published installers. Quick connect and keyboard cancellation focus were
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
Jump-hop/reconnect challenge ownership, OpenSSH/PAM interoperability, other
platforms and updated published installers remain separate acceptance gates.

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
