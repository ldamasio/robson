# RBX Systems — Design System

> Institutional. Zurich-coded. Dark-first. Engineered, not decorated.

RBX Systems develops software and AI systems for financial and high-reliability environments: trading engines, decision platforms, and data validation systems. This design system encodes the visual and editorial language used across the marketing site, product surfaces, and institutional materials.

**Principles**
- Structural, not decorative. Every element serves a purpose.
- Interfaces reflect system state. Clarity > ornament.
- Restrained color. High signal-to-noise ratio.
- Typography carries hierarchy.
- Calm, precise, dependable. Not playful, not aggressive.

---

## Sources

- **Codebase:** `rbxrobotica/rbx-robotica-frontend` (Next.js 14 + Tailwind + shadcn/ui). Key files read:
  - `app/globals.css`, `tailwind.config.ts` — Tailwind/shadcn token layer
  - `app/atelier/AtelierContent.tsx` — the most Zurich-coded surface, most-aligned reference
  - `app/page/views/{main,about,header,footer}` — marketing surfaces
  - `app/data/products/productsData.ts` — product catalog + phase taxonomy
  - `docs/WRITING-STYLE.md`, `.claude/…/feedback_zurich_coded.md` — editorial direction
  - `blog-posts/*.mdx` — tone reference
- **Assets:** `public/bitmap.svg` (logo), `public/bitmap_bg.svg` (hero graphic), `public/diamond-sunset.svg`, `public/polka-dots.svg` (section patterns) — copied into `assets/`.

---

## Index

| File | Purpose |
|---|---|
| `README.md` | This file — foundations, content, visuals, iconography |
| `SKILL.md` | Skill manifest for Claude Code / Agent Skills |
| `colors_and_type.css` | Design tokens — colors, type, spacing, radii, shadows, motion |
| `assets/` | Logos, patterns, hero graphics |
| `preview/` | Rendered cards (typography, color, components, spacing, brand) |
| `ui_kits/marketing/` | Marketing site UI kit (nav, hero, about, products, footer) |
| `ui_kits/atelier/` | RBX Atelier surface — the most Zurich-coded reference |
| `ui_kits/product/` | Product / operations UI kit (TruthMetal-style control plane) |

---

## Content Fundamentals

The RBX voice is **institutional, engineering-led, Zurich-coded**. It reads like a Swiss B2B systems firm: sober, structural, precise. It explicitly rejects agency speak.

**Persona.** Write as *"RBX Systems"* or *"the RBX engineering team"* — never as a faceless brand, never as a single individual. Use first-person plural sparingly; prefer declarative system statements.

**Tone tests.** Before shipping a sentence, ask: *Does this sound like it could come from a Swiss engineering firm?* If not, rewrite.

- Less adjective, more architecture.
- Less creativity, more precision.
- Less marketing, more system.
- Less agency, more engineering.

**Casing.** Sentence case for body and buttons. Title Case for headings only when scanning structure matters. **UPPERCASE + `0.14em` tracking** for eyebrows, labels, and section markers (e.g. `POSITIONING`, `CAPABILITIES`, `WHAT WE DO`). State labels are ALL-CAPS mono (`SUBMITTED`, `CANONICAL`, `REJECTED`).

**"We" vs "You".** Prefer neutral declaratives about the system over second-person calls-to-action. *"Parameters move through four states"* beats *"You can track your parameters…"*. Use *"you"* only in direct instructions (forms, CTAs).

**Editorial rules (mandatory).**
- **No em-dashes (—).** Use periods or commas.
- **No arrows (→, ↓, ⇒).** Use prose or bullets.
- **No filler.** Drop *essentially*, *basically*, *actually*.
- Active voice, short sentences, trust the reader.
- **No emoji.** Not in UI, not in marketing, not in blog.
- **No sensitive infra.** No IPs, hostnames, credentials.

