# FE-P1 — Frontend MVP — Analysis & Execution Plan

**Date**: 2026-04-23
**Author**: Claude Opus 4.7 (planner) — execution open to Codex / GLM handoff
**Status**: Draft
**Related**: ADR-0025 (stack), ADR-0026 (brand), ADR-0027 (UX)

---

## Executive Summary

**Problem Statement**: Retire the legacy React `apps/frontend/` and deliver a new static SvelteKit frontend for Robson v3, hosted on Contabo Object Storage and served via PowerDNS on dual domains (`robson.rbx.ia.br` pt-BR, `robson.rbxsystems.ch` en). MVP scope is single operator with GitHub OAuth, dashboard for the current month, operation event log, and kill-switch.

**Key Findings**:
- The SvelteKit + `adapter-static` stack fits Contabo S3 hosting with minimal infrastructure.
- The RBX Voltage design system is fully specified; tokens and SVG assets are ready to copy into the scaffold.
- Robson backend already emits an event log and exposes REST endpoints for operations, slots, and kill-switch. No backend changes required for FE-P1 (hash chain display deferred to FE-P3).
- Two domains are supported by a single codebase with hostname-driven locale selection.

**Recommended Action**: Execute the entry points in order (EP-001 scaffold → EP-002 design system → EP-003 auth → EP-004 dashboard → EP-005 operation → EP-006 kill-switch → EP-007 i18n → EP-008 deploy).

**Estimated Effort**: 5–8 working days for a single experienced Svelte developer. Can be parallelized across 2 agents (e.g. scaffold + design system track vs auth + API client track).

---

## Current State

### System Overview
- Backend: Robson v3 (`v2/`, `v3/`) live in production with real capital (`project_robson_v3_capital_real_state`).
- Legacy frontend: `apps/frontend/` (React + Vite + `react-bootstrap`) — to be removed post-cutover.
- Brand assets: `/home/psyctl/apps/robson/brand-voltage/` — tokens, SVGs, preview HTML (approved).
- Infrastructure: Contabo S3 (`rbx-content` bucket already in use for blog), PowerDNS sovereign 2-VPS.
- Blog frontend: `rbx-robotica-frontend/` (Next.js) — unrelated, marketing only.

### Observed Behavior
- No new frontend exists yet. Scaffold directory `apps/frontend-v2/` is empty.
- Legacy frontend still deployed; operators currently use Robson CLI for all operations.

### Expected Behavior
- `apps/frontend-v2/` contains a production-ready SvelteKit app with Voltage brand applied.
- Operator can log in via GitHub, view current month slots + active operations + today's events, drill into any operation's event log, and trigger kill-switch with type-to-confirm and 5-minute cooldown.
- Two domains resolve to the same static bundle; locale defaults by hostname.

### Root Cause Analysis
N/A — greenfield implementation, no gap to analyze.

---

## Gaps

### Documentation Gaps

| Priority | File/Location | Issue | Impact |
|----------|---------------|-------|--------|
| P1 | `docs/runbooks/frontend-deploy.md` | Does not exist — needed for CI/CD + manual deploy recovery | MED |
| P2 | `apps/frontend-v2/README.md` | Does not exist — needed for agent/human onboarding | LOW |

### Code Gaps

| Priority | Component | Issue | Blocker For |
|----------|-----------|-------|-------------|
| P0 | `apps/frontend-v2/` | Scaffold missing | Everything |
| P0 | `apps/frontend-v2/src/lib/design/tokens.css` | Voltage tokens not yet copied from `brand-voltage/` | All UI work |
| P0 | `apps/frontend-v2/src/lib/api/robson.ts` | API client missing | Auth + dashboard + operations |
| P0 | Backend `/kill-switch` endpoint verification | Need to confirm endpoint contract exists and behaves per ADR-0027 | Kill-switch feature |
| P1 | RBX custom icons (20 glyphs) | SVGs must be produced | Polished UI surfaces |
| P2 | SSE event stream endpoint | Confirm backend supports SSE or fall back to polling | Today's events panel |

