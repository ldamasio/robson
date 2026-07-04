# Leandro Damasio — Design System

A complete, implementation-ready design system for the personal site of
**Leandro Damasio**, Computer Engineer and AI systems specialist.

**Targets:** `leandrodamasio.rbx.ia.br` (PT-BR) · `leandrodamasio.rbxsystems.ch` (EN)
**Stack:** Next.js 14 + Tailwind CSS + shadcn/ui

---

## Sources

| Resource | Location |
|---|---|
| Personal frontend codebase | `ldamasio/lda-front` (GitHub) |
| RBX Systems corporate site | `rbxrobotica/rbx-site` (GitHub) |
| Fonts (Geist / Geist Mono) | `lda-front/app/fonts/` → `fonts/` |
| Portrait photos | `lda-front/app/images/` → `assets/` |

---

## Product Context

### Who is Leandro Damasio?

Computer Engineer and AI systems specialist. Currently AI Engineer at
**Enforce** (BTG Pactual Group), São Paulo. Maintainer of the open-source
monorepo at **RBX Systems**, where he builds AI agents, code agents, and
system-level tooling. Background spanning AI/LLM systems, distributed
trading infrastructure, cloud-native platforms, and DevOps.

Education: Computer Engineering (UNIVESP), Master in Public Administration
(FGV/EAESP), Philosophy (UFSC). Languages: EN, PT, DE, IT, FR, ES.

**The deliberate ambiguity:** the site must not resolve the visitor's reading
into either "founder selling a product" or "engineer seeking employment." It
is both simultaneously. Phrasing, layout and visual tone all serve this
posture: institutional enough to feel like a firm, personal enough to feel
like one person's body of work.

### RBX Systems

Leandro's company. Products include: TruthMetal, Strategos (strategic
decision/situation room platform), trading/decision systems, internal AI
agents, rbx-infra (k8s GitOps cluster). The personal site is a "fork" of
the RBX brand — it inherits the institutional vocabulary but applies it at
a personal/authorial register.

---

## CONTENT FUNDAMENTALS

### Tone

Institutional-authorial. Declarative. Technical. Sober.

- Sentences are short and active.
- Third-person declaratives over first-person confessionals.
- Metrics in mono: `99.97%`, `p99: 48ms`, `12 yrs`.
- No em-dashes. No arrows. No emoji. No agency-speak.

### Casing

| Context | Rule |
|---|---|
| Body text, buttons, nav | Sentence case |
| Section labels (eyebrows) | UPPERCASE + tracking |
| Status indicators | ALL-CAPS MONO |
| Brand mark | Uppercased at discretion |

### Approved phrasings

- "Computer Engineer. AI systems for finance and high-reliability environments."
- "Based in Brazil, working across Zürich and São Paulo."
- "Selected work, 2018–2026."
- "Available for selected engagements." *(deliberately ambiguous)*
- Metrics: `99.97%`, `p99: 48ms`, `12 yrs`

### Prohibited

- "Hire me" / "Let's work together" / "Book a call"
- "Innovative solutions" / "Unlock the power of"
- "I'm passionate about…"
- Em-dashes (—), arrows (→ ↓), emoji

### Bilingual

EN default on `.ch` domain; PT-BR default on `.rbx.ia.br`. Copy parity
expected. DE basic (Zürich market signal). The existing codebase implements
7-language i18n (EN, PT, DE, ES, FR, IT, ZH).

---

## VISUAL FOUNDATIONS

### Philosophy

Structural, not decorative. Every element earns its place. Influenced by
Swiss institutional print: DIN-adjacent, systematic, calm.

> "A Swiss engineer's office that exists. Those who need it, find it."

### Colors

Dark-first palette. Near-black surfaces with cold neutral undertone.
One warm metallic accent (RBX-inherited brass). One cold personal accent
(new, authorial). Semantic colors calm and desaturated.

See `colors_and_type.css` for all tokens.

**Surface scale (dark):**
`#050507` → `#0A0A0B` → `#0F1012` → `#15161A` → `#1D1D22` → `#26272C` → `#33343A`

**Text scale:**
`#F5F5F3` primary → `#B4B5B8` secondary → `#72747A` labels → `#45474D` disabled

**Accent — brass/bone `#D9CBA3`:** RBX-inherited. One per surface. Emphasis only.

**Accent — steel blue `#5C7080`:** Personal/authorial. Chosen because it reads
as technical precision without the warmth of brass. Signals "this is one
person's work, not a firm's product page." Quietly distinct.

**Semantic:** ok `#7FB77E` · warn `#D9B55A` · err `#C56A6A` · info `#7A93B0`

**Rules:**
- One accent per surface maximum.
- Color signals hierarchy/status/emphasis — never decoration.
- No gradients on text. No gradients on large backgrounds.
- All backgrounds are flat.

### Typography

