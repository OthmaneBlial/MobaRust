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

Five tests cover:

- Password acceptance/rejection, connected lifecycle state, explicit disconnect
  and session/socket cleanup.
- Keyboard-interactive acceptance/rejection, zero/one/eight non-echo prompts,
  and a two-round challenge with the same response in each prompt.
- Echo-enabled and nine-prompt challenge refusal before any response is sent.
- Host-key rejection before password or keyboard-interactive callbacks run.
- Timeout and task cancellation after each authentication method has started,
  followed by session/socket cleanup.

These are actual SSH handshakes and encrypted authentication packets, with the
same Rust stack at both ends. They do not establish OpenSSH password/PAM/MFA
interoperability. The existing static keyboard-interactive credential repeats
one secret for all non-echo prompts and rounds; distinct password-plus-OTP or
user-selected responses are not supported by this path.

Verified 2026-10-02 on macOS ARM64 with Rust 1.95.0 and `russh` 0.63.1:
the focused command passed all five tests. `cargo xtask test-ssh` passed 41
unit, five authentication-wire and 12 OpenSSH tests; the real Xvfb case reported
a prerequisite skip, while loopback IPv6 executed.
The ordinary `cargo xtask test-ssh` and workspace suite include this fixture.

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
