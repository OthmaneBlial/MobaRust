# Migration from MobaXterm

MobaRust will prioritize migration paths that are documented, user-controlled, and based on formats users can legally access.

## Current status

No MobaXterm importer is shipped yet. The application currently exposes a typed session model with protocol, host, port, username, tags, folder, startup settings, jump-host references, and forwarding references.

## Planned order

1. OpenSSH `~/.ssh/config`, including `Host`, `HostName`, `User`, `Port`, `IdentityFile`, `ProxyJump`, and `ServerAliveInterval`.
2. Publicly documented or user-exported formats from open-source tools such as PuTTY, Remmina, Tabby, electerm, and mRemoteNG.
3. MobaXterm migration only where the input format is publicly documented or supplied by the user; no proprietary binary dependency or unauthorized reverse engineering.

Unsupported directives must remain visible in an import report. Imports create configuration and credential references, never silently copied passwords.
