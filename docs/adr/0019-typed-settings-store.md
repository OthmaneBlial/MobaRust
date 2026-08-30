# ADR 0019: Keep typed settings separate from session and secret storage

## Status

Accepted and implemented for the desktop settings slice.

## Decision

Application preferences use typed Rust structures grouped by general,
appearance, terminal, SSH, and network concerns. They are persisted in a
versioned `settings.json` beside, but separately from, the session catalog.
The file contains no credential values or credential references.

Settings are validated before replacement and written through a durable
temporary-file-plus-rename path. Newly created settings files use owner-only
mode `0600` on Unix. On Windows, replacement uses the OS
replace-existing move primitive rather than deleting the destination first.
Unknown top-level fields and unsupported
schema versions fail loudly so an upgrade cannot silently discard or replace
configuration. Reset is an explicit command that writes the documented safe
defaults.

The renderer receives and submits the typed settings object through three
narrow Tauri commands: `settings_get`, `settings_save`, and `settings_reset`.
The native layer remains the source of truth. Terminal font size, scrollback,
cursor blink, and multiline-paste confirmation are applied to new terminal
instances; invalid numeric ranges are rejected in Rust.

All persisted JSON catalogs are opened through a regular-file, bounded reader
with a 64 MiB safety limit before deserialization. The OpenSSH import path uses
its stricter 1 MiB limit because it parses an explicitly selected external
configuration file.

## Security boundary

Settings are non-secret preferences. Passwords, private keys, passphrases,
tokens, and environment values do not belong in this file. Portable mode must
not reuse this plaintext settings mechanism for credential storage; it needs a
separate encrypted-vault design.

## Verification

Core validation tests cover safe defaults and invalid ranges. Store tests cover
round-trip persistence, reset durability, and refusal to replace a corrupt
file. The desktop quality command also runs Tauri compilation, Rust tests,
TypeScript checks, lint, and the production Vite build.
