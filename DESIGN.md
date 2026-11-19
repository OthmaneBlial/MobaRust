# MobaRust design: Instrument Panel

The desktop UI reads like a calm hardware console. Graphite surfaces sit under
an aluminium top strip. State shows in round lamps. Orange is used only for
the primary action.

## Tokens (`apps/desktop/src/styles.css`, `:root` and light blocks)

| Token | Dark | Light | Use |
|---|---|---|---|
| `--panel` | #141517 | #dedcd6 | Sidebar, tab strip |
| `--bg` / `--term-bg` | #1d1e20 | #e9e8e3 / #fbfaf7 | Workspace, terminal |
| `--panel-raised` / `--panel-soft` | #25272a / #2f3134 | #f5f4f0 / #d3d1ca | Selected rows, inputs, popovers |
| `--text` / `--muted` / `--faint` | #e4e2dc / #a9a8a2 / #85888d | #1d1e20 / #4c4e53 / #686b70 | Text levels |
| `--amber` | #ee6a2c | #c4501a | Primary action only (Braun orange) |
| `--mint` / `--warn` / `--coral` / `--lamp-off` | #47b05c / #e7b53c / #e5534b / #3d4044 | darker equivalents | Lamps: connected / connecting / failed / off |
| `--g1`…`--g6` | blue, violet, teal, ochre, indigo, rose | same | Group hues |

The terminal palettes live in `apps/desktop/src/theme.ts` and follow the same ramp.

## Rules

- **Type:** the system UI sans for chrome, and the system mono only for terminal output, hosts, paths and key hints. Labels use 10.5px semibold sans with tracking, not mono.
- **Lamps** (`.terminal-tab-dot-*`): 8px round lights with a soft glow. Transitions take 180ms. Connecting lamps pulse, except under reduced motion. Tabs, the session tree and the group-open meter all use them.
- **Group hue:** `groupHue(folder)` in `workspace-layout.ts` hashes the top-level folder to one of `--g1…--g6`. The hue appears as a 3px bar in the tree and on the top edge of tabs opened from that group.
- **Focus:** a 2px orange `:focus-visible` outline everywhere. Selection is tinted orange. Scrollbars are thin.
- **Shape:** buttons are 6–7px radius, and dialogs and menus are 10px.
- The terminal gets the space. Focus mode hides all chrome.

The public website has its own world, documented in [docs/WEBSITE_DESIGN.md](docs/WEBSITE_DESIGN.md).
