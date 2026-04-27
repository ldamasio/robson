# Product UI Kit — Robson Control Plane

Dense, operational product surface. Demonstrates how the RBX primitives compose into an actual product UI rather than a marketing page.

**Layout**
- `ProductShell` — sidebar + main split
- `ProductSidebar` — logo, quick-jump, grouped nav, user pill
- `ProductTopbar` — env/region breadcrumb, live status pill, git ref, deploy CTA

**Content**
- `KPIRow` — four tabular-numerics cards with status dots (ok / neutral / warn)
- `PortfolioChart` — SVG sparkline with brass stroke and date-range chips
- `StrategiesCard` — ranked list with PnL, latency, and LIVE/PAUSED state pills
- `SystemHealth` — service list with status dots, uptime and p50

**Design-system notes**
- Brass accent reserved for: "institutionalized" sidebar nav marker, chart line. Nothing else.
- All numbers use JetBrains Mono + tabular-nums for column alignment.
- Gain/loss color tokens (`--rbx-ok` / `--rbx-err`) from the semantic scale.
