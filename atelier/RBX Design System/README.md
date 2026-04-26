# RBX Design System

Design system for **RBX Systems** — software and AI systems for financial and high-reliability environments. Built to reflect the work itself: trading engines, decision platforms, and data validation systems where correctness and clarity are non-negotiable.

Design at RBX is **structural, not decorative**. The goal is interfaces that feel stable, predictable, and trustworthy in contexts involving capital and decision-making. Zurich-coded: institutional tone, restrained color, strong typography, high signal-to-noise ratio.

> "Robson is concerned with what happens after a trading decision is made… The execution and risk layers are the primary concern." — from the Robson repo README. The design system mirrors the engineering stance: quiet, deterministic, auditable.

---

## Sources

This system was derived from:

- **Repository:** `ldamasio/robson` — the Robson execution & risk engine. Frontend at `apps/frontend/` (React + Vite + react-bootstrap). The existing palette (slate-900 bg, blue→violet gradient) was used as a **starting point**, then refined into a more restrained institutional system.
- **Notable existing files:**
  - `apps/frontend/src/index.css` — current CSS variables and utilities (`--bs-body-bg`, `.text-gradient`, `.bg-glass`, `.card-premium`)
  - `apps/frontend/src/components/logged/RobsonCommandDock.css` — the CLI-style command dock (JetBrains Mono, status chips, prompt line) — the most on-brand artifact in the codebase
  - `apps/frontend/src/components/common/Header.jsx` & `Footer.jsx` — layout shell
  - `apps/frontend/src/screens/HomeScreen.jsx` — marketing hero & feature cards
  - `apps/frontend/src/screens/LoggedHomeScreen.jsx` — logged-in dashboard shell

---

## Products

1. **Robson app** — the logged-in operations dashboard. Portfolio, positions, trading intents, pattern detection, command dock.
2. **Robson marketing site** — the `/` landing with hero, feature cards, CTA.

Both share the same visual language; the app skews denser and more monospaced, the site skews more spacious.

---

## Refinements from the source

The existing Robson UI is fintech-dark but leans playful in places (emoji feature cards, blue→violet gradient text, rounded-pill CTAs, `card-premium` hover lift). This system **keeps the dark-first, high-contrast stance** and **drops** the consumer-fintech tropes:

| Dropped | Replaced with |
|---|---|
| Blue→violet gradient text (`.text-gradient`) | Off-white wordmark, single muted accent |
| Emoji feature cards (🛡️ ⚡ 📱) | Typographic numerals (01 / 02 / 03) and a small iconographic set |
| Hover lift + glow on cards | Border-color shift only, no translate |
| Rounded-pill primary buttons | 2px radius, square-ish buttons |
| Bootstrap body (`min-vh-100` flex stack) | 8-point grid with explicit max-widths |

What's **preserved** from the source:
- Near-black slate background and off-white text
- Inter as primary UI font
- JetBrains / Mono for data and command surfaces
- Status chip vocabulary (positive / negative / warning / neutral)

---

## Content fundamentals

**Tone.** Institutional, precise, technical. Written for engineers and researchers, not retail traders. No hype, no exclamation marks, no "unlock," no "supercharge." If the product README says "Robson is not a trading bot," UI copy should inherit that refusal to oversell.

**Voice.** Third-person system voice ("The engine enforces…"), or direct imperative ("Validate plan," "Acknowledge risk"). Avoid "we" / "our." Use "you" sparingly, only when addressing the operator directly ("You are in demo mode").

**Language.** Use domain vocabulary exactly — *plan, validate, execute, stop, liquidation distance, exit reason, dry-run*. Don't soften technical terms. "Position closed: stop hit" beats "Your trade has ended."

**Casing.** Sentence case for UI labels and headings. UPPERCASE reserved for status tokens in monospace (`PLAN`, `VALIDATE`, `EXECUTE`, `LIVE`, `DRY-RUN`) and section labels in small caps (`Portfolio · BTC`).

**Numbers.** Always tabular, monospace, with explicit precision. `0.00482 BTC`, not `~0.005 BTC`. Show currency and decimals, show zero fills.

**Emoji.** **Not used.** The Robson frontend currently uses emoji feature cards — these are explicitly deprecated by this system.

**Examples (good):**
- `Plan validated. Dry-run ready.`
- `Stop triggered · BTCUSDT · -1.24%`
- `Liquidation distance: 12.40%`
- `Execution requires --live --acknowledge-risk`

**Examples (avoid):**
- ~~"🚀 Ready to supercharge your trading?"~~
- ~~"Smart Trading Made Simple"~~ (current homepage H1 — leans marketing; acceptable on landing but never in-product)
- ~~"Oops! Something went wrong."~~

---

## Visual foundations

**Palette.** Dark-first. Near-black background (`--bg-0: #08090B`), with two surface tiers above it (`--bg-1: #101215`, `--bg-2: #16191D`). Text is off-white (`--fg-0: #ECECEC`), never pure white. The accent is a single muted amber (`--accent: #C9A96E`) — warm metallic, used sparingly for emphasis and interactive focus. Semantic status colors are muted, not saturated: positive `#6AA77B`, negative `#C26B6B`, warning `#C9A96E`, info `#7891A8`. No blue→violet gradient.

