# MobaRust threat model

This document describes the security boundary for the first vertical slice and the constraints future protocol work must preserve.

## Assets

- passwords, private-key passphrases, agent handles, and future vault entries;
- session configuration, host fingerprints, jump-host topology, and imported profiles;
- terminal output, remote filenames, transfer content, and diagnostic exports;
- local PTY processes and the user's clipboard.

## Trust boundaries

1. **Rust/native boundary** — owns PTYs, network connections, credential access, process lifecycle, and future vault integrations.
2. **React/webview boundary** — receives display-safe state and terminal output only; it is not a secret store and has no general shell/filesystem command.
3. **Remote host boundary** — terminal output and filenames are untrusted input. They must never become HTML or an implicit command.
4. **Local OS boundary** — malicious local processes, crash reporters, backups, and clipboard managers may observe data outside MobaRust's control.

## Threats and mitigations

| Threat | Current policy / planned control |
| --- | --- |
| Plaintext password exposure | Session records contain credential references only. Password acquisition stays native, secret material is bounded before vault/backend use, keyboard-interactive response fan-out is capped before response strings are created, and it is not serialized in diagnostics. |
| Private-key exposure | Keys are referenced by path and never copied into session metadata. Native SSH loading bounds the selected file and reads it through a zeroizing byte buffer before handing text to the third-party parser. Passwords and passphrases move from the native vault into zeroizing SSH buffers without an extra plaintext clone; the parser's loaded-key memory hygiene remains a release-review gate. |
| Session environment exposure | Environment entries are bounded, validated, and treated as potentially sensitive configuration. They are applied natively to the SSH channel, excluded from `Debug` output/logs, and the UI warns that passwords/tokens belong in the vault instead. |
| Malicious local process | Documented as an OS limitation; minimize plaintext lifetime and never claim protection from a process with equivalent user privileges. |
| Compromised application database | The current session store contains metadata and opaque references only. Native session, settings, snippet, macro, and audit files are bounded before parsing. Platform vault entries are separate; portable credentials use a separate encrypted vault file and remain unavailable while locked. |
| Logs and crash dumps | Native `tracing` is structured, defaults to `WARN`, writes to stderr only, and uses redacted fields for credential boundaries. Passwords, key material, tokens, and sensitive environment values are forbidden in logs. Native helper credentials and clipboard commands are not cloneable, reducing accidental in-memory duplication. MobaRust does not submit crash dumps; an OS crash reporter or debugger may still capture process memory and is outside the app boundary. The optional audit file is a separate bounded lifecycle journal, not a terminal transcript. |
| Clipboard exposure | Paste is explicit; MobaRust intercepts multiline terminal paste and asks for confirmation before sending it. RDP and VNC clipboard input are disabled by default and opt-in per profile; RDP's native backend is Windows-only, while VNC uses its bounded negotiated text channel behind the loopback-only fixture boundary. The parent and helper both reject non-requested clipboard input, and a loopback fixture verifies that no RFB `ClientCutText` is sent without opt-in. Remote content is never copied into the Mac clipboard automatically. |
| Exported profiles | Export configuration and secret references separately. Never include secret values by default; warn before exporting sensitive references. Settings export is a separate non-secret schema and cannot include sessions or vault material. |
| Portable mode | Portable mode is marker-gated by `portable.flag`; credentials use a separate Argon2id + AES-256-GCM vault file, atomic private writes, and explicit native unlock/lock. The vault path must be a regular file and symlinks/directories are rejected. It is not a plaintext JSON exception. |
| Application backups | Backups may contain session metadata and, when the portable data directory is included, the encrypted vault ciphertext. Treat backup locations as sensitive, keep the vault passphrase separate, and never treat a backup as a secret-free export. Recovery is explicit and schema-validated; the app does not silently replace corrupt state. |
| Host impersonation | SSH adapters must verify known_hosts/fingerprints and must never silently accept unknown keys. |
| Remote content execution | Terminal output and PTY titles are rendered as bounded terminal/display text only; URL detection accepts bounded HTTP(S) links without embedded credentials, and opening one requires explicit user confirmation. No remote text becomes HTML or an automatic navigation. |
| Saved startup command execution | Startup commands are optional, bounded session configuration, validated natively, and documented as shell input on SSH connect or after explicit confirmation for a saved local profile. They are not silently derived from remote content or snippets. |
| Webview script injection | The Tauri CSP disallows `unsafe-eval`, objects, and framing; remote output is escaped before any non-terminal rendering. |
| IPC abuse | Use typed, narrow commands with validation. Do not expose `execute_anything(command: String)`. Native PTY/WSL errors keep underlying OS paths and process details out of their user-facing display text. |
| X11 display/cookie exposure | X11 is opt-in and requires an explicit TCP/Unix display target. Rust generates the temporary forwarding cookie, keeps display bytes native, caps channels, and never reads or exposes `DISPLAY`/`.Xauthority` automatically. |

## Audit history boundary

The optional local audit history is capped at 1,000 events and stored separately
from sessions, settings, and the vault. Its schema permits only a timestamp,
event kind, opaque session ID, and protocol. It intentionally has no fields for
terminal input, remote paths, hostnames, usernames, error text, or credentials.
The UI provides an explicit clear action. Audit history is not part of session
import/export, and a corrupt or unknown-schema audit file is rejected rather
than silently replaced.

## Backup and recovery boundary

MobaRust does not silently synchronize its session store or vault to a cloud
service. A normal session/settings export is intentionally secret-free, but a
filesystem backup can still include profile metadata, application state, or
the encrypted portable vault file. Operators must protect those backups with
the same care as the device and must keep the portable-vault passphrase out of
the backup set.

Restoring a backup is not an implicit startup action. The store and settings
loaders validate their schema and reject corrupt or unknown data rather than
overwriting it with defaults. Import remains explicit, and importing session
definitions never imports password or private-key material.

## Non-goals

MobaRust cannot protect secrets from a fully compromised operating system, an attacker with equivalent local-user access, or a user who explicitly exports or pastes the secret. These limitations must remain visible in security documentation and UI copy.

## Release gates

The credential-backed SSH slice has local coverage for redaction, malformed
imports, host-key mismatch, cancellation, explicit host trust, and secret
separation. Promotion to a broadly supported production release still requires
the cross-platform runtime, packaging/signing, and external interoperability
evidence listed in `ROADMAP.md`. Run dependency audits locally and review every
new native capability against this boundary.
