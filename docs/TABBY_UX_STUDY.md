# Tabby UX study (secondary reference)

Date: 2026-11-19. Tabby 1.0.238 (`org.tabby`) is installed in `/Applications` by
the user. Tabby is a UX reference for daily terminal comfort, not the target
product: MobaRust stays a MobaXterm-class suite and does not copy Tabby's
interface.

## Method and limits

- Sources: Tabby's public README, its default macOS hotkeys
  (`tabby-core/src/configDefaults.macos.yaml`) and its SSH profile settings
  template (`tabby-ssh/src/components/sshProfileSettings.component.pug`), all
  from the public MIT-licensed repository.
- The installed app was only identified (version and bundle id). Its
  `app.asar` was not unpacked, and no assets were extracted.
- Tabby's window was not observed or screenshotted: this session has no
  Screen Recording or Accessibility access. The notes below describe its
  documented behaviour, not a hands-on session.

## Comparison

| Area | Tabby (documented) | MobaRust today | Decision |
| --- | --- | --- | --- |
| Close / reopen tab | ⌘W / ⌘⇧T | ⌘W and ⌘⇧T, plus a tab menu entry and toolbar button | Same; keep |
| Next / previous tab | Ctrl-Tab / Ctrl-Shift-Tab, optional MRU order | Configurable next/previous shortcuts | Keep; MRU order not needed yet |
| Split panes | Nested splits, rearrange panes with a modifier | Split right/down, focus pane shortcut | Keep; nested rearranging is a later idea |
| Focus on the terminal | Tabs on any side, fullscreen, Quake-style dropdown | Collapsible sidebar (⇧⌘B), focus mode (⌘⇧↵) | MobaRust's focus mode covers the need; no global dropdown |
| Rename / move tabs | ⌘R rename, ⌘⇧←/→ move | **Added:** "Rename tab…" in the tab menu (the name then stays, instead of following the shell title) and Move left/right with ⌃⇧PageUp/PageDown, which works even with focus in the terminal. ⌘⇧→ was already MobaRust's split-right shortcut | Done; checked in the browser preview |
| Copy on select, right-click paste | Optional (PuTTY style) | **Added:** opt-in "Copy on select" in terminal settings (off by default). Right-click paste not added: pasting stays explicit so the multiline paste check always applies | Done; checked in the browser preview |
| Multiline paste warning | Yes | Yes, and required for broadcast | Same |
| SSH connection manager | Profiles with groups, jump hosts, port forwarding tab, "reuse session for multiple tabs" | Nested folders, jump hosts, tunnels, one-click group opening with 4 connections at a time | **Added:** "New shell on this connection" in an SSH tab's menu opens another shell over the same authenticated connection (no new handshake or login). Each shell keeps its own input queue and reconnects on its own; the connection closes with its last shell |
| Secrets | Encrypted container for SSH secrets and config | OS keychain references; optional portable vault | Equivalent goal |
| File transfer | Zmodem in SSH sessions, SFTP | SFTP browser with progress, SCP, remote editing | MobaRust is broader; Zmodem not planned |
| Serial, Telnet | Yes | Yes | Same |
| Import from MobaXterm | No | Yes, including Windows exports | MobaRust's differentiator |

## Takeaways for MobaRust

1. Done: tab rename and keyboard tab reordering.
2. Done: opt-in copy on select. Right-click paste is left out on purpose.
3. Done: SSH connection reuse for extra shells on the same host, from the tab
   menu. Checked against local OpenSSH by an automated test; the native click
   path is not yet observed.
4. Keep what already differs on purpose: groups opened in one click,
   MobaXterm import, and safety prompts on broadcast.
