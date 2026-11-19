# MobaRust website design: Freight Manifest

The public site (`site/`) is a container yard, not the app's instrument panel.
Every server is a container. The download form is a cargo manifest clipped to
a corrugated corten wall. One click opens the whole group, four boxes at a
time. The app UI keeps its own world in [DESIGN.md](../DESIGN.md).

Surface brief and direction contract: `.impeccable/surfaces/site-index-html.md`.

## Tokens (`site/styles.css`, `:root`)

| Token | Value | Use |
|---|---|---|
| `--oxide-950` / `--oxide-900` | #160c07 / #21120b | Header rail, totals strip, footer, code |
| `--corten-700…500` | #5b2b14 · #763718 · #93451d | Corrugated wall ribs (`.steel`) |
| `--corten-200`, `--muted-on-dark` | #f0c9ac, #dbbaa5 | Body text and captions on the wall |
| `--paper` / `--ink` / `--ink-2` | #f1ebdf / #1a1410 / #4a3d33 | Manifest, paper sections, rate card |
| `--yellow` | #f3c234 | Safety yellow: primary action, focus ring, seal tags, title accent |
| `--seal-red` | #a3381c | The €0 stamp and "Never read" |
| lamps | off #4a3a31, wait yellow, on #4fbf63 | Container status lights in the yard |

## Type

- Display: Big Shoulders Stencil (OFL, `site/fonts/`), uppercase, at most 6rem.
- Body: Archivo. Data and codes: JetBrains Mono.
- No system display face. Fonts are self-hosted variable latin subsets.

## Components

- `.wall.steel`: corrugated corten section. The repeating gradient is the
  material, not decoration, and is kept on purpose.
- `.manifest`: ruled paper form with the download buttons, the platform table
  and the honest preview note. `.stamp` is the €0 seal.
- `.totals`: four measured figures, each linked to its receipt.
- `.yard`: 14 sample containers that light four at a time
  (`site/app.js`). Under reduced motion they show connected at once.
- `.rates` / `.rate`: one tariff today. The grid takes a second column for a
  paid edition later without a new layout.
- `.seals`: three safety promises, each closed by a yellow seal tag.

## Finish review (2026-11-19)

Checked at 1440 px and 390 px for `index.html` and `docs.html`: no horizontal
overflow; mobile menu, copy button and yard verified in headless Chrome.
`impeccable detect` findings left on purpose:

- repeating-stripes-gradient: the corrugated wall is the direction's material.
- dark-glow: the yard's status lamps glow because they are lamps.
- cramped-padding and flat-type-hierarchy: full-bleed sections inset by
  `--wrap`, and the detector reads computed sizes it does not have; the
  rendered captures show the hierarchy.
- low-contrast #f3c234 on #8a4d1c: the yellow "fleet." accent is display-size
  text (passes the 3:1 large-text rule).

Verdict: ships.

## Raster provenance

| File | Source |
|---|---|
| `site/media/workspace-dark.webp`, `workspace-light.webp` | Headless Chrome capture of the desktop UI browser preview (main branch), 2400×1500 |
| `site/media/group-picker.webp` | Same preview, group picker dialog, 1280×800 |
| `site/media/og.png` | `site/og.html` rendered in headless Chrome at 1200×630 |
| `site/media/workspace-light.jpg`, demo files | Earlier captures and `tools/render-demo.mjs`, kept for the README and release notes |