**Typography.** `Inter` (variable) for all UI, `IBM Plex Mono` for data, status tokens, timestamps, and the command dock. Inter is loaded from Google Fonts as a fallback; see `fonts/`. Heading scale tops out at 40px — this is an instrument panel, not a landing page. Line-height is tight on headings (1.1) and generous on prose (1.6).

**Spacing.** Strict 4px base unit, 8-point grid for layout (`4, 8, 12, 16, 24, 32, 48, 64`). Components have consistent internal padding (cards = 24px, buttons = 12px/16px). Section spacing is 64px on desktop, 32px on mobile.

**Backgrounds.** Flat near-black. **No hero gradients, no mesh, no images behind content.** Full-bleed imagery is rare and only used for case studies; when used, it's black-and-white with subtle grain. Repeating patterns are permitted only as 1px hairline grids on data surfaces. No hand-drawn illustrations.

**Borders.** 1px, single color (`--border: #24282E`). Emphasis borders step to `--border-strong: #3A4048`. No shadows to simulate elevation — elevation is communicated by background tier, not shadow.

**Shadows.** None by default. A single `--shadow-overlay` (for modals, dropdowns) exists: `0 8px 24px rgba(0,0,0,0.5)` — flat, neutral, no colored glows.

**Corner radii.** 2px (`--radius-sm`) for buttons and inputs, 4px (`--radius-md`) for cards and panels, 8px (`--radius-lg`) for modals and top-level containers. **No pill-shaped buttons.** No 16px+ radii.

**Cards.** Flat rectangles. Background `--bg-1`, border `--border`, radius 4px, no shadow. On hover, only the border shifts to `--border-strong` — no translate, no glow, no scale. This is deliberate: cards are containers, not clickable affordances.

**Hover states.** Tight and restrained. Text links → `--accent`. Buttons → background steps one tier up. Cards → border-color shift only. No opacity changes as hover state (too fuzzy for an instrument).

**Press states.** Background steps one tier down. Buttons do not shrink or translate. No ripple effects.

**Focus states.** 2px `--accent` outline with 2px offset. Keyboard focus must be visible — this is non-negotiable for an operations tool.

**Animation.** Minimal and short (120–180ms). Easing is `cubic-bezier(0.2, 0, 0, 1)` — a standard material-style decelerate curve. **No bounces, no springs, no entrance animations.** State changes fade/shift; they don't perform.

**Transparency & blur.** Used only for overlays (modal scrims, sticky headers). `rgba(8,9,11,0.72)` + 8px backdrop-blur for sticky nav. Never used decoratively inside content.

**Imagery.** Rare. When used: black-and-white photography, subtle grain, architectural or industrial subject matter. Never stock "diverse team at laptop." Never abstract 3D renders or AI-generated gradients.

**Layout rules.** Fixed header (64px), optional fixed command dock (floating bottom, centered, 1100px max). Content uses a 12-column grid at 1200px max-width. Data-heavy views can go full-bleed.

**Icons.** Lucide, 1.5px stroke, 16px or 20px only. Currency/crypto marks use proper SVG logos where possible. See `ICONOGRAPHY` below.

---

## Iconography

**Icon system:** [Lucide](https://lucide.dev) — loaded from CDN (`lucide@latest`). Stroke-based, 1.5px, neutral. Chosen over Heroicons for its broader coverage of finance/data symbols and monoline consistency.

**Sizes:** 16px (inline with body), 20px (buttons, list items), 24px (section headers). Never larger in-product; 32px only permitted in empty-states.

**Color:** Icons inherit `currentColor`. Default `--fg-2` (dimmed), `--fg-0` on active/hover. Status icons pick up the corresponding status color.

**SVGs:** Wordmark and monogram live in `assets/` as SVG. One `rbx-mark.svg` (monogram) and one `rbx-wordmark.svg` (full lockup). The codebase currently has no dedicated logo asset — this system introduces one.

**Crypto/asset marks:** Use the `cryptocurrency-icons` CDN for trading-pair symbols (`BTC`, `ETH`, `USDT`). Never hand-draw.

**Emoji:** Not used. The existing frontend uses 🎯, 🛡️, ⚡, 📱, 👁️, 🔑 — these must be replaced with Lucide icons in any new work.

**Unicode:** A small set is permitted for tabular data — `↑` `↓` `→` for trend arrows, `·` as separator, `—` (em-dash) for null / not-applicable. Never ✓ ✗ for status (use Lucide `check` / `x`).

---

## Index

Root files:

- `README.md` — this document
- `SKILL.md` — agent-invocable skill manifest (Claude Code compatible)
- `colors_and_type.css` — CSS custom properties for the full token system
- `fonts/` — Inter + IBM Plex Mono (Google Fonts `@import` in the CSS; no local files yet)
- `assets/` — logos, marks, and reference SVGs
- `preview/` — small HTML cards that populate the Design System tab
- `ui_kits/` — interactive recreations of the two products
  - `ui_kits/app/` — Robson operations dashboard (dashboard, positions, command dock, trading intent)
  - `ui_kits/site/` — Robson marketing site (hero, feature grid, footer)

## Caveats

- The source repo ships no dedicated logo asset — the wordmark/monogram SVGs in `assets/` are **new** for this system and should be reviewed.
- Fonts load from Google Fonts CDN. No local `.ttf` files are bundled. For production, self-hosting is recommended.
- The existing Robson frontend uses `react-bootstrap`; this system is framework-agnostic and does not attempt to re-implement Bootstrap classes.
