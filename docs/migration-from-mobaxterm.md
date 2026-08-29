# Migration from MobaXterm

MobaRust will prioritize migration paths that are documented, user-controlled, and based on formats users can legally access.

## Current status

OpenSSH import is shipped for the common, secret-free connection fields. The application currently exposes a typed session model with protocol, host, port, username, tags, folder, startup settings, jump-host references, and forwarding references. Imported profiles are idempotent by protocol/name and never copy passwords or private-key material into the session store.

The desktop session list exposes an import action. It reads the user-selected config through a typed native command, reports unsupported directives and skipped malformed hosts, and keeps `IdentityFile` as a key reference. `ProxyJump` is preserved as a jump-host reference; imported profiles are not presented as reconnectable until config-alias resolution supplies the complete hop descriptors. Quick Connect can already establish an explicit agent-backed jump hop. `ServerAliveInterval` is retained as an import note until native keepalive settings are wired.

## Planned order

1. OpenSSH `~/.ssh/config` — implemented for `Host`, `HostName`, `User`, `Port`, `IdentityFile`, `ProxyJump`, and `ServerAliveInterval` import boundaries.
2. Publicly documented or user-exported formats from open-source tools such as PuTTY, Remmina, Tabby, electerm, and mRemoteNG.
3. MobaXterm migration only where the input format is publicly documented or supplied by the user; no proprietary binary dependency or unauthorized reverse engineering.

Unsupported directives must remain visible in an import report. Imports create configuration and credential references, never silently copied passwords.
