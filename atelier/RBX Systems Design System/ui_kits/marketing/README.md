# Marketing UI Kit

Full recreation of the `rbxsystems.ch` homepage flow, built with the RBX design tokens.

**Screens / sections**
- `TopNav` — floating pill nav with hover mega-menus (About / Services dropdowns)
- `Hero` — thin-weight display H1 + supporting lead + primary/outline CTAs + logo
- `PositioningRow` — asymmetric "Positioning" block with lead paragraph
- `Capabilities` — 3-column numbered hairline-left cards (Architecture / Governance / Operations)
- `Products` — tabular product index with phase tags (Institutionalized, Structuring, Seed)
- `Team` — 5-person grid on `polka-dots.svg` rhythm background
- `Footer` — 4-column link grid on `surface-3`

**Components used**
`Container`, `Hair`, `Eyebrow`, `Button`, `PhaseTag`, `RbxLogo`, `RbxMark`, `Icon`, `TopNav`, `Footer`.

**Intentional gaps**
- `Blog` list and `Services` detail page not recreated — they reuse the same primitives with a different composition.
- Product phase tags use `accent` (brass) only for `Institutionalized` — matches the "earned gravitas" rule.

**Known fidelity notes**
- The repo's homepage ships an animated goo-gradient background behind the hero. Per the Zurich-coded brief, we have removed it in favor of flat `--rbx-ink`. The file is present in `assets/bitmap_bg.svg` if you want it back.
- Team photos are not included in the repo imports. We substitute initials discs.
