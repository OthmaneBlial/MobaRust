# Migration from MobaXterm

MobaRust will prioritize migration paths that are documented, user-controlled, and based on formats users can legally access.

## Current status

OpenSSH import is shipped for the common, secret-free connection fields. The application currently exposes a typed session model with protocol, host, port, username, tags, folder, startup settings, jump-host references, and forwarding references. Imported profiles are idempotent by protocol/name and never copy passwords or private-key material into the session store.

The desktop session list exposes an import action. It reads the user-selected config through a typed native command, reports unsupported directives and skipped malformed hosts, and keeps `IdentityFile` as a key reference. `ProxyJump` is preserved as a jump-host reference; imported profiles are not presented as reconnectable until config-alias resolution supplies the complete hop descriptors. Quick Connect can already establish an explicit agent-backed jump hop. A valid `ServerAliveInterval` is stored as a bounded native SSH keepalive setting; `0` disables keepalives and invalid values remain visible in the import note without being applied. The native importer bounds the selected config to 1 MiB and refuses non-regular paths before parsing.

## Planned order

1. OpenSSH `~/.ssh/config` — implemented for `Host`, `HostName`, `User`, `Port`, `IdentityFile`, `ProxyJump`, and `ServerAliveInterval` import boundaries. The path must be supplied explicitly; MobaRust never reads it automatically.
2. Publicly documented or user-exported formats from open-source tools such as PuTTY, Remmina, Tabby, electerm, and mRemoteNG.
3. MobaXterm migration (partial, shipped): **Import MobaXterm sessions** in the session list reads an exported `.mxtsessions` file or `MobaXterm.ini` chosen by the user. SSH bookmarks (type 109) are imported with name, nested folder, host, port and username, after a preview where each row can be added, kept as a duplicate, replace an existing session, or skipped. Not imported: passwords (never read), private-key paths, gateway/jump hosts, X11, terminal/font settings, and non-SSH bookmarks (listed as skipped). Imported sessions use the SSH agent until edited. A file exported on Windows imports the same way on macOS, Linux or Windows: UTF-8 (with or without BOM), UTF-16 with a BOM, and Windows-1252 (ANSI) files are decoded, CRLF line endings and lowercase section names are accepted. Validated with synthetic fixtures that mimic a Windows export (CRLF, Windows-1252 accents and €, UTF-16LE/BE, nested `SubRep`, Windows key paths), not with a file produced by MobaXterm itself.

Unsupported directives must remain visible in an import report. Imports create configuration and credential references, never silently copied passwords.
