# Local OpenSSH integration lab

Run from the repository root:

```bash
cargo xtask test-ssh
```

The command runs SSH transport unit tests and the disposable OpenSSH fixtures
with an isolated HOME/XDG environment and without an inherited SSH agent or
askpass. It uses the existing Rust test harness; Docker, system-service changes,
personal SSH files, and remote servers are unnecessary.

## Requirements

- macOS or Linux, Rust, `ssh-keygen`, `ssh-agent`, `ssh-add`, and an installed OpenSSH `sshd` at
  `/usr/sbin/sshd` or `/usr/local/sbin/sshd`;
- an existing, unlocked, non-root OS account (`USER`) that can run its shell;
- `xauth` on PATH for the loopback X11-channel test;
- optional Xvfb and an existing real `/tmp/.X11-unix` directory for the real
  X11-server test. That test reports a skip when its prerequisites are absent.

Tests do not create users, set account passwords, install dependencies, or
enable Remote Login. A missing prerequisite is a lab setup failure; configure
a dedicated test runner rather than granting the tests system permissions.
On Windows, use `cargo test --locked -p mobarust-ssh --lib` for portable unit
coverage; `test-ssh` reports that the OpenSSH fixture requires a Unix host.

## Coverage

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
They do not prove Windows SSH-server compatibility, password or PAM/MFA
authentication, Windows Pageant or other agent implementations, routed IPv6,
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