### Infrastructure Gaps

| Priority | Resource | Issue | Impact |
|----------|----------|-------|--------|
| P0 | S3 bucket `robson-app` | Does not exist; must be created in Contabo | Deploy blocked |
| P0 | IAM credentials for CI | Need deploy-only S3 key in GitHub Actions secrets | CI blocked |
| P0 | DNS CNAME records | `robson.rbx.ia.br` and `robson.rbxsystems.ch` not configured | Public access blocked |
| P1 | TLS strategy | Contabo domain mismatch for custom domains — decide between accept-redirect or Cloudflare-from-day-1 | Public access quality |
| P2 | GitHub OAuth app | Create OAuth app, store `GITHUB_CLIENT_ID` + `GITHUB_CLIENT_SECRET` | Auth blocked |

---

## Priority Tracks

### Track 1: Scaffold & Design System — bring the app to a running state with Voltage applied
**Effort**: 1 day
**Dependencies**: None
**Deliverables**:
- `apps/frontend-v2/` with SvelteKit + `adapter-static` initialized
- Voltage tokens imported and applied to root layout
- Placeholder landing route renders correctly with L-corners + cyan accents
- `pnpm dev` runs locally

**Tasks**: See EP-001, EP-002.

### Track 2: Auth & API Client — enable authenticated data fetching
**Effort**: 1 day
**Dependencies**: Track 1 complete
**Deliverables**:
- GitHub OAuth flow working end-to-end
- Robson API client module with typed methods
- Auth store with session management
- `/login` and `/dashboard` routes with auth guard

**Tasks**: See EP-003.

### Track 3: Dashboard — current month view
**Effort**: 1.5 days
**Dependencies**: Track 2 complete
**Deliverables**:
- Slots visualizer (discrete cells) with inherited indicator
- Active operations panel
- Today's events mini stream
- Compass mark + wordmark header

**Tasks**: See EP-004.

### Track 4: Operation Detail — event log surface
**Effort**: 1 day
**Dependencies**: Track 2 complete
**Deliverables**:
- `/operation/{id}` route
- Summary card (collapsed high-level)
- Event stream (always expanded, voltage hairline signature)
- Deep-link anchors `#event-{n}`

**Tasks**: See EP-005.

### Track 5: Kill-Switch — type-to-confirm + cooldown UI
**Effort**: 1 day
**Dependencies**: Track 2 complete; backend endpoint verified
**Deliverables**:
- `/kill-switch` route
- Confirmation modal with type-to-confirm keyword
- Countdown timer (mono tabular) during cooldown
- Integration with dashboard status strip

**Tasks**: See EP-006.

### Track 6: i18n — dual-domain locale handling
**Effort**: 0.5 day
**Dependencies**: Tracks 1–5 complete
**Deliverables**:
- `svelte-i18n` (or `paraglide-js`) integrated
- pt-BR + en translation files covering MVP strings
- Hostname detection sets default locale
- Cookie toggle overrides default

**Tasks**: See EP-007.

### Track 7: Deploy — CI/CD + DNS + TLS
**Effort**: 1 day
**Dependencies**: Tracks 1–6 complete; infrastructure gaps resolved
**Deliverables**:
- GitHub Actions workflow builds + syncs to Contabo bucket
- DNS CNAMEs resolve
- TLS strategy decided and implemented

**Tasks**: See EP-008.

---

## Execution Selector

| Objective | Entry Point | Effort |
|-----------|-------------|--------|
| Scaffold SvelteKit app | EP-001 | 2h |
| Apply Voltage design tokens | EP-002 | 4h |
| GitHub OAuth + API client | EP-003 | 1 day |
| Dashboard current month | EP-004 | 1.5 day |
| Operation event log page | EP-005 | 1 day |
| Kill-switch type-to-confirm | EP-006 | 1 day |
| i18n pt-BR + en | EP-007 | 4h |
| CI/CD + DNS + TLS | EP-008 | 1 day |

### Default Execution Order

