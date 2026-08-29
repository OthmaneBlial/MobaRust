# ADR 0003: native russh transport for SSH/SFTP

## Status

Accepted for the SSH foundation; the UI adapter and jump-host layer remain follow-up work.

## Decision

Use the actively maintained `russh` transport and `russh-sftp` subsystem from a dedicated `mobarust-ssh` crate. Rust owns TCP connection setup, host-key verification, authentication, PTY channel setup, SFTP I/O, lifecycle state, and timeout handling.

The default host-key policy reads OpenSSH `known_hosts` and rejects unknown keys. A pinned SHA-256 fingerprint is an explicit alternative for a user-confirmed key. Unknown keys return the observed fingerprint so the UI can build a deliberate confirmation flow later; there is no silent trust-on-first-use path.

SFTP file movement uses `tokio::io::copy` between async readers and writers. The API never reads a complete remote file into a `Vec<u8>`. Dropping a cancelled future releases the in-flight operation; a transfer manager will add visible cancellation and bounded concurrency above this primitive.

## Rejected alternatives

- shelling out to the system `ssh` client: it would make lifecycle, PTY, host-key UX, and transfer cancellation harder to own consistently;
- accepting every server key: it violates the threat model and makes a man-in-the-middle indistinguishable from first contact;
- putting passwords in the session model: it would duplicate secret material and cross the React boundary.

## Evidence

`crates/mobarust-ssh/tests/local_sshd.rs` starts a temporary OpenSSH server, proves an unknown host key is rejected, authenticates using a generated Ed25519 key, opens an interactive PTY, and streams an SFTP upload/download without using a whole-file buffer.
