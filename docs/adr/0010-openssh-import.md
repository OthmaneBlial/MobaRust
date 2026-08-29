# ADR 0010: Import OpenSSH profiles as secret-free session definitions

## Status

Accepted and implemented for the common OpenSSH fields.

## Decision

MobaRust imports a user-selected OpenSSH config through a dedicated native
command. The importer recognizes:

- `Host` exact aliases;
- `HostName`, `User`, and `Port`;
- the first `IdentityFile` as a private-key reference;
- comma-separated `ProxyJump` entries as jump-host references;
- `ServerAliveInterval` as a visible note until the native keepalive model is
  implemented.

Wildcard aliases, negated patterns, malformed ports, and unsupported
directives are not silently converted into a profile. The report contains
skipped hosts and distinct unsupported directive names. `Host *` is treated as
a defaults block; exact aliases receive the first value found for each option,
matching the important precedence rule without claiming full OpenSSH parser
compatibility.

Imported profiles are persisted in the existing versioned session store and
are idempotent by protocol and alias. A repeated import updates the existing
profile while preserving its session ID. `IdentityFile` remains a path
reference and passwords are never read from the config or stored. Profiles
with `ProxyJump` are retained for migration visibility but the current UI
blocks reconnect with an explicit message until jump-host transport exists.

## Rationale

- OpenSSH config is a high-value, user-controlled migration source.
- A dedicated parser keeps the IPC contract narrow and avoids generic file
  access from React.
- An import report makes partial compatibility reviewable.
- Idempotency prevents repeated imports from flooding the session catalog.

## Rejected for this milestone

- importing `IdentityFile` contents or passphrases;
- pretending wildcard/pattern matching is fully supported;
- executing `ProxyJump` through a shell string;
- reading arbitrary config includes recursively without a separate path and
  security review;
- claiming imported `ServerAliveInterval` changes runtime behavior.

## Verification

Store tests cover global defaults, aliases, key and jump references, notes,
unsupported directives, malformed ports, secret absence, persistence, and
repeat-import idempotency. Desktop TypeScript and Rust command compilation
must also pass before release.
