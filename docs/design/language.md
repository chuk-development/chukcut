# chukcut design language

The editor keeps CapCut's *placement* — title bar, asset panel, player,
inspector, timeline — and its dark, quiet mood. Everything else is ours:
cool graphite instead of neutral grey, an aqua accent, hairline borders,
softer radii, a mono face for every time and number readout, and our own
icons for the things that carry the product's identity.

Tokens live in `crates/app/src/theme.rs`; components in `crates/app/src/ui/`.
Never write a hex value in a view. If a colour is missing, add a token.

## Principles

1. **Content first.** The picture and the clips are the only saturated
   things on screen. Chrome is graphite; the accent marks *one* thing per
   region: the active tab, the selection, the primary action.
2. **Elevation by lightness, not shadow.** Each step up is a lighter
   surface plus a hairline. Shadows only for things that float
   (menus, tooltips, dialogs).
3. **Calm density.** Compact rows (28 px), generous gutters between
   groups (16 px). Align everything on a 4 px grid.
4. **Every value is editable and resettable.** Numbers are mono, right
   aligned in a sunken well; a reset and a keyframe slot sit at the end of
   each property row, always in the same column.

## Colour

### Surfaces (by elevation)

| Token | Hex | Use |
|---|---|---|
| `BG` | `#0f1013` | window, gutters between panels (elevation 0) |
| `PANEL` | `#1a1b1f` | panels: assets, player, inspector, timeline (1) |
| `PANEL_RAISED` | `#24262b` | cards, tiles, selected list rows, hovered rows (2) |
| `OVERLAY` | `#2a2c32` | menus, popovers, tooltips, dialogs (3) |
| `WELL` | `#131417` | sunken fields: number boxes, search, path fields |
| `VIEWER` | `#000000` | the canvas backdrop in the player |

### Borders

| Token | Hex | Use |
|---|---|---|
| `HAIRLINE` | `#26282d` | panel outline, dividers inside a panel |
| `BORDER` | `#33363d` | control outlines, tile outline on hover |
| `BORDER_STRONG` | `#464a52` | outlined buttons, focus-less emphasis |

### Text

| Token | Hex | Use |
|---|---|---|
| `TEXT` | `#e9eaee` | values, titles, active labels |
| `TEXT_DIM` | `#9a9ea8` | labels, inactive tabs, captions |
| `TEXT_MUTED` | `#6b6f79` | hints, placeholders, units |
| `TEXT_DISABLED` | `#4a4d55` | disabled controls |

### Accent — "aqua"

| Token | Hex | Use |
|---|---|---|
| `ACCENT` | `#2fd5c8` | active tab, selection, primary button, focus ring |
| `ACCENT_HOVER` | `#5fe2d7` | primary hover |
| `ACCENT_PRESSED` | `#22b5aa` | primary pressed |
| `ACCENT_SOFT` | accent at 14 % | selected row / toggled icon background |
| `ON_ACCENT` | `#062220` | text and icons on an accent fill |

### Semantic

| Token | Hex | Use |
|---|---|---|
| `SUCCESS` | `#3ecf8e` | done, saved |
| `WARNING` | `#f2b440` | will be replaced, slow path |
| `DANGER` | `#f2555a` | failed, delete, record |
| `INFO` | `#5aa8ff` | neutral notices |

### Clips (by media kind)

Body / title strip / detail (waveform, glyph). Mid-saturation so the white
selection outline and the playhead always win.

| Kind | Body | Strip | Detail |
|---|---|---|---|
| Video | `#0d4f54` | `#11666c` | — (thumbnails) |
| Image | `#463b86` | `#57499f` | — |
| Audio | `#14345c` | `#1b4377` | `#4f86c9` |
| Text | `#7d5419` | `#946620` | — |
| Effect | `#6b2b60` | `#823576` | — |
| Other | `#4a4c55` | `#585a64` | — |

## Type

Face: **Noto Sans** for UI, **Noto Sans Mono** for timecodes, durations,
sizes and every number readout (fixed width, no jitter while playing).

| Role | Size | Weight | Colour |
|---|---|---|---|
| Display (dialog title, empty-state title) | 15 | Semibold | `TEXT` |
| Panel title | 13 | Semibold | `TEXT` |
| Body / tab label | 13 | Regular (Medium when active) | `TEXT` / `TEXT_DIM` |
| Property label | 12 | Regular | `TEXT_DIM` |
| Value | 12 mono | Regular | `TEXT` |
| Caption / tile name / hint | 11 | Regular | `TEXT_MUTED` |
| Badge | 10 mono | Medium | per tone |

## Spacing, size, radius

- Spacing scale (px): `2 · 4 · 6 · 8 · 12 · 16 · 24`. Gutter between
  panels: 6. Panel padding: 12. Between sections: 16.
- Heights: title bar 40 · panel header 40 · tab rail 56 · property row 28 ·
  control 26 (compact) / 30 (default) · icon button 24 / 28 / 32.
- Radii: `R_XS 3` (checkbox, badge) · `R_SM 5` (controls, inputs, tiles) ·
  `R_MD 8` (panels, cards) · `R_LG 12` (dialogs).
- Asset panel 640, inspector 590; the player takes the rest.

## Icons

- Lucide (the GPUI Kit catalog) for generic glyphs: undo, trash, search,
  folder, chevrons.
- **Our own set** (`ui::icons`) for what carries identity: the logo mark,
  the asset-tab rail, the transport (play, pause, frame step), keyframe
  diamond and reset. Drawn on a 24 grid, 1.75 stroke, round caps and joins,
  no fills except "on" states (filled diamond, filled play).
- Sizes: 14 in rows and badges, 16 in toolbars and headers, 20 on the tab
  rail. The hit target is never under 24.
- Colour: `TEXT_DIM` at rest, `TEXT` on hover, `ACCENT` when on.

## States

| State | Treatment |
|---|---|
| Hover | surface one step up (`PANEL` → `PANEL_RAISED`), text `TEXT_DIM` → `TEXT` |
| Pressed | `OVERLAY` |
| Selected / on | `ACCENT_SOFT` fill and `ACCENT` glyph or label; tiles get a 2 px accent ring |
| Focus (keyboard) | 1 px `ACCENT` border on the control; never a glow |
| Disabled | `TEXT_DISABLED`, no hover, default cursor |
| Drop target | `ACCENT` dashed-looking 1 px border plus `ACCENT_SOFT` fill |

## Motion

GPUI hover states switch instantly; we do not fake transitions there.
Where we animate: collapse/expand 120 ms, dialogs and popovers 160 ms
(GPUI Component's own), progress bars linear. Ease-out cubic. Nothing loops
except a busy spinner.

## Density rules

- One accent per region. A panel header never has two accent things.
- Labels sit left, values right, actions at the far right in a fixed
  column (reset, then keyframe).
- Truncate with an ellipsis, never wrap, in rows, tiles and headers.
- Empty states say what to do and offer the one action that does it.
