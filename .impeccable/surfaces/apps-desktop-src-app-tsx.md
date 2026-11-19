---
version: 1
slug: "apps-desktop-src-app-tsx"
primary_target: "apps/desktop/src/App.tsx"
related_targets: ["apps/desktop/src/styles.css"]
---

# Surface brief: desktop workspace

Scope: the MobaRust desktop app shell (topbar, session tree, tabs, terminal, tool views, dialogs). Mode: Operate.
Audience and job: sysadmins/DevOps opening groups of servers, working in terminals for hours, moving files, keeping tunnels up.
Constraints: terminal owns most of the window; standard controls; light and dark themes; keyboard-first with visible focus.

## Direction contract

THESIS: A remote-admin console built like a precise Braun instrument. It refuses the category default of flat grey panels with one blue accent: here state lives in round indicator lamps and orange is reserved for the single primary action.

OWN-WORLD: Graphite body on one numbered 10-step warm-neutral ramp (n0–n9), brushed-aluminium hairlines and a slotted grille on the top strip, round lamps (green connected, amber connecting, red failed, off grey), Braun orange for primary, a fixed six-hue wheel for session groups. Light theme is warm aluminium/off-white on the same ramp inverted. System sans for UI, system mono for terminal and figures.

STORY: The admin sees every host's state at a glance as lamps, opens a whole group with one press, and watches the lamps light in sequence while the terminal keeps the window.

FIRST VIEWPORT: 40px aluminium strip (mark, name, grille, Quick connect, Commands, theme, settings); 264px session tree with group hues and per-host lamps and a per-group open button; tab strip with lamp and group-hue edge per tab; group-open meter row of lamps when running; terminal fills the rest; 24px status strip.

FORM: Braun / Rams industrial controls, position 5 of 7 on the ordered list. seed:41586f83a09c0fbd

SIGNATURE: group opening drawn as a level-meter row of lamps lighting in sequence; a changed session state keeps its lamp lit until its tab is seen. Motion: 150–200 ms lamp fades only.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
