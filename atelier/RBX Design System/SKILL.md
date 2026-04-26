---
name: rbx-design
description: Use this skill to generate well-branded interfaces and assets for RBX Systems, either for production or throwaway prototypes/mocks/etc. Contains essential design guidelines, colors, type, fonts, assets, and UI kit components for prototyping.
user-invocable: true
---

Read the README.md file within this skill, and explore the other available files.
If creating visual artifacts (slides, mocks, throwaway prototypes, etc), copy assets out and create static HTML files for the user to view. If working on production code, you can copy assets and read the rules here to become an expert in designing with this brand.
If the user invokes this skill without any other guidance, ask them what they want to build or design, ask some questions, and act as an expert designer who outputs HTML artifacts _or_ production code, depending on the need.

## Quick orientation

- `README.md` — full brand, content, visual, and iconography guidelines. Read first.
- `colors_and_type.css` — canonical CSS tokens (colors, type, spacing, radii, motion). Import or copy.
- `assets/` — `rbx-mark.svg`, `rbx-wordmark.svg`.
- `ui_kits/app/` — Robson operations dashboard (React/Babel). Reusable components and a click-thru prototype.
- `ui_kits/site/` — Robson marketing landing. Static HTML.
- `preview/` — small token and component cards used by the design-system review pane.

## Core principles
- Dark-first, near-black, off-white text. Never pure black or pure white.
- One muted warm accent (`#C9A96E`). No gradients, no emoji, no playful fintech tropes.
- Inter + IBM Plex Mono. Monospace for all data, tokens, and timestamps.
- 4px base, 2/4/8 radii, no shadows except one overlay shadow.
- Copy is institutional, precise, technical. No hype.
