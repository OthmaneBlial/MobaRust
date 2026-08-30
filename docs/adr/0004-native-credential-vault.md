# ADR 0004: Keep credential material in a native vault

## Status

Accepted and implemented for the native credential boundary and explicit
reference save/delete flow.

## Decision

Session records store opaque credential references. The Rust `mobarust-vault`
crate owns access to the platform credential store through `keyring`:

- macOS Keychain
- Windows Credential Manager
- Secret Service on supported Unix desktops

Passwords, private keys, passphrases, and tokens are never serialized into the
session snapshot sent to React. Native operations receive a reference and load
the secret only for the operation that needs it. Vault-owned buffers move into
the SSH secret wrapper without an extra plaintext clone; both wrappers redact
`Debug` output and zeroize owned in-memory strings on drop where possible. SSH
private-key files are bounded to 4 MiB and loaded through a zeroizing native
byte buffer before parsing; the third-party private-key parser remains a
separate release-review boundary.
Platform lookups and writes, portable-vault entries, and portable-vault
passphrases are bounded to 1 MiB before backend use or key derivation to prevent
accidental large secret allocations from becoming a native denial-of-service
path.
Keyboard-interactive SSH servers are also limited to eight non-echo prompts per
request before the response is copied for the native library, preventing a
remote server from multiplying a secret into unbounded response allocations.

## Consequences

The frontend can display whether a session references credentials without
being able to read them. A compromised local process with the same user
privileges remains in scope: an OS credential store is not a defense against a
fully compromised desktop account. Clipboard, crash-dump, export, and logging
policies still need separate controls.

Portable mode is not allowed to fall back to plaintext JSON. The separate
encrypted portable backend is now implemented with an explicit passphrase,
Argon2id + AES-256-GCM, native lock/unlock lifecycle, and repository tests;
signed distribution and recovery UX remain separate release gates.

The desktop now exposes only typed `vault_put` and `vault_delete` commands.
Saving is an explicit action: the transient secret is accepted by the native
command, written to the platform store, and never returned to the renderer.
The credential modal clears its input after the operation and does not list or
export stored secrets. Tests deliberately avoid the platform backend so they
cannot write to a developer's Keychain, Credential Manager, or Secret Service.
