# RBX Robson Frontend v2

SvelteKit static frontend for Robson v3. Dual-domain:
`robson.rbx.ia.br` (pt-BR default) and `robson.rbxsystems.ch` (en default).

## Stack

- SvelteKit 2 + `@sveltejs/adapter-static`
- TypeScript strict
- Custom design tokens from RBX Voltage System (no Tailwind)
- `svelte-i18n` for locale handling
- Auth.js for GitHub OAuth (to be wired in EP-003)
- Vitest + Playwright

## Quickstart

```bash
pnpm install
cp .env.example .env.local
# fill in AUTH_GITHUB_ID / AUTH_GITHUB_SECRET / AUTH_SECRET
pnpm dev
```

## Scripts

| Command | Purpose |
|---------|---------|
| `pnpm dev` | Run dev server (http://localhost:5173) |
| `pnpm build` | Build static output to `build/` |
| `pnpm preview` | Preview production build |
| `pnpm check` | Run TypeScript + svelte-check |
| `pnpm test` | Run Vitest unit tests |
| `pnpm test:e2e` | Run Playwright E2E tests |
| `pnpm lint` | Run Prettier + ESLint |

## Architecture

See `docs/adr/ADR-0025`, `ADR-0026`, `ADR-0027` and
`docs/implementation/FE-P1-FRONTEND-MVP.md`.

## Brand

Design tokens: `src/lib/design/tokens.css`
Logo + wordmarks: `static/brand/`
Source of truth: `brand-voltage/` at repo root.

## Path aliases

- `$design` → `src/lib/design`
- `$api` → `src/lib/api`
- `$stores` → `src/lib/stores`
- `$components` → `src/lib/components`
- `$icons` → `src/lib/icons`
- `$i18n` → `src/lib/i18n`