1. EP-001 (scaffold is a blocker for everything)
2. EP-002 (design tokens needed before any UI)
3. EP-003 (auth is prerequisite for all data-dependent UI)
4. EP-004, EP-005, EP-006 (can run in parallel after EP-003)
5. EP-007 (can start in parallel, completes before deploy)
6. EP-008 (final step)

---

## Entry Points

### EP-001: Scaffold SvelteKit app

**Objective**: Create `apps/frontend-v2/` with SvelteKit + `@sveltejs/adapter-static`, TypeScript strict, Vitest, Playwright. App runs with `pnpm dev`.

**Preconditions**:
```bash
# Node >= 20 installed
node --version | grep -Eq "^v(20|21|22)\."

# pnpm installed
which pnpm

# robson repo checked out at expected path
test -d /home/psyctl/apps/robson

# apps/frontend-v2 does not yet exist (idempotence)
test ! -d /home/psyctl/apps/robson/apps/frontend-v2
```

**Inputs** (explicit):
- `SCAFFOLD_PATH`: `apps/frontend-v2` (relative to repo root)
- `PACKAGE_NAME`: `@robson/frontend-v2`

**Steps**:
```bash
cd /home/psyctl/apps/robson

# Use the skeleton scaffold created by this planner (in RELEASE bundle).
# If running fresh, substitute with `pnpm create svelte@latest apps/frontend-v2`
# and answer: Skeleton project / TypeScript / ESLint / Prettier / Vitest / Playwright.

cd apps/frontend-v2
pnpm install

# Verify build works
pnpm run check
pnpm run build

# Run dev server
pnpm run dev  # should open http://localhost:5173
```

**Expected Outcome**:
```bash
# PASS: app directory exists with expected structure
test -d apps/frontend-v2/src/routes

# PASS: package.json has svelte + adapter-static
grep -q '"@sveltejs/kit"' apps/frontend-v2/package.json
grep -q '"@sveltejs/adapter-static"' apps/frontend-v2/package.json

# PASS: TypeScript check succeeds
cd apps/frontend-v2 && pnpm run check && echo "PASS"

# PASS: Build succeeds
cd apps/frontend-v2 && pnpm run build && test -d build && echo "PASS"
```

**Failure Detection**:
- FAIL if `pnpm install` errors (lockfile corruption, network, version mismatch)
- FAIL if `pnpm run check` reports TS errors
- FAIL if build does not produce `build/index.html`

**Rollback**:
```bash
rm -rf apps/frontend-v2
```

---

### EP-002: Apply Voltage design system

**Objective**: Copy `brand-voltage/colors_and_type.css` into the app as `src/lib/design/tokens.css`, create layout primitives (`Stack`, `Row`, `Grid`, `Bleed`, `Prose`), and render a placeholder landing page demonstrating L-corner signature + cyan accent.

**Preconditions**:
```bash
test -d /home/psyctl/apps/robson/apps/frontend-v2
test -f /home/psyctl/apps/robson/brand-voltage/colors_and_type.css
```

**Steps**:
```bash
cd /home/psyctl/apps/robson/apps/frontend-v2

# Copy design tokens
mkdir -p src/lib/design
cp ../../brand-voltage/colors_and_type.css src/lib/design/tokens.css

# Copy logo + wordmarks
mkdir -p static/brand
cp ../../brand-voltage/marks/rbx-mark-B-refined.svg static/brand/rbx-mark.svg
cp ../../brand-voltage/wordmarks/rbx-wordmark-robson.svg static/brand/wordmark-robson.svg
cp ../../brand-voltage/wordmarks/rbx-wordmark-holding.svg static/brand/wordmark-holding.svg

# Create layout primitives under src/lib/design/components/
# (Stack.svelte, Row.svelte, Grid.svelte, Bleed.svelte, Prose.svelte, LCorners.svelte, TickRuler.svelte)
# See template files in scaffold; edit as needed.

# Import tokens.css in src/routes/+layout.svelte
# Apply .rbx root class so tokens cascade.

pnpm run dev
# Verify http://localhost:5173 renders warm-dark background + cyan accent + L-corners on test card.
```

