# RBX Robson Frontend v2

SvelteKit static frontend for Robson v3. Dual-domain:
`robson.rbx.ia.br` (pt-BR default) and `robson.rbxsystems.ch` (en default).

## Stack

- SvelteKit 2 + `@sveltejs/adapter-static`
- TypeScript strict
- Custom design tokens from RBX Voltage System (no Tailwind)
- `svelte-i18n` for locale handling
- RBX Identity OIDC for native login; legacy web bearer fallback during migration
- Vitest + Playwright

## Quickstart

```bash
pnpm install
cp .env.example .env.local
# fill in the public RBX OIDC client metadata; never add a client secret
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

The host-side physical-device loop is available through
`scripts/android-device-cycle.sh`. It discovers the SDK from `ANDROID_HOME`,
`ANDROID_SDK_ROOT`, or the ignored Android `local.properties` file and never
prints an ADB serial.

```bash
scripts/android-device-cycle.sh doctor
scripts/android-device-cycle.sh verify
scripts/android-device-cycle.sh build
scripts/android-device-cycle.sh install
scripts/android-device-cycle.sh instrumented-test
scripts/android-device-cycle.sh launch
```

The `install`, `stage-apk`, and `cycle` actions refuse to place a login-capable
APK on a device unless Vite resolves complete public RBX Identity metadata for
Android. Keep real public metadata in an ignored local environment file or
inject it into the build environment. A client secret must never be present.
`build` remains available for repository and UI-shell verification while the
operational Identity contract is pending.

If HyperOS rejects installation through ADB, use the explicit `stage-apk`
action and confirm installation on the device. The script does not change phone
settings or install emulator assets.

## Android MOB-P1

The Android client is a Capacitor delivery target of this SvelteKit app. The
first milestone is intentionally read-only: it signs in through RBX Identity
using Authorization Code + PKCE, keeps its token in memory, hides mutation
controls, and rejects non-GET/HEAD API calls before `fetch`.

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

See ADR-0054, ADR-0055, and
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

- `$design` maps to `src/lib/design`
- `$api` maps to `src/lib/api`
- `$stores` maps to `src/lib/stores`
- `$components` maps to `src/lib/components`
- `$icons` maps to `src/lib/icons`
- `$i18n` maps to `src/lib/i18n`

## Deploy

Production deployment: container in k3s rbx-infra cluster,
image at `ghcr.io/ldamasio/robson-frontend-v2`, ArgoCD-managed.

See `docs/runbooks/frontend-deploy.md` for the full deploy procedure. The GitHub Actions workflow `Frontend Build & Publish` builds the Docker image on push to main (or workflow_dispatch) and pushes to GHCR. ArgoCD reconciles the deployment in rbx-infra. Prerequisites B1–B7 must be satisfied by the operator before the first deployment; see runbook.

## Dashboard Semantics

- The dashboard operations panel renders only live positions that still occupy a slot.
- Terminal positions such as `Closed`, `Error`, and `Canceled` remain available in historical views, but are omitted from the live slot list.
- This keeps the slot grid aligned with the current risk surface: occupied slots on one side, available slots on the other.
