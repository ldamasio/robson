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

| Command                  | Purpose                                                |
| ------------------------ | ------------------------------------------------------ |
| `pnpm dev`               | Run dev server (http://localhost:5173)                 |
| `pnpm build`             | Build static output to `build/`                        |
| `pnpm preview`           | Preview production build                               |
| `pnpm check`             | Run TypeScript + svelte-check                          |
| `pnpm test`              | Run Vitest unit tests                                  |
| `pnpm test:e2e`          | Run Playwright E2E tests                               |
| `pnpm lint`              | Run ESLint with TypeScript, Svelte, and security rules |
| `pnpm build:android:web` | Build the MOB-P1 read-only Android web bundle          |
| `pnpm android:sync`      | Build and copy the web bundle into the Android project |
| `pnpm android:run`       | Sync and run on an attached Android device             |

## Android MOB-P1

The Android client is a Capacitor delivery target of this SvelteKit app. The
first milestone is intentionally read-only: its token is memory-only, mutation
controls are hidden, and non-GET/HEAD API calls are rejected before `fetch`.

Capacitor 8 requires Node.js 22 or later. Android packaging also requires the
official Android SDK on an x86_64 host. The POCO can build and test the web
bundle through `scripts/poco-web-worker.sh`, but Gradle packaging remains on
the Linux Mint host.

```bash
pnpm install --frozen-lockfile
pnpm android:sync
adb devices
pnpm android:run
```

See ADR-0054 and
`docs/implementation/2026-08-29-android-client-mob-p1.md` for scope and
acceptance gates.

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

## Deploy

Production deployment: container in k3s rbx-infra cluster,
image at `ghcr.io/ldamasio/robson-frontend-v2`, ArgoCD-managed.

See `docs/runbooks/frontend-deploy.md` for the full deploy procedure. The GitHub Actions workflow `Frontend Build & Publish` builds the Docker image on push to main (or workflow_dispatch) and pushes to GHCR. ArgoCD reconciles the deployment in rbx-infra. Prerequisites B1–B7 must be satisfied by the operator before the first deployment; see runbook.

## Dashboard Semantics

- The dashboard operations panel renders only live positions that still occupy a slot.
- Terminal positions such as `Closed`, `Error`, and `Canceled` remain available in historical views, but are omitted from the live slot list.
- This keeps the slot grid aligned with the current risk surface: occupied slots on one side, available slots on the other.
