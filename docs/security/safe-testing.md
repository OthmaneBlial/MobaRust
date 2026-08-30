# Safe local testing policy

MobaRust protocol tests must be safe to run on a developer workstation. A
green test run is not permission to inspect or alter the developer's personal
configuration.

## Hard boundaries

- Tests use temporary directories created for the test process and remove only
  files they created.
- Network fixtures bind to `127.0.0.1` on an OS-assigned port. Tests do not use
  public hostnames, LAN addresses, cloud instances, or production endpoints.
- SSH tests generate fixture host/client keys in the temporary test directory.
  They pass an explicit fixture `known_hosts` path and never read `~/.ssh`,
  `SSH_AUTH_SOCK`, GitHub keys, or a user's private key.
- OpenSSH import requires an explicit path and never falls back to
  `~/.ssh/config`; tests and development runs must use a repository fixture or
  an isolated temporary file.
- SSH connection setup never falls back to `~/.ssh/known_hosts`. An explicit
  known-hosts path or pinned fingerprint is required for trust; otherwise the
  observed key is rejected without reading a personal file.
- RDP/VNC tests use protocol-independent fixtures or an explicitly approved
  disposable server. The isolated RDP helper can be compiled and exercised on
  EOF without opening a socket; the VNC integration fixture binds only to
  `127.0.0.1` and an OS-assigned port.
- Serial tests use configuration and lifecycle logic only. Hardware
  interoperability requires a separate, explicit manual test session.
- WSL discovery and launch are compiled conditionally for Windows. macOS/Linux
  tests do not invoke `wsl.exe`; the parser is tested with synthetic UTF-16
  output and no local distribution is probed.
- Tests do not install system packages, alter shell profiles, change firewall
  rules, modify keychains, write outside the repository/temp directories, or
  send credentials to a real host.

## Review checklist for new tests

Before merging a new integration test, verify that it:

1. has an explicit target address and timeout;
2. uses a temporary fixture path for keys, known-hosts, databases, and files;
3. uses synthetic credentials held only for the test lifetime;
4. does not inherit or query the SSH agent when agent behavior is not the
   subject of the test; fixture child processes remove agent and Git SSH
   environment variables explicitly;
5. cleans up child processes and listening sockets on success, failure, and
   cancellation;
6. does not print passwords, private keys, tokens, or full environment values.

## Safe commands

The default local quality command is repository-scoped:

```text
cargo xtask check
```

Its test subprocesses also remove ambient SSH-agent, askpass, and Git SSH
configuration variables. The check deliberately keeps the normal Cargo/rustup
cache environment, so this is an application-level safety boundary rather than
an operating-system sandbox.

The isolated RDP helper check is also repository-scoped:

```text
cargo xtask check-rdp-helper
```

The isolated VNC helper check is repository-scoped:

```text
cargo xtask check-vnc-helper
```

The synthetic benchmark probe is repository-scoped as well:

```text
cargo xtask benchmark
```

It uses in-memory fixtures only; it does not open sockets or inspect local
configuration, credentials, SSH directories, or attached devices.

The helper EOF smoke test below emits its native handshake and exits without
connecting:

```text
cargo run --manifest-path tools/rdp-helper/Cargo.toml -- \
  --mobarust-protocol rdp --host 127.0.0.1 --port 3389 \
  --username fixture-user < /dev/null
```

These guarantees cover MobaRust's test behavior. They cannot protect secrets
from a fully compromised operating system, a malicious process with the same
user privileges, or an operator who explicitly exports or pastes a secret.
