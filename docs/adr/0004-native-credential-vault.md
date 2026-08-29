# ADR 0004: Keep credential material in a native vault

## Status

Accepted for the first credential-storage slice.

## Decision

Session records store opaque credential references. The Rust `mobarust-vault`
crate owns access to the platform credential store through `keyring`:

- macOS Keychain
- Windows Credential Manager
- Secret Service on supported Unix desktops

Passwords, private keys, passphrases, and tokens are never serialized into the
session snapshot sent to React. Native operations receive a reference and load
the secret only for the operation that needs it. Rust wrappers redact `Debug`
output and zeroize owned in-memory strings on drop where possible.

## Consequences

The frontend can display whether a session references credentials without
being able to read them. A compromised local process with the same user
privileges remains in scope: an OS credential store is not a defense against a
fully compromised desktop account. Clipboard, crash-dump, export, and logging
policies still need separate controls.

Portable mode is not allowed to fall back to plaintext JSON. An encrypted
portable vault requires a separate design, key lifecycle, recovery story, and
tests before portable mode can claim parity with native storage.

The current crate is a storage boundary, not yet a complete saved-session UI
flow. Wiring references into session CRUD and typed Tauri commands is the next
step.
