ADR-0026: RBX Voltage Brand System

Status: Accepted
Date: 2026-04-23

Context
- RBX Systems (the holding) operates multiple products: Robson (trading execution engine, live in prod), TruthMetal (validator / state machine, planned), and future systems. Each product previously risked developing its own visual language, fragmenting the brand.
- Two prior design-system explorations exist under `atelier/` (`RBX Design System` and `RBX Systems Design System`), both derived from different codebases with incompatible palettes (one amber-led, one brass + legacy cyan). Neither was promoted to canonical.
- The legacy production logo (`rbx-robotica-frontend/public/bitmap.svg`) is an angular cyan letterform `#00FFFF` with radial gradient stroke. High contrast, memorable, but risks feeling dated-tech-bro at pure saturation.
- The brand target is Zurich + Zug coded: Swiss institutional restraint plus Crypto Valley modernity. Anthropic's strategy (single brand accent bridging light marketing and dark product) was studied and informs the approach, but the specific aesthetic differs — RBX is dark-first throughout, not light-brand-plus-dark-product.

Decision
- Adopt the **RBX Voltage System** as the unified brand system for RBX Systems and all products.
- **Dark-first everywhere.** Brand marketing and product apps share the same warm-dark palette. No light-mode brand layer.
- **Single-accent cyan family.** Five-step scale from signal to subtle. Product differentiation is achieved via lockup + UI chrome + density, **never by sub-accent color**.
- **Typography**: Inter (all UI, headings, body) + JetBrains Mono (data, status, timestamps, command dock). Serif explicitly rejected for system coherence.
- **Signature elements (three-layer)**:
  1. L-corner brackets on cards/sections (8–10px, `--cyan-brand` 40% opacity)
  2. Voltage hairline sweep on hover/focus (1px, 80ms, cyan)
  3. Tick ruler data marks on axes/borders/timelines (engineer's ruler aesthetic)
- **Logo**: Option B (refined bitmap.svg — letterform preserved, color `#22E5E5`, radial gradient removed, warm-dark frame). Option C (geometric R monogram in Swiss Style) archived as alternative.
- **Wordmark pattern**: `RBX` (holding) / `RBX Systems` (engineering firm) / `RBX Robson` (product) / `RBX TruthMetal` (product). No separator between tokens.
- **Palette** (complete token set documented in `~/docs/rbx-brand-voltage-system.md` and `atelier/brand-voltage/colors_and_type.css`):
  - Backgrounds (warm-dark): `#07080A` / `#0D0F12` / `#13161A` / `#1A1E23`
  - Foreground: `#ECECEC` / `#B8BCC2` / `#7A7F87` / `#4A4F56`
  - Cyan scale: signal `#00FFFF` / brand `#22E5E5` / muted `#06B6B6` / dim `#0B6E6E` / subtle `rgba(0,255,255,0.08)`
  - Semantic (status only): ok `#7FB77E` / warn `#D9B55A` / err `#C56A6A` / info `#7A93B0`
  - Radii: 2 / 4 / 8 / 999-pill-status-only
  - Spacing: 4px base, 8pt rhythm
  - Grid: 12-col 1152px desktop, 8-col tablet, 4-col mobile, 42rem prose
- **Iconography**: Lucide subset + 20 RBX-custom domain glyphs covering trading / systems / state-machine concepts Lucide does not ship.

Consequences
- Positive
  - Single brand identity that scales across marketing, product, and future systems without fragmentation.
  - Swiss restraint + Zug modernity: rigorous enough for capital-operating product, elegant enough for institutional marketing.
  - Single-accent discipline forbids decorative color, reinforcing the audit/instrument aesthetic.
  - Warm-dark background differentiates from cold-dark competitors (Linear, Vercel at cooler near-black) without breaking the dark-first rule.
  - Signature elements (L-corners, voltage hairline, tick rulers) give a distinctive visual DNA no competitor SaaS uses together.
- Negative / Trade-offs
  - Dark-only brand is bolder than the Anthropic-style light-warm-plus-dark-product split. Requires discipline: the marketing site must convince institutional clients without a traditional "warm welcome" layer.
  - Custom icon set adds build-time tooling and ongoing maintenance (vs using Lucide exclusively).
  - No Tailwind / no shadcn-svelte means every interactive component is built from primitives. Slower first implementation, faster subsequent iteration.
  - Single-accent rule means products cannot visually differentiate by color. Mitigation: chrome + density + wordmark handle differentiation convincingly.

Alternatives
- **RBX Metals System** (brass holding / amber Robson / steel TruthMetal) — rejected. Reads too Zurich-institutional-heavy, loses Zug-coded lightness, risks corporate-bureau feel.
- **RBX Warm Duality** (light warm brand à la Anthropic + dark product) — rejected. Warm accent (Tomato) carries memory of legacy Porzioni D'Italia food brand. Not RBX-native. Also: Anthropic peach works for Anthropic; RBX's engineering-first posture is better served by dark throughout.
- **Keep cyan `#00FFFF` everywhere at pure saturation** — rejected. Pure saturation drifts toward 2010s hacker/gaming aesthetic. Downgraded to `--cyan-signal` for critical moments only; `--cyan-brand #22E5E5` used as default brand cyan.
- **Per-product sub-accents** — rejected. Violates single-accent discipline of Swiss Style canon (Müller-Brockmann, Karl Gerstner) and Anthropic's own strategy. Fragmentation risk too high.
- **Serif typography (Fraunces, GT Alpina) on brand** — rejected. Warm serif does not cohere with cyan voltage aesthetic.

Implementation Notes
- Token file: `apps/frontend-v2/src/lib/design/tokens.css` (generated from `atelier/brand-voltage/colors_and_type.css`).
- Logo assets: `apps/frontend-v2/static/brand/` (copied from `atelier/brand-voltage/marks/` and `atelier/brand-voltage/wordmarks/`).
- Icon generation: build script reads SVGs from `src/lib/icons/rbx/*.svg` and produces Svelte component wrappers with typed props.
- Signature elements implemented as:
  - L-corners: Svelte component `<LCorners>` or CSS pseudo-elements on `.card`.
  - Voltage hairline: CSS animation triggered on `:hover` / `:focus-visible`.
  - Tick ruler: SVG helper `<TickRuler>` component or CSS `background-image` repeating pattern.
- Related agnostic doc: `~/docs/rbx-brand-voltage-system.md` (source of truth).
- Brand assets repo location: `/home/psyctl/apps/robson/atelier/brand-voltage/` (marks, wordmarks, tokens, preview HTML).
- Preview: `atelier/brand-voltage/preview/voltage-preview.html` (approved 2026-04-23).
- Explicit discards enforced in code review: no em-dashes, no arrows, no emoji, no gradients on text or large surfaces, no hover lifts, no mixing radii within a composition.
