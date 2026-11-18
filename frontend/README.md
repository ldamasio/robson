# RBX Robson Client

SvelteKit frontend and Tauri 2 client for Robson v3. Dual-domain web build:
`robson.rbx.ia.br` (pt-BR default) and `robson.rbxsystems.ch` (en default).

## Stack

- SvelteKit 2 + `@sveltejs/adapter-static`
- Tauri 2 for Android and desktop packaging
- TypeScript strict
- Custom design tokens from RBX Voltage System (no Tailwind)
- `svelte-i18n` for locale handling
- Google sign-in brokered by RBX Identity/ZITADEL (Authorization Code + PKCE)
- Vitest + Playwright

## Quickstart

```bash
pnpm install
cp .env.example .env.local
# fill in the real public RBX Identity client/project metadata
pnpm dev
```

## Scripts

| Command                                              | Purpose                                 |
| ---------------------------------------------------- | --------------------------------------- |
| `pnpm dev`                                           | Run dev server (http://localhost:5173)  |
| `pnpm build`                                         | Build static output to `build/`         |
| `pnpm preview`                                       | Preview production build                |
| `pnpm check`                                         | Run TypeScript + svelte-check           |
| `pnpm test`                                          | Run Vitest unit tests                   |
| `pnpm test:e2e`                                      | Run Playwright E2E tests                |
| `pnpm lint`                                          | Run Prettier + ESLint                   |
| `pnpm tauri:dev`                                     | Run the Tauri desktop development shell |
| `pnpm tauri:android:init`                            | Generate the Android Studio project     |
| `pnpm tauri:android:build -- --target aarch64 --apk` | Build an ARM64 Android APK              |

## Architecture

See `docs/adr/ADR-0025`, `ADR-0026`, `ADR-0027`, `ADR-0057`, `ADR-0058` and
`docs/implementation/FE-P1-FRONTEND-MVP.md`.

## Authentication modes

- Tauri owns the PKCE exchange, bearer token, REST requests, and SSE stream in
  Rust. The access token is process-memory-only and never reaches Web Storage
  or the WebView.
- Browser `bff` mode uses a Secure, HttpOnly session cookie.
- Browser `oidc` mode is an explicit static-hosting compatibility mode. It
  keeps the ZITADEL access token in JavaScript memory only; a reload signs in
  again through SSO.

The ZITADEL organization must be configured Google-only, the client must have
`accessTokenRoleAssertion=true`, and the authorize request is scoped to the
real RBX organization and Robson project audience. See ADR-0058. The example
files intentionally leave client/project/Google-IdP IDs blank when no
repository-verified value exists.

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