**Good examples (from the corpus).**
- *"Systems engineering for operations that demand control."* (hero)
- *"We treat software as operational infrastructure. Every system we deliver is designed to be maintained, observed and evolved safely over years, not just to work on deployment day."* (positioning)
- *"A parameter moves through four states: SUBMITTED, VALIDATING, CONSENSUS_PENDING, and CANONICAL. If consensus fails, it becomes REJECTED."* (technical)
- *"We work with a limited number of projects per cycle."* (atelier CTA)

**Anti-patterns (rewrite on sight).**
- *"Transformamos ideias em realidade"* / *"soluções inovadoras"* / *"presença digital"*
- *"Essentially provides a way to manage…"* → *"Manages…"*
- *"Unlock the power of AI"* — never.

**Numerals.** Spell out zero to nine in prose; use digits for 10+ and for all metrics, counts, states, and timings. Metrics use the mono face (`99.97%`, `p99: 48ms`).

**Bilingualism.** Site is bilingual (en / pt-BR). English default on `rbxsystems.ch`, Portuguese default on `rbx.ia.br`. Portuguese uses full UTF-8 diacritics — never ASCII approximations.

---

## Visual Foundations

### Color

The system is **dark-first**. Light mode exists in the codebase but is de-prioritized — institutional surfaces run dark.

**Surfaces (dark scale).** Near-black, cool-neutral, narrow step.
`#050507` page → `#0A0A0B` primary → `#0F1012` raised → `#15161A` card → `#1D1D22` footer → `#26272C` hairline → `#33343A` strong divider.

**Foreground.** Warm off-whites.
`#F5F5F3` primary text → `#B4B5B8` secondary → `#72747A` labels/captions → `#45474D` disabled.

**Accents.** Restrained and rare.
- `#D9CBA3` (warm metallic / brass-bone) — the preferred institutional accent for emphasis and highlights.
- `#01FFFF` (cyan) — legacy product accent inherited from the current site. Use **sparingly**, for product-facing surfaces only. Not for institutional work.

**Semantic.** Calm, desaturated. `#7FB77E` ok · `#D9B55A` warn · `#C56A6A` err · `#7A93B0` info. These are muted enough to sit quietly on dark surfaces without screaming.

**Rules.**
- At most one accent color per surface. Two is noise.
- Color only carries hierarchy, status, or emphasis — never decoration.
- No gradients on text. No gradients on large surfaces. The legacy `gradient-bg` goo effect on `main.tsx` is explicitly de-emphasized in favor of flat near-black.
- Backgrounds are flat. Patterns (`polka-dots.svg`, `diamond-sunset.svg`) are available but used at low contrast and small scale only.

### Typography

**Families.** Inter (UI + headings + body), JetBrains Mono (data, code, status, metrics). The codebase ships Geist; we substitute Inter as the canonical face per spec.

> Substitution note: the repo uses Geist (via `localFont`). We've replaced it with **Inter** (Google Fonts) as specified. If this deviates from your intent, ship the Geist `.woff` files and swap the `@import`.

**Hierarchy.** Weight, not size, does most of the work. Display (H1) at `300` weight for a thin, institutional presence. Body at `400`. Labels at `500`. Never `700+` except for brand mark.

- H1 64 / 300 / -0.015em
- H2 36 / 400 / -0.015em
- H3 22 / 500
- Lead 18 / 300 — used for positioning paragraphs
- Body 15 / 400 / 1.55
- Caption 13 / 400, muted
- Eyebrow 11 / 500 / UPPER / 0.14em
- Status 11 / 500 / UPPER / 0.04em (mono)

**Column width.** `42rem` (672px) max for prose. Wider columns feel like marketing, narrower feel like poetry. Neither is wanted.

### Spacing

4px base, preferring generous vertical rhythm between sections (64–128px) and tight rhythm within components (4–16px). Sections on institutional surfaces are separated by `border-top: 1px solid var(--rbx-line)` rather than padding alone — the hairline **is** the structure.

Scale: `4 · 8 · 12 · 16 · 24 · 32 · 48 · 64 · 96 · 128`.

### Borders, radii, shadows

