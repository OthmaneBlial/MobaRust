# ADR 0027: Keep SSH host-key inspection unauthenticated and explicit

## Status

Accepted and implemented for the diagnostics surface.

## Context

Administrators need a safe way to compare an SSH server's presented SHA-256
host-key fingerprint before saving or changing trust policy. This is a
different operation from opening an authenticated session and must not quietly
become trust-on-first-use.

## Decision

Expose a one-shot native `inspect_host_key` operation with only:

- an explicitly entered host;
- an explicitly entered port; and
- a bounded timeout.

The operation performs the SSH handshake, records the server fingerprint, and
disconnects immediately. It has no username, password, private-key path,
passphrase, agent access, jump-host chain, or known_hosts path. The public
authenticated connection policy has no accept-any variant; observation mode is
private to this operation.

The renderer receives only the host, port, and observed fingerprint. It never
receives credentials, and inspection never writes `known_hosts` or any trust
record.

## Safety and verification

Invalid hosts, port zero, control characters, zero timeouts, and timeouts over
60 seconds are rejected. The UI labels the result as an observation rather
than a security audit. The integration fixture uses a temporary local `sshd`,
generated fixture keys, and `127.0.0.1`; it compares inspection output with the
same fingerprint that the normal known-host rejection reports.

This does not replace strict known-host or pinned-fingerprint verification for
real authenticated sessions.