**Fonts:** Geist (variable, the existing site's implementation) + Geist Mono.
Production target: Inter (UI) + JetBrains Mono (data/code). Geist and Inter
are metrically compatible; Geist Mono and JetBrains Mono are visually
equivalent.

**Hierarchy is carried by weight, not size:**
- Display / H1: weight `300` (thin, institutional presence)
- Body: weight `400`
- Labels/UI: weight `500`
- Brand mark only: weight `700`

**Special patterns:**
- Eyebrows: UPPERCASE + `0.14em` tracking (e.g. `WORK`, `SELECTED PROJECTS`)
- Status mono: ALL-CAPS + `0.04em` tracking (e.g. `● SHIPPED`)
- Prose max-width: `42rem` (672px)
- Content max-width: `72rem` (1152px)

### Spacing

Base unit: 4px. Vertical rhythm between sections: `64–128px`.
Within components: `4–16px`. See `--space-*` tokens in `colors_and_type.css`.

### Borders

1px hairlines are the primary structural device. Sections separated by
`border-top` before padding. No ornamental borders.

### Radii

`2px` / `4px` / `8px` — small, consistent. Buttons: `6px`. Cards: `8px`.
Pills: `999px` (status labels only). Never mix radii within one composition.

### Shadows

Near-absent. `inset 0 1px 0 rgba(255,255,255,0.04)` highlight +
`0 1px 3px rgba(0,0,0,0.5)` drop. Nav gets a heavier `24px` drop.
Hairlines do the structural work; shadows are atmosphere only.

### Motion

- **Entry:** 500–800ms, `cubic-bezier(0.22, 1, 0.36, 1)`, translate `20–30px` + fade
- **Hover:** 160ms, color/border only. No scale, no lift, no bounce.
- **Press:** instantaneous (0ms), opacity 0.9.
- **Reduced motion:** translate disabled, opacity transitions kept.

### Backgrounds

Flat. Near-black surfaces. The existing codebase used a subtle grid dot
pattern (`radial-gradient` at 24px spacing) — this is acceptable as a
very-low-contrast texture, but the brief discourages it for the new system.
No large-scale background imagery. No hero photography. No gradients.

### Imagery

Portrait: small, square, B&W treatment, with hairline border.
Any photography should be dark-biased, desaturated or B&W.
No colourful product shots.

### Cards

Background: `--surface-card` (`#15161A`). Border: 1px `--surface-hairline`.
Radius: `8px`. Shadow: subtle inset highlight + drop. Hover: border-color
shifts to `--accent-brass`, no transform.

### Navigation

Floating pill with `backdrop-filter: blur(12px)`, glass-institutional
background (`hsla(222,47%,6%,0.7)`). Max-width: `80rem`. Sticky, `z-50`.
Border: 1px with low-opacity hairline.

### Icons

No icon system used in the existing lda-front codebase beyond Lucide React
(imported per-component: `Code`, `Link` from `lucide-react`). No icon fonts.
No emoji. No custom SVG illustrations. In artifacts, link Lucide from CDN
(`https://unpkg.com/lucide@latest`) or inline Lucide SVG paths.

### Light mode

Exists as a secondary mode. Same typographic system and layout. Surfaces
invert to warm near-whites. Light mode surfaces defined as `--light-*`
tokens in `colors_and_type.css`.

---

## ICONOGRAPHY

**System used:** Lucide React (stroke icons, 1.5px stroke weight, 24px base).
- Imported individually: `import { Code, Link } from "lucide-react"`
- CDN for artifacts: `https://unpkg.com/lucide@latest/dist/umd/lucide.js`
- No icon font. No emoji. No unicode characters as decorative icons.
- Unicode used functionally: `●` dot in status pills (plain text character).
- No custom drawn SVG illustrations.

**Key icons in use:**
- `Code` — for technical/project sections
- `Link` — for external links
- Navigation icons sourced from Lucide only

---

## Files

```
README.md                    — this file
SKILL.md                     — Claude skill definition
colors_and_type.css          — all design tokens (color, type, spacing, motion)

fonts/
  GeistVF.woff               — Geist variable font (sans-serif, UI/headings)
  GeistMonoVF.woff           — Geist Mono variable font (code/data/status)

assets/
  ldamasio-portrait.jpeg     — portrait photo (square crop)
  ldamasio-portrait-2.jpeg   — portrait photo (alternate)
  lugano.jpg                 — Lugano/Swiss reference photo

preview/
  colors-surfaces.html       — surface scale card
  colors-text.html           — text color scale card
  colors-accents.html        — accent + semantic colors card
  type-scale.html            — full type scale specimen
  type-eyebrow-status.html   — eyebrow + status pattern card
  spacing-tokens.html        — spacing token card
  spacing-radius-shadow.html — radius + shadow system card
  components-buttons.html    — button states card
  components-card.html       — card component card
  components-nav.html        — navigation pill card
  components-status-pill.html — status pill card
  components-footer.html     — footer card

ui_kits/
  personal-site/
    README.md                — kit overview
    index.html               — interactive prototype (Home screen)
    Home.jsx                 — Hero + Selected Work + Capabilities + Contact
    WorkDetail.jsx           — Project detail page
    Notes.jsx                — Notes index + Note detail
```
