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
| Rename / move tabs | ⌘R rename, ⌘⇧←/→ move | Not available | **Gap.** Worth adding: long days with many servers benefit from naming and ordering tabs |
| Copy on select, right-click paste | Optional (PuTTY style) | Copy button and normal shortcuts | **Gap**, low priority: familiar to MobaXterm users, add as an opt-in setting |
| Multiline paste warning | Yes | Yes, and required for broadcast | Same |
| SSH connection manager | Profiles with groups, jump hosts, port forwarding tab, "reuse session for multiple tabs" | Nested folders, jump hosts, tunnels, one-click group opening with 4 connections at a time | MobaRust is stronger for groups; **session reuse is a gap** worth studying (one SSH connection, several shells) |
| Secrets | Encrypted container for SSH secrets and config | OS keychain references; optional portable vault | Equivalent goal |
| File transfer | Zmodem in SSH sessions, SFTP | SFTP browser with progress, SCP, remote editing | MobaRust is broader; Zmodem not planned |
| Serial, Telnet | Yes | Yes | Same |
| Import from MobaXterm | No | Yes, including Windows exports | MobaRust's differentiator |

## Takeaways for MobaRust

1. Add tab rename and keyboard tab reordering. Both are small, and both help with
   long sessions across many servers.
2. Consider opt-in copy-on-select with right-click paste for users coming from
   MobaXterm and PuTTY.
3. Study SSH connection reuse for extra shells on the same host. It cuts
   connection time and the number of authentication prompts.
4. Keep what already differs on purpose: groups opened in one click,
   MobaXterm import, and safety prompts on broadcast.