**Expected Outcome**:
```bash
test -f apps/frontend-v2/src/lib/design/tokens.css
test -f apps/frontend-v2/static/brand/rbx-mark.svg

# Dev server renders without console errors
# Manually verify: page has #07080A background, cyan #22E5E5 accents, L-corners visible on card
```

**Failure Detection**:
- FAIL if tokens.css path imports break in Vite
- FAIL if Svelte components fail to compile

**Rollback**:
```bash
rm -rf apps/frontend-v2/src/lib/design
rm -rf apps/frontend-v2/static/brand
```

---

### EP-003: GitHub OAuth + Robson API client

**Objective**: Integrate Auth.js for GitHub OAuth, implement typed API client, create `/login` and protected routes, persist session via HTTP-only cookie.

**Preconditions**:
```bash
# OAuth app created on GitHub
test -n "$GITHUB_CLIENT_ID"
test -n "$GITHUB_CLIENT_SECRET"

# Robson backend reachable
curl -s https://api.robson.internal/health | jq '.status' | grep -q '"ok"' || echo "adjust endpoint"
```

**Inputs**:
- `GITHUB_CLIENT_ID`: OAuth app client ID
- `GITHUB_CLIENT_SECRET`: OAuth app client secret (never log, never commit)
- `ROBSON_API_BASE`: Robson backend URL (e.g. `https://api.robson.internal`)
- `SESSION_SECRET`: random 32-byte hex for signing session cookie

**Steps**:
```bash
cd apps/frontend-v2

# Install Auth.js
pnpm add @auth/sveltekit @auth/core

# Create src/hooks.server.ts with SvelteKitAuth({ providers: [GitHub(...)] })
# Create src/lib/api/robson.ts (typed fetch wrapper with cookie auth)
# Create src/lib/stores/auth.ts (writable session store)
# Create src/routes/login/+page.svelte with "Login with GitHub" button
# Create src/routes/+layout.server.ts with auth guard (redirect to /login if no session)

# Local .env.local
cat > .env.local <<EOF
AUTH_SECRET=$(openssl rand -hex 32)
AUTH_GITHUB_ID=$GITHUB_CLIENT_ID
AUTH_GITHUB_SECRET=$GITHUB_CLIENT_SECRET
PUBLIC_ROBSON_API_BASE=http://localhost:8080
EOF

pnpm run dev
# Test: /login → GitHub consent → redirect back authenticated → access /dashboard
```

**Expected Outcome**:
```bash
# PASS: login flow completes
# PASS: authenticated session cookie set (HTTP-only, Secure, SameSite=Strict)
# PASS: protected routes redirect to /login when unauthenticated
# PASS: API client attaches session automatically
```

**Failure Detection**:
- FAIL if OAuth callback mismatch (check GitHub app callback URL matches `http://localhost:5173/auth/callback/github`)
- FAIL if session cookie not persisted (check SameSite, Secure in dev vs prod)

**Rollback**:
```bash
rm apps/frontend-v2/src/hooks.server.ts
rm -rf apps/frontend-v2/src/lib/api
rm -rf apps/frontend-v2/src/lib/stores
rm apps/frontend-v2/.env.local
```

---

### EP-004: Dashboard current month

**Objective**: Implement `/dashboard` with compass-mark header, status strip, slots visualizer (discrete cells), active operations panel, today's events mini stream.

**Preconditions**: EP-002 and EP-003 complete.

**Steps**:
```bash
# Create stores: src/lib/stores/slots.ts, operations.ts, events.ts
# Create components:
#   src/lib/components/dashboard/SlotsVisualizer.svelte
#   src/lib/components/dashboard/ActiveOperationsPanel.svelte
#   src/lib/components/dashboard/TodayEventsStream.svelte
#   src/lib/components/dashboard/StatusStrip.svelte
# Create route: src/routes/(authed)/dashboard/+page.svelte + +page.server.ts (load slots + ops + events)
# Apply L-corners signature to cards via LCorners component
# Apply tick ruler signature to events stream via TickRuler component
# Verify inherited operation indicator renders with INHERITED FROM MAR eyebrow
```

