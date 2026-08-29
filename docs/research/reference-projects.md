# Reference project research log

The repositories below are cloned into `base/` for local study only. They are not source material to copy and are excluded from Git.

| Project | Repository | License/revision | Notes |
| --- | --- | --- | --- |
| Tabby | https://github.com/Eugeny/tabby | `14e2d60` (shallow clone) | Terminal/session UX, profiles, plugins, xterm integration |
| electerm | https://github.com/electerm/electerm | `ba923cd` (shallow clone) | SSH/SFTP composition and transfer UX |
| mRemoteNG | https://github.com/mRemoteNG/mRemoteNG | `3d8d1ab` (shallow clone) | Connection hierarchy and protocol workflows |
| 1Remote | https://github.com/1Remote/1Remote | `5b9d844` (shallow clone) | Multi-protocol remote management patterns |
| RustDesk | https://github.com/rustdesk/rustdesk | `03a7fc5` (shallow clone) | Rust remote desktop and platform boundaries |
| FreeRDP | https://github.com/FreeRDP/FreeRDP | `b2a1214` (shallow clone) | RDP integration surface |
| Remmina | https://github.com/FreeRDP/Remmina | `bb33690` (shallow clone) | Plugin-oriented multi-protocol UX |
| Alacritty | https://github.com/alacritty/alacritty | `ede2ac1` (shallow clone) | PTY, input, rendering, and performance |
| WezTerm | https://github.com/wezterm/wezterm | `08e5e0a` (shallow clone) | Panes, tabs, Unicode, multiplexing |
| russh | https://github.com/Eugeny/russh | `d3ae702` (shallow clone) | Rust SSH choices and limitations |

After each clone, record the exact revision, license, useful ideas, limitations, and rejected concepts here before using it to justify an implementation decision.

## Initial findings

This is an initial architecture pass, not a claim of feature parity or permission to copy code.

- **Tabby** — its terminal/profile/plugin composition reinforces keeping terminal rendering and session metadata separate. The plugin surface is powerful but is a poor early dependency because it expands security and lifecycle boundaries.
- **electerm** — the SSH/SFTP and file-manager composition is a useful product reference for making a remote filesystem a peer of the terminal, not a separate disconnected tool. MobaRust keeps the transport in Rust instead of inheriting a JavaScript-first backend.
- **mRemoteNG / 1Remote** — connection hierarchy and protocol-specific settings justify a typed session record plus folder/tag indexes. Their Windows-oriented workflows are useful for organization, but are not a reason to make every protocol look identical.
- **RustDesk** — the repository makes the native helper, codec, input, and cross-platform packaging boundary visible. Remote desktop support will need the same isolation rather than a pretend web framebuffer.
- **FreeRDP / Remmina** — FreeRDP is the serious protocol engine; Remmina's plugin boundary is a useful integration reference. Remmina's GPL license is a reason to study behavior and interfaces carefully, not copy implementation or assets into MobaRust.
- **Alacritty / WezTerm** — both reinforce that PTY ownership, Unicode/input semantics, panes, and rendering throughput deserve native architecture and measurable backpressure. MobaRust's first PTY slice follows this direction with a bounded native event channel.
- **russh** — the maintained client exposes explicit `check_server_key`, interactive PTY channels, direct forwarding primitives, and SFTP ecosystem support. The default handler rejects keys; MobaRust makes the policy explicit and adds known_hosts/fingerprint tests around it.

Rejected for the first slice: copying a renderer or protocol adapter wholesale, shelling out to `ssh` as the application architecture, and accepting unknown host keys for convenience.
