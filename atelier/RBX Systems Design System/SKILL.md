# RBX Systems Design System — SKILL

Operating instructions for any designer or engineer working inside this system. Read this first.

---

## Stack

- **Tokens** live in `colors_and_type.css` as CSS variables under the `.rbx` class. All kits apply `class="rbx"` to `<body>`.
- **Shared components** live in `ui_kits/_shared/` — `RbxPrimitives.jsx` (Container, Button, Icon, Card, StatusPill, PhaseTag, Field, RbxLogo) and `ChromeComponents.jsx` (TopNav, Footer).
- **Kits** live in `ui_kits/<name>/`. Each kit is a standalone HTML file + its own sections JSX.
- **Type**: Inter (sans) + JetBrains Mono (code/data). Both self-hosted; no Google Fonts runtime dependency.

## Non-negotiable rules

1. **Dark first.** The system is designed on `--rbx-ink` (#0A0A0B). Light surfaces do not exist yet. Do not invent them.

2. **Brass is earned.** `--rbx-accent` (#B08D3C) is reserved for:
   - "Institutionalized" phase tags
   - Active sidebar nav indicator (left border)
   - Live chart line / headline telemetry
   - The word "Atelier" in the top nav
   Never use it for decorative flourishes, hover states, or to "liven up" a section.

3. **Numbers are data.** Any numeric output — metrics, prices, percentages, times, latency, counts — uses:
   - `font-family: var(--rbx-font-mono)`
   - `font-variant-numeric: tabular-nums`
   Headlines that happen to contain a number (e.g. `99.97%` in a KPI card) keep Inter but still get `tabular-nums`.

4. **Eyebrows over headings.** Every section opens with an `<Eyebrow>` — 11px, uppercase, 0.14em tracking, `--rbx-fg-dim`. Use them as the primary hierarchy cue; reserve H2s for actual titles.

5. **Hairlines separate; shadows elevate.** Prefer a 1px `--rbx-line` border over a drop shadow. Only use shadows on floating UI (dropdown menus, tooltips).

6. **Corner radii are small.** 4 / 6 / 8px. Never exceed 8px except on the logo dots and full-round pills (`999px`).

## Composition patterns

**Three-column numbered cards** (`Capabilities`, principles on README): hairline left border + mono serial number + 18/500 title + 13/1.6 body.

**Tabular index** (`Products`, `AtelierIndex`, `StrategiesCard`): grid with fixed-width meta columns on the left/right, flexible description in the middle. First/last columns are mono; middle is Inter.

**KPI card row**: 4-up grid, no gap, single outer border, internal dividers. Big numeric (36–44px, weight 300), eyebrow label above, mono sub-caption below, optional status dot top-right.

**Section rhythm**: `padding: 64–96px 0`, separator is always `border-top: 1px solid var(--rbx-line)`. No section gets a colored background — the contrast comes from content density, not surface.

## Accessibility targets

- Body minimum 13px. Mono meta minimum 10px — only on short labels, never on running text.
- All interactive elements ≥ 32px hit target. Buttons at the default `md` size are 36px tall.
- Focus ring: use `--rbx-line-strong` on border, never remove outlines on keyboard focus.

## Files to read, in order

1. `README.md` — product story and repo inventory
2. `README.html` — visual index
3. `colors_and_type.css` — tokens (source of truth)
4. `preview/` — one card per token (open in the Design System tab)
5. `ui_kits/_shared/*.jsx` — primitives
6. `ui_kits/<kit>/` — full surfaces

## When you add a new surface

- Use the `.rbx` class on body; import `colors_and_type.css`.
- Start with primitives from `_shared/RbxPrimitives.jsx`. Do not inline-redefine buttons, pills, or cards.
- If a section needs a new primitive, add it to `RbxPrimitives.jsx` and export it via `Object.assign(window, ...)`.
- Put page-specific layout in a sibling `<Name>Sections.jsx` file — never in `_shared/`.
- New semantic color? Add the token to `colors_and_type.css` first, then consume it. Raw hex values in JSX are forbidden outside the tokens file.
