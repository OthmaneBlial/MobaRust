# ADR 0011: Chain SSH handshakes over native direct-tcpip channels

## Status

Accepted and implemented for explicitly described jump hops and saved-profile
reconnection when each imported alias resolves to a saved SSH profile.

## Decision

The SSH transport supports a target `SshConnectOptions` plus an ordered list
of jump options. It connects and authenticates the first hop directly, opens
one `direct-tcpip` channel to the next hop, runs the next SSH handshake over
that channel, and repeats until the target is reached. The resulting target
connection retains its parent chain so the underlying transports remain alive.

Each hop has its own host, port, username, authentication reference, timeout,
and host-key policy. The native manager resolves those typed descriptors and
never builds a shell command or passes secrets through React. A target shell,
SFTP channel, or local forward uses the final connection exactly as it does for
a direct SSH session.

Quick Connect exposes one optional agent-backed hop. The transport API accepts
multiple hops so deterministic native tests and a later saved-profile editor
can support multi-hop chains without changing the protocol boundary. Saved
Quick Connect profiles retain non-secret hop descriptors. OpenSSH imports keep
`ProxyJump` aliases, and the renderer resolves them only against the imported
secret-free catalog before creating the typed hop requests.

Connection requests allow up to eight jump hosts. Oversized OpenSSH entries
are reported as skipped while other entries import normally. Existing saved
profiles remain readable; an oversized chain cannot start a connection.

## Safety and lifecycle

Jump tests start an ephemeral `sshd` in a temporary directory, generate
throwaway Ed25519 keys there, and use a temporary `known_hosts` file. They do
not read `~/.ssh`, use the system SSH agent, modify personal keys, or contact
an internet host. Parent connections are closed iteratively after the target
connection, and failures are bounded by per-hop timeouts.

## Rejected for this milestone

- invoking `ssh -J` through a shell;
- accepting unknown keys on a bastion for convenience;
- copying one target credential into every hop;
- silently inventing credentials or ports for an alias that is not present in
  the saved catalog;
- using a shell-based `ProxyJump` fallback when a typed hop cannot be resolved.

## Verification

`cargo xtask test-ssh` exercises `crates/mobarust-ssh/tests/local_sshd.rs` with
two distinct ephemeral jump servers and a separate target. It rejects a wrong
fingerprint at each hop, verifies target shell execution and exit status, and
round-trips a Unicode-named file over SFTP through the chain. Each server has
its own generated host/client keys and temporary trust file. Rust Clippy and the desktop TypeScript/build
checks remain required for the desktop integration.