**Expected Outcome**:
```bash
# PASS: /dashboard renders 6 slot cells (configurable via SLOT_COUNT constant, MVP default 6)
# PASS: active operations list populates from API
# PASS: today's events stream shows last N events with mono timestamps
# PASS: inherited indicator shows when applicable
```

**Rollback**: remove dashboard route and components.

---

### EP-005: Operation detail event log

**Objective**: Implement `/operation/{id}` with summary card + full event stream. Deep-link anchors `#event-{n}` scroll into view and highlight.

**Preconditions**: EP-002, EP-003 complete.

**Steps**:
```bash
# Create route: src/routes/(authed)/operation/[id]/+page.svelte + +page.server.ts
# Load operation + full event log via API
# Render summary card (collapsed high-level outcome) with L-corners
# Render event stream (always expanded, voltage hairline signature vertical connector)
# Each event: mono ms timestamp, cyan-brand event type, muted summary, hash placeholder
# Anchor each event row via id="event-{seq}"
# Scroll-to-anchor behavior on hash change
```

**Expected Outcome**:
```bash
# PASS: /operation/{id} loads and renders all events chronologically
# PASS: deep-link /operation/{id}#event-5 scrolls event 5 into view
# PASS: mono tabular formatting for all numbers and timestamps
```

---

### EP-006: Kill-switch type-to-confirm + cooldown

**Objective**: Implement `/kill-switch` and inline kill-switch entry on dashboard footer. Modal with type-to-confirm keyword, countdown timer, i18n-aware keywords.

**Preconditions**: EP-002, EP-003 complete. Backend `/kill-switch` endpoint verified.

**Steps**:
```bash
# Create route: src/routes/(authed)/kill-switch/+page.svelte
# Create component: src/lib/components/kill-switch/ConfirmModal.svelte
#   - role="alertdialog"
#   - input field with uppercase mono font
#   - confirm button disabled until input matches keyword exactly
#   - display current state: open positions preview, slot counts
# Cooldown countdown: mono tabular, polling /kill-switch/status every 1s
# i18n keywords: DESLIGAR/RELIGAR (pt-BR), DISABLE/ENABLE (en)
# Handle locale switch during open modal: close + reopen
# Handle network failure: retry 3× exponential backoff
```

**Expected Outcome**:
```bash
# PASS: modal opens with current state + open positions
# PASS: confirm disabled until keyword matches
# PASS: successful toggle → cooldown state with countdown
# PASS: re-enable blocked until cooldown_until timestamp passed (backend-enforced)
# PASS: event log shows KILL_SWITCH_TRIGGERED event with operator + timestamp
```

---

### EP-007: i18n pt-BR + en

**Objective**: Integrate `svelte-i18n` (default choice; `paraglide-js` alternative), create translation files, hostname-driven default.

**Preconditions**: UI strings extracted and replaced with `$_('key')` references.

**Steps**:
```bash
pnpm add svelte-i18n

# Create src/lib/i18n/{pt-BR,en}.json
# Create src/lib/i18n/index.ts with locale detection from hostname:
#   hostname ends with .ia.br → pt-BR
#   hostname ends with .ch → en
# Cookie `locale` overrides if set.
# Register locale loader in src/routes/+layout.ts.
# Replace hardcoded strings with $_('key') calls throughout components.
```

**Expected Outcome**:
```bash
# PASS: rbx.ia.br renders pt-BR default
# PASS: rbxsystems.ch renders en default
# PASS: locale toggle in header switches persistently (cookie)
# PASS: kill-switch keyword localizes correctly
```

---

### EP-008: CI/CD + DNS + TLS

**Objective**: GitHub Actions workflow builds and syncs to Contabo bucket. DNS CNAMEs configured via PowerDNS. TLS strategy implemented.

