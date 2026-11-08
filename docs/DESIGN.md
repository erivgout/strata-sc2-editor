# Strata design system

Strata is a StarCraft II map editor for people who build arcade and melee maps
and spend hours inside the tool. Its job: sculpt terrain, place units, script
triggers and edit game data quickly, in one window.

## Principles

- **The map is the hero.** Chrome is quiet slate; the viewport is the brightest,
  most saturated thing on screen.
- **One accent, one meaning.** Mineral cyan marks what is active or selected:
  the current mode, the selected object, the focused field. Never decoration.
- **Everything is reachable from the keyboard.** Every command lives in the
  command palette (Ctrl+K) and shows its shortcut. Modes have single-key shortcuts.
- **Nothing is modal unless it must be.** Panels dock, undock and rearrange.
  Editing triggers never hides the map.
- **Every change is undoable**, including changes made by AI clients over MCP,
  which are labeled as such in History.
- **Plain words, sentence case.** No all-caps labels. Buttons say what they do
  ("Save map", "Place unit"). Errors say what happened and how to fix it.
  Empty states invite an action.

## Color tokens

| Token      | Dark      | Light     | Use |
|------------|-----------|-----------|-----|
| bg_deep    | `#131920` | `#D9DFE6` | Behind the viewport, text fields |
| bg_panel   | `#1B232D` | `#EAEEF2` | Panels |
| bg_raised  | `#252F3B` | `#F7F9FB` | Popups, hovered rows, cards |
| stroke     | `#33404F` | `#BCC6D1` | Dividers, outlines |
| text       | `#DDE4EC` | `#1C2733` | Body text |
| text_dim   | `#8A98A8` | `#5A6878` | Secondary text, hints |
| accent     | `#62CDEB` | `#0E83A8` | Active / selected only (mineral cyan) |
| warn       | `#E6B450` | `#9A6A00` | Warnings |
| error      | `#E0626A` | `#B8323C` | Errors |
| ok         | `#7BD88F` | `#2E8A45` | Success (vespene green) |

Dark is the default. Users can choose Dark, Light or Match system (View menu
and command palette). Tokens live in `crates/strata-app/src/theme.rs`; read
them with `theme::tokens(ctx)` instead of hard-coding colors.

## Type

- UI: **Barlow** (a slightly condensed industrial grotesk; OFL). Body 14px,
  small 12px, panel headings 16px semibold, window title 20px.
- Code (Galaxy script, raw XML): **JetBrains Mono** (OFL) 13px.

## Layout

```
┌ menu bar ─────────────── command palette hint (Ctrl+K) ───────────── ┐
├─┬───────────────────────────────────────────────────┬──────────────── ┤
│M│                                                   │ Inspector       │
│o│                 Map viewport (3D)                 │ (tool options / │
│d│                                                   │  selection)     │
│e│        floating brush HUD (bottom-center)         ├──────────────── ┤
│ │                                                   │ Minimap         │
│r├───────────────────────────────────────────────────┴──────────────── ┤
│a│ Triggers │ Data │ Text │ Script │ Problems │ History │ AI link      │
│i│                                                                     │
│l│                                                                     │
├─┴─────────────────────────────────────────────────────────────────── ┤
│ status: map name • size • problems           cursor x,y • height • fps│
└────────────────────────────────────────────────────────────────────── ┘
```

Content is left-aligned. Numbers in tables are right-aligned. Spacing grid: 4px.
Corner radius: 4px on controls, 6px on popups; the viewport has none.
