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
| Plaintext password exposure | Session records contain credential references only. Password acquisition stays native and is not serialized in diagnostics. |
| Private-key exposure | Keys are referenced by path and never copied into session metadata. Passphrases use the native vault; loaded key-memory hygiene remains a release-review gate. |
| Malicious local process | Documented as an OS limitation; minimize plaintext lifetime and never claim protection from a process with equivalent user privileges. |
| Compromised application database | The current session store contains metadata and opaque references only. Platform vault entries are separate; portable encrypted storage still needs its own audited design. |
| Logs and crash dumps | Structured redaction is required. Passwords, key material, tokens, and sensitive environment values are forbidden in logs. |
| Clipboard exposure | Paste is explicit; multiline shell input is not auto-executed. Remote clipboard support will be opt-in per protocol. |
| Exported profiles | Export configuration and secret references separately. Never include secret values by default; warn before exporting sensitive references. |
| Portable mode | A portable directory is not a plaintext exception. Use an audited cryptographic design and an explicit unlock flow. |
| Application backups | Document that backups can contain session metadata; provide a safe export format and migration versioning. |
| Host impersonation | SSH adapters must verify known_hosts/fingerprints and must never silently accept unknown keys. |
| Remote content execution | Terminal output is rendered as terminal text only; URLs need explicit user action and are not automatic HTML. |
| IPC abuse | Use typed, narrow commands with validation. Do not expose `execute_anything(command: String)`. |

## Non-goals

MobaRust cannot protect secrets from a fully compromised operating system, an attacker with equivalent local-user access, or a user who explicitly exports or pastes the secret. These limitations must remain visible in security documentation and UI copy.

## Release gates

Before credential-backed SSH is presented as production-ready, add tests for redaction, malformed imports, host-key mismatch, cancellation, and secret lifetime. Run dependency audits locally and review every new native capability against this boundary.