**Preconditions**:
- S3 bucket `robson-app` created in Contabo
- IAM credentials with PutObject/DeleteObject for that bucket in GitHub Actions secrets (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`)
- OAuth app secrets added as GitHub Actions secrets

**Steps**:
```bash
# Create .github/workflows/frontend-deploy.yml
# Steps: checkout, setup-pnpm, pnpm install, pnpm build, aws s3 sync
#
# Create PowerDNS records (via dns-tofu-env.sh pattern per project_dns_secrets_pattern):
#   robson.rbx.ia.br       CNAME  eu2.contabostorage.com.
#   robson.rbxsystems.ch   CNAME  eu2.contabostorage.com.
#
# TLS decision:
#   Option A (MVP-fast): accept Contabo TLS, accept domain mismatch warning, ship
#   Option B (MVP-polished): Cloudflare free tier in front, custom cert via Cloudflare
# Decision tracked in ADR-0025 pending.
```

**Expected Outcome**:
```bash
# PASS: push to main triggers deploy, bucket updated
# PASS: https://robson.rbx.ia.br loads the app (with chosen TLS strategy)
# PASS: https://robson.rbxsystems.ch loads the app
# PASS: pt-BR default on .ia.br, en default on .ch
```

---

## Verification Commands Reference

**Check if dev server runs**:
```bash
cd apps/frontend-v2 && timeout 10 pnpm run dev &
sleep 5
curl -s http://localhost:5173 | grep -q "RBX" && echo "PASS" || echo "FAIL"
pkill -f "vite dev"
```

**Check if build produces static output**:
```bash
cd apps/frontend-v2 && pnpm run build
test -f build/index.html && echo "PASS" || echo "FAIL"
```

**Check if tokens applied**:
```bash
cd apps/frontend-v2 && pnpm run build
grep -q "#07080A" build/_app/**/*.css && echo "PASS" || echo "FAIL"
```

**Check if auth flow works (manual)**:
```bash
# open http://localhost:5173/login
# click "Login with GitHub"
# verify redirect to GitHub, consent, redirect back to /dashboard
# check cookie: Application tab → session-token HttpOnly=true
```

**Check if kill-switch cooldown enforced**:
```bash
# trigger kill-switch via UI
# immediately attempt re-enable
# verify backend returns 409 with cooldown_until timestamp
```

---

## Rollback Notes

### Rollback Pattern 1: Remove entire new frontend
```bash
rm -rf apps/frontend-v2
git restore .github/workflows/frontend-deploy.yml  # if committed
# Revert DNS changes via opentofu in rbx-infra
```

### Rollback Pattern 2: Revert to legacy apps/frontend/ during transition
```bash
# Keep legacy frontend deployed in parallel
# DNS CNAME change: point robson.* back to legacy host
# apps/frontend/ remains untouched until FE-P1 proves stable
```

---

## Delegation Notes (Codex / GLM handoff)

This guide is self-contained. To delegate to Codex or GLM:

- **Codex**: strong at scaffolding and Svelte component work. Assign Tracks 1–4. Remind Codex that Codex does not run nightly rustfmt (`feedback_rustfmt_pattern`) — this does not apply to Svelte but applies if they touch Rust backend code.
- **GLM**: capable on tooling and infrastructure. Assign Tracks 5–7 plus any backend contract verification.
- Both agents should be briefed on:
  - ADR-0025, ADR-0026, ADR-0027 (read these first)
  - Brand artifacts at `brand-voltage/`
  - Canonical nomenclature: `FE-PN` for frontend phases (do not use bare "Phase N")
  - Editorial rules: no em-dashes, no arrows, no emoji, no pure white backgrounds
  - Security rules: never print secrets, never use bang `!` for interactive prompts

Progress tracking: update the `## Changelog` section below after each entry point is completed. Mark entry points DONE / IN-PROGRESS / BLOCKED.

---

## Appendices

### Appendix A: Backend endpoints needed (FE-P1)

```
GET  /auth/me                           → session user info
POST /auth/logout                       → clear session
GET  /slots?month=YYYY-MM               → slot utilization for month
GET  /operations?status=open            → active operations
GET  /operations/{id}                   → operation detail
GET  /operations/{id}/events            → full event log for operation
GET  /events?from=YYYY-MM-DD&to=...     → events in time range (dashboard today)
GET  /kill-switch/status                → current state + cooldown_until
POST /kill-switch                       → toggle with operator_id + reason
```

Verify each exists; file tickets for any gaps before starting related entry points.

### Appendix B: File tree (target state after FE-P1)

```
apps/frontend-v2/
├── .env.example
├── .gitignore
├── README.md
├── package.json
├── pnpm-lock.yaml
├── svelte.config.js
├── tsconfig.json
├── vite.config.ts
├── playwright.config.ts
├── vitest.config.ts
├── src/
│   ├── app.d.ts
│   ├── app.html
│   ├── hooks.server.ts
│   ├── lib/
│   │   ├── api/
│   │   │   └── robson.ts
│   │   ├── components/
│   │   │   ├── dashboard/
│   │   │   ├── kill-switch/
│   │   │   ├── operation/
│   │   │   └── shared/
│   │   ├── design/
│   │   │   ├── components/
│   │   │   │   ├── Stack.svelte
│   │   │   │   ├── Row.svelte
│   │   │   │   ├── Grid.svelte
│   │   │   │   ├── Bleed.svelte
│   │   │   │   ├── Prose.svelte
│   │   │   │   ├── LCorners.svelte
│   │   │   │   └── TickRuler.svelte
│   │   │   └── tokens.css
│   │   ├── i18n/
│   │   │   ├── index.ts
│   │   │   ├── pt-BR.json
│   │   │   └── en.json
│   │   ├── icons/
│   │   │   ├── lucide/
│   │   │   └── rbx/
│   │   └── stores/
│   │       ├── auth.ts
│   │       ├── events.ts
│   │       ├── operations.ts
│   │       └── slots.ts
│   └── routes/
│       ├── +layout.svelte
│       ├── +layout.server.ts
│       ├── +page.svelte                    # / landing
│       ├── login/
│       │   └── +page.svelte
│       └── (authed)/
│           ├── +layout.server.ts           # auth guard
│           ├── dashboard/
│           │   └── +page.svelte
│           ├── operation/
│           │   └── [id]/
│           │       └── +page.svelte
│           └── kill-switch/
│               └── +page.svelte
├── static/
│   ├── brand/
│   │   ├── rbx-mark.svg
│   │   ├── wordmark-holding.svg
│   │   └── wordmark-robson.svg
│   └── favicon.svg
└── tests/
    ├── e2e/
    │   ├── auth.spec.ts
    │   ├── dashboard.spec.ts
    │   └── kill-switch.spec.ts
    └── unit/
        └── i18n.test.ts
```

### Appendix C: Decision log

- **SvelteKit over Angular** — ADR-0025 alternatives section
- **No Tailwind** — ADR-0025 alternatives section
- **Defer hash chain to FE-P3** — backend not ready; additive integrity, not core audit info
- **Defer export to FE-P2** — MVP operator can screenshot; polish bundled with history
- **Defer command palette to FE-P3** — 6-screen surface doesn't need fuzzy search
- **Single-accent cyan** — ADR-0026; rejected per-product sub-accents
- **Type-to-confirm kill-switch** — ADR-0027; rejected modal OK/Cancel

---

## Changelog

| Date | Change | Author | Status |
|------|--------|--------|--------|
| 2026-04-23 | Initial draft | Claude Opus 4.7 | Draft |
| 2026-04-23 | EP-001 scaffold — package.json, svelte.config, tsconfig, +layout, +page, app.html, /login, /(authed) routes stubs | Claude Opus 4.7 | DONE (pending `pnpm install` + `pnpm dev` verification by operator) |
| 2026-04-23 | EP-002 design system — tokens.css copied, brand assets copied, layout primitives (Stack/Row/Grid/Prose/Card/LCorners/TickRuler) written, +page demo renders signature elements | Claude Opus 4.7 | DONE (pending visual verification) |
| — | EP-003 auth + api | — | TODO (stub API client in `src/lib/api/robson.ts`, stub auth guard in `+layout.server.ts`) |
| 2026-04-23 | EP-003 — typed API client mapped to real robsond endpoints, Bearer token auth (OAuth deferred), client-side auth guard, login page, Svelte 4-to-5 migration | GLM-5.1 | DONE (see Blocker Findings below for pending items) |
| 2026-04-23 | EP-004 dashboard — real API wiring (getStatus + getHaltStatus + SSE), slots derivation from positions (SLOT_COUNT=6), presentation labels layer, error states, retry, e2e with mocked API, vitest slot derivation tests | GLM-5.1 | DONE |

### EP-003 Blocker Findings (2026-04-23, GLM-5.1)

**Architectural conflict**: `adapter-static` + `hooks.server.ts` (Auth.js) incompatible. Static S3 hosting has no server runtime. Auth.js `SvelteKitAuth` requires server-side hooks. Resolution: implemented client-side token-based auth instead. GitHub OAuth deferred until backend supports it or edge function added.

**Backend endpoint contract mismatch** (Appendix A vs actual robsond API):

| Frontend expects (Appendix A) | Backend actually has | Status |
|---|---|---|
| `GET /auth/me` | N/A | MISSING — no auth concept in backend |
| `POST /auth/logout` | N/A | MISSING |
| `GET /slots?month=YYYY-MM` | N/A | MISSING — no slots concept in backend |
| `GET /operations?status=open` | `GET /status` (returns positions + pending approvals) | DIFFERENT NAME + SHAPE |
| `GET /operations/{id}` | `GET /positions/{id}` | DIFFERENT NAME |
| `GET /operations/{id}/events` | N/A | MISSING — only SSE stream for all events |
| `GET /events?from=...&to=...` | `GET /events` (SSE stream only, no query params) | DIFFERENT PROTOCOL |
| `GET /kill-switch/status` | `GET /monthly-halt` | DIFFERENT NAME |
| `POST /kill-switch` | `POST /monthly-halt` | DIFFERENT NAME |

**GitHub OAuth app**: Does not exist. No `AUTH_GITHUB_ID` / `AUTH_GITHUB_SECRET` available.

**What was completed despite blockers**:
- Typed API client (`src/lib/api/robson.ts`) mapped to real robsond endpoints: `/status`, `/positions/{id}`, `/monthly-halt`, `/panic`, `/safety/status`, SSE `/events`
- Token-based auth store (`src/lib/stores/auth.ts`) with sessionStorage persistence
- Client-side auth guard (`src/routes/(authed)/+layout.ts`) — redirects to `/login` without token
- Login page with Bearer token input + backend health validation
- Svelte 4 → 5 migration across all components (props, events, slots)
- `pnpm check`: 0 errors, 4 benign warnings
- `pnpm build`: succeeds
- `pnpm test`: 1/1 passing

**What is needed to unblock EP-003 completion (GitHub OAuth)**:
1. Backend must add OAuth endpoint (or frontend needs edge function for OAuth callback)
2. GitHub OAuth app must be created, secrets stored in `.env.local`
3. Decision needed: Auth.js vs custom session middleware (ADR-0025 pending item)
4. Decision needed: `adapter-static` + OAuth requires server-side component — either switch to `adapter-node`, add edge function, or accept token-based auth for MVP

**Decision needed before EP-004**: Backend response shapes for `/status` are known and typed. Dashboard can proceed using `robsonApi.getStatus()` for positions. "Slots" concept does not exist in backend — dashboard EP-004 must adapt to what `/status` returns.
| — | EP-005 operation detail | — | TODO |
| — | EP-006 kill-switch | — | TODO |
| — | EP-007 i18n | — | TODO |
| — | EP-008 deploy | — | TODO |
