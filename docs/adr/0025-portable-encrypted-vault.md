# ADR 0025: Gate portable mode and encrypt its credential vault

## Status

Accepted and implemented for the native backend, marker-gated data directory,
explicit unlock/lock commands, and credential lookup integration. Signed
portable packaging and cross-platform distribution evidence remain pending.

## Decision

Portable mode activates only when a regular `portable.flag` file is present
beside the executable. Symbolic-link markers are rejected so a portable launch
cannot redirect its data root through an unexpected filesystem target. In that
mode, the application catalog and settings are placed in a `portable-data`
directory beside the executable; otherwise the normal Tauri application-data
directory is used. The marker prevents a normal development or installed run
from silently switching storage roots.

Portable credentials are stored in a separate `vault.bin` file. The file uses
Argon2id to derive a 256-bit key from an explicit passphrase and AES-256-GCM
for authenticated encryption with a random salt and nonce. The passphrase is
never persisted. The unlocked native vault retains only the derived key and
zeroizing secret wrappers; lock drops the entire native vault state.

Mutations use a private temporary file, `sync_all`, and atomic replacement. On
Unix, the temporary vault file is created with owner-only mode `0600` before
replacement. On Windows, the final replacement uses the OS replace-existing
move primitive with write-through semantics instead of deleting `vault.bin`
first. The backend refuses oversized files, oversized secrets, invalid credential IDs,
unknown payload fields, unsupported schema versions, and tampered ciphertext.
React receives only status, opaque IDs, and actionable errors. It does not
receive a vault listing's secret values or a get-secret command.

On Unix, vault reads also open with no-follow and nonblocking flags so a local
path swap cannot redirect the read through a symlink or leave the native
operation waiting on a special file. Other platforms retain the regular-file
check and bounded read path while using their native file-opening semantics.

## Consequences

Portable operation works without silently replacing the platform keyring, but
it requires an explicit passphrase and careful user lock/unlock behavior. A
portable vault protects data at rest from ordinary file disclosure; it cannot
protect an unlocked process or a malicious local process with equivalent user
privileges. Packaging must ship the marker deliberately and must not create it
implicitly.

## Verification

Repository tests create temporary vault files only. They verify encrypted
round-trips, absence of fixture passphrases and secrets in the file, wrong
passphrase rejection, tamper rejection, delete semantics, atomic writes, and
the native SSH credential lookup boundary. No platform keychain, personal
filesystem, SSH key, or real host is used by the tests.
