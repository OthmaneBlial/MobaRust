# Feature gap: MobaXterm workflows vs MobaRust

MobaXterm behaviour below comes from its public documentation and widely
described behaviour, not from running it (it is Windows-only and not installed
here). Tabby notes (Tabby 1.0.238, from its public docs) are in
[TABBY_UX_STUDY.md](TABBY_UX_STUDY.md).

| MobaXterm workflow | MobaRust state | Proposed / done | Priority | Verification |
|---|---|---|---|---|
| Large tabbed terminal | Was capped at ~half the window | Full-height terminal, focus mode, hideable rail | P0 | Browser preview; native pending |
| Close tab / close others | Close only | Middle-click, context menu, close others, reopen closed (`Mod+Shift+T`) | P0 | Browser preview |
| Split terminals | Right/down splits with draggable dividers | Keep | P1 | Existing |
| Session tree with nested folders | Flat folder labels | Nested collapsible tree from `A/B` paths | P0 | Browser preview |
| Open a whole folder of sessions | Missing | "Open all" (4 concurrent), progress, cancel, retry, `--open-group` | P0 | Unit + real local sshd (14 sessions) |
| Import MobaXterm sessions | Done | `.mxtsessions` / `.moba` / `MobaXterm.ini` SSH and SFTP bookmarks (folders, host, port, user, login command) with preview and merge decisions; Windows-1252, UTF-8 and UTF-16 files on any OS | P0 | Fixture tests plus a real local OpenSSH connection through an imported gateway; no file written by MobaXterm itself |
| Import passwords from MobaXterm | Not supported | Deliberately not imported (encrypted, not portable) | — | By design |
| Private-key path, gateway, X11 from MobaXterm bookmarks | Key and gateway done; X11/proxy not imported | Key paths map to `~/.ssh/<file name>` (agent kept when that file is missing; `.ppk` gets a puttygen hint); SSH gateways become jump hosts | P2 | Field layout from the community format study (MobaXterm 26.3); end-to-end test with local sshd |
| Telnet / RDP / VNC bookmarks | Skipped with a warning | Map once field layouts are verified | P2 | — |
| SFTP side browser | Files view per SSH session | Make it a dockable side panel | P1 | Existing sshd tests |
| SSH tunnels | Local/remote/dynamic with manager | Keep; UI polish | P1 | Existing sshd tests |
| Multi-exec | Broadcast input to chosen tabs | Keep | P1 | Existing |
| Macros / snippets | Present | Keep | P2 | Existing |
| Open selected sessions | Done (browser preview) | Group picker: tick sessions, open N, 4 at a time | P1 | — |
| Workspace restore | Done (browser preview) | Opt-in from the tab menu; stores only saved-session ids and plain local shells | P2 | — |
| Favorite (pinned) groups | Done (browser preview) | Star on a group pins it above the tree with Open all / choose; stored per device | P2 | — |
| X server | X11 forwarding to an existing display | No built-in X server | — | Existing tests |