- **Borders.** 1px, `var(--rbx-line)`. Hairlines are a primary structural device. Avoid thicker borders except on explicit focus rings.
- **Radii.** Small. `2 / 4 / 8px`. Buttons `6px`. Cards `8px`. Pills `999px` for status only. Never aggressive roundness. Rounded-2xl (16px) exists in the codebase and is considered legacy — prefer `8px`.
- **Shadows.** Nearly absent. A 1px inset highlight at 3–5% white plus a long, soft, near-black drop is all most surfaces need. Never colored shadows.

### Backgrounds & imagery

- Full-bleed imagery is rare. When used, images are dark-biased, cool, grainless, **b&w or near-mono**. Warm filters forbidden. No bright product photography.
- Team photos: square, subtle border, minimal chrome. Sit over a repeating low-contrast pattern (`polka-dots.svg`) for rhythm.
- Brand patterns (`diamond-sunset.svg`, `polka-dots.svg`) at ≤10% opacity, background-repeat, used to differentiate one positioned card from a flat surface. Never as hero.
- No hand-drawn illustrations. No isometrics. No 3D renders.

### Motion

- Entry: 500–800ms, `ease-out`, short translate (20–30px) + opacity fade. Framer `staggerChildren: 0.1–0.15`.
- Hover: 120–200ms, color/border only. **No scaling, no lifting, no bouncing.**
- Press: instant. Slight opacity change (0.9). No shrink.
- Reduced motion: disable translate, keep opacity.

### Hover / press / focus

- **Hover.** Link: border color shifts from `--rbx-line-strong` to `--rbx-fg`. Button: background shifts one step (primary → primary/90). Icon button: `color` shift only.
- **Press.** Opacity 0.9. No transform.
- **Focus.** 1px ring in `--rbx-line-strong` + 2px offset. Never a glow.
- **Disabled.** Opacity 0.5, no pointer events. No color change.

### Transparency & blur

- **Blur is allowed once:** the sticky top nav (`backdrop-filter: blur(12px)` over `rgba(bg, 0.6)`). Nowhere else.
- Avoid stacked transparencies. Prefer flat surfaces.

### Layout rules

- Top nav is **pill-shaped**, floating, max-width `80rem`, `16px` from top and sides, with hairline border and backdrop-blur.
- Footer is a solid darker surface (`#1D1D22`) separated by hairline.
- Content columns: 72rem max (1152px). Prose columns: 42rem max.
- Grid gutters: 16–32px. Section padding: 96–128px vertical.

### Corner rhythm

Mixing radii is forbidden within a composition. If the card is `8px`, the button inside is `6px`, the pill is `999px` — and nothing else has a radius.

---

## Iconography

The codebase uses **Lucide** (via shadcn/ui) and **React Icons `si`** (Simple Icons) for brand marks (GitHub, LinkedIn). There is no custom icon font or sprite.

**Rules.**
- Icons are **line**, stroke `1.5`, never filled except for status dots.
- Neutral color — icons inherit `--rbx-fg-muted` or `--rbx-fg-dim`; they do not carry accent color.
- 16–20px at body size, 24px for section markers. Never decorative at >32px.
- No emoji. Ever.
- No unicode glyphs as icons, except `•` for inline separators and `/` for path separators.

**CDN usage.** This design system links Lucide from CDN (`https://unpkg.com/lucide@latest`). Brand marks (GitHub, LinkedIn) use inline SVGs copied from Simple Icons. Substitution flag: the repo bundles these via npm; we use CDN equivalents. If you need an offline build, inline the SVGs.

**Status indicators.** A `6px` filled circle, color from the semantic palette, precedes ALL-CAPS mono state labels. This is the canonical status pattern (e.g. `● HEALTHY`, `● DEGRADED`, `● REJECTED`).

---

## Preview cards

Rendered swatches, specimens, components in `preview/`. These populate the Design System tab.

## UI kits

- `ui_kits/marketing/` — the rbxsystems.ch homepage: nav, hero, positioning, products, team, footer.
- `ui_kits/atelier/` — the most Zurich-coded reference in the corpus.
- `ui_kits/product/` — an internal control-plane surface inspired by TruthMetal's state machine.

Each kit has its own `README.md`, an `index.html` click-thru, and small JSX components you can lift.
