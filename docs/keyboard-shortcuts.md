# Keyboard shortcuts

MobaRust keeps shortcuts in the typed, secret-free application settings. They
can be edited in **Settings → Keyboard shortcuts**, imported/exported with the
settings document, and reset to the defaults without affecting sessions or the
credential vault.

## Defaults

| Action | Default |
| --- | --- |
| New terminal | `Mod+N` |
| Quick Connect (new SSH or other protocol session) | `Mod+K` |
| Command palette | `Mod+Shift+P` |
| Close tab | `Mod+W` |
| Next tab | `Ctrl+Tab` |
| Previous tab | `Ctrl+Shift+Tab` |
| Split right | `Mod+Shift+ArrowRight` |
| Split down | `Mod+Shift+ArrowDown` |
| Focus pane | `Mod+1` |
| Search terminal | `Mod+F` |
| Toggle sidebar | `Mod+Shift+B` |
| Open macros | `Mod+Shift+M` |
| Disable broadcast | `Escape` |

While a terminal has focus, standard zoom keys remain available even though
xterm uses an internal editable element: `Mod+=` or `Mod++` increases the font,
`Mod+-` decreases it, and `Mod+0` resets it to 13 px. The value is bounded to
8–32 px and persisted as an ordinary non-secret appearance setting.

`Mod` means Command on macOS and Control on Windows/Linux. The other tokens
are explicit: `Ctrl`, `Alt`, `Shift`, `Tab`, `Escape`, `Enter`, `Backspace`,
`Delete`, `Space`, `ArrowUp`, `ArrowDown`, `ArrowLeft`, `ArrowRight`, or one
ASCII letter/digit. Tokens are separated with `+`, for example
`Mod+Shift+ArrowRight`.

MobaRust rejects malformed shortcuts, repeated modifiers, contradictory
`Mod+Ctrl` combinations, and effective collisions. Modifier order and letter
case do not create a second usable shortcut, so those forms are also treated
as collisions.

Shortcuts do not intercept typing in inputs, textareas, selects, or editable
content. The command palette remains searchable with the keyboard, and the
Help screen renders the current values rather than stale hard-coded hints.

Escape is always retained as a safety action: it closes transient overlays and
cancels macro recording/runs or broadcast input when active. A custom
**Disable broadcast** shortcut can be configured as an additional emergency
action; it never sends data to a terminal.

Shortcut handling is UI-only. It does not expose credentials, shell access, or
remote protocol bytes to the frontend. Settings migration supplies these
defaults when an older settings document has no keyboard section.
