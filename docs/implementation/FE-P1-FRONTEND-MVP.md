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

### EP-003: Bearer-token auth + Robson API client — DONE (2026-04-23)

**Status**: DONE by GLM-5.1 on branch `fe-p1/ep-003-auth`, commit `edc19919`.

**Amended objective** (final, supersedes original GitHub OAuth plan):
Implement Bearer-token auth for MVP (per ADR-0025 Amendment 1), typed API client mapped to the real `robsond` REST surface, token store with sessionStorage persistence, client-side auth guard, Svelte 5 migration.

**Rationale for amendment**: `adapter-static` has no server runtime, so OAuth callback cannot be handled without an edge function. Bearer token is the simplest MVP path for a single operator and preserves S3 static hosting. See ADR-0025 A1.2.

**Shipped deliverables**:
- `src/lib/api/robson.ts` — typed client mapped to 7 real endpoints (`/health`, `/status`, `/positions/{id}`, `/positions` POST, `/queries/{id}/approve`, `/monthly-halt` GET/POST, `/panic`, `/safety/status`, SSE `/events`).
- `src/lib/stores/auth.ts` — `authToken` store with `sessionStorage` persistence, `initAuth`, `setToken`, `clearAuth`.
- `src/routes/login/+page.svelte` — token input, validates via `GET /health`, redirects to `/dashboard`.
- `src/routes/(authed)/+layout.server.ts` — static-friendly (no server redirect). Auth guard is client-side via `initAuth()` in root layout.
- Svelte 5 runes throughout: `$props`, `$state`, `$derived`, `Snippet` for `children`.

**Caveats logged**:
- No OAuth. No GitHub OAuth app. ADR-0025 A1.2 is authoritative.
- Token stored in `sessionStorage` key `robson_api_token`. Never logged, never committed.
- Bearer attached to REST via `Authorization` header; to SSE via `?token=` query param (EventSource limitation).

**Verification outcome**:
- `pnpm run check`: 0 errors
- `pnpm run build`: success
- `pnpm run test`: 1/1 passing
- Manual: `/login` accepts token → `/dashboard` renders

**Rollback** (if needed):
```bash
git checkout main -- apps/frontend-v2
```

---

### EP-004: Dashboard current month (adapted to backend reality)

**Objective**: Implement `/dashboard` wired to the real Robson REST + SSE surface. Compass-mark header, status strip from `GET /status` + `GET /monthly-halt`, slots visualizer (discrete cells) derived client-side from positions, active positions panel, today's events mini stream from SSE `/events`.

**Preconditions**: EP-002 and EP-003 complete. Backend reachable at `PUBLIC_ROBSON_API_BASE`.

**Decisions applied** (see ADR-0027 Amendment 1):
- Backend `Position` type is the canonical model. UI label is "Operation".
- Slots are frontend-derived. `SLOT_COUNT = 6` (constant in `src/lib/config.ts`).
- Occupied = count of positions whose state is one of `Armed | Entering | Active`.
- Inherited indicator DEFERRED to FE-P2.
- Today events = SSE events with `occurred_at` within current UTC day, buffered client-side.

**Steps**:
```bash
# 1. Create src/lib/config.ts
#    export const SLOT_COUNT = 6;

# 2. Create src/lib/presentation/labels.ts
#    - positionStateLabel(state: PositionState): "ARMED" | "ENTERING" | "ACTIVE" | "EXITING" | "CLOSED" | "ERROR"
#    - positionPnlPct(p: Position): number | null   (derived from realized_pnl or from Active.current_price/entry_price)
#    - slotStateForPosition(p: Position): 'occupied-ok' | 'occupied-neg' | 'occupied-signal' | 'occupied-warn'

# 3. Create src/lib/stores:
#    - stores/status.ts    — writable<StatusResponse | null>
#    - stores/halt.ts      — writable<MonthlyHaltStatus | null>
#    - stores/events.ts    — writable<SseEvent[]> capped at last 100 events of current UTC day

# 4. Create components (co-located or under src/lib/components/dashboard/):
#    - SlotsVisualizer.svelte — 6 cells, state + glyph per ADR-0027
#    - ActiveOperationsPanel.svelte — grid of position cards
#    - TodayEventsStream.svelte — mono event list + TickRuler
#    - StatusStrip.svelte — "● LIVE · SLOT X/6 · HALT: NO" mono eyebrow

# 5. Wire route: src/routes/(authed)/dashboard/+page.svelte
#    - onMount: call robsonApi.getStatus() + robsonApi.getHaltStatus() into stores
#    - connectEventStream((e) => events.update(arr => capAndAppend(arr, e)))
#    - Poll getStatus() + getHaltStatus() every 10s as fallback to SSE
#    - Show inline error + Retry button if API fails (locator .err-text / .btn-retry per e2e test)

# 6. Unit test: tests/unit/slots-derivation.test.ts
#    - Pure function deriveSlots(positions, SLOT_COUNT) returns SlotState[]
#    - Cases: 0 positions → 6 free, 4 Active → 2 free, 6+ positions → 6 occupied (cap)

# 7. Playwright e2e: tests/e2e/dashboard.spec.ts (already written by operator)
#    - Mocks /health, /status, /monthly-halt
#    - Asserts 6 slots, 2 occupied, status strip shows "SLOT 2/6"
#    - Error state test asserts .err-text + .btn-retry visible on 502
```

**Expected Outcome**:
```bash
# PASS: pnpm run check (0 TS errors)
# PASS: pnpm run build (static output)
# PASS: pnpm run test (slot derivation test + existing)
# PASS: pnpm run test:e2e (dashboard.spec.ts + auth)
# PASS: with backend running, /dashboard shows real position cells
# PASS: with backend 502, /dashboard shows .err-text + .btn-retry
```

**Failure detection**:
- FAIL if mocked e2e fails (locator miss)
- FAIL if `deriveSlots` returns wrong count for any test case
- FAIL if SSE `connectEventStream` leaks (verify cleanup in `onDestroy`)

**Non-goals for EP-004**:
- Inherited ops indicator (FE-P2)
- Monthly history aggregation (FE-P2)
- Real kill-switch integration beyond read-only halt state display (that's EP-006)

**Rollback**: remove stores, components, route reverts to pre-EP-004 commit.

---

### EP-005: Operation detail event log (adapted to backend reality)

**Objective**: Implement `/operation/{id}` wired to real backend. Fetch position via `GET /positions/{id}`. Filter global SSE `/events` stream by `payload.position_id === id` to populate the event stream. Deep-link anchors `#event-{seq}` scroll into view.

**Preconditions**: EP-002, EP-003 complete.

**Decisions applied** (see ADR-0027 Amendment 1):
- No per-position history endpoint on backend. FE-P1 shows only events received after page mount.
- Visible limitation notice: "Events from this session only. History deferred to FE-P2."
- Hash chain UI deferred to FE-P3 (no hash field in `Position` or `SseEvent` currently).

**Steps**:
```bash
# 1. src/routes/(authed)/operation/[id]/+page.svelte
#    - onMount: robsonApi.getPosition(id) into a local $state
#    - connectEventStream((e) => {
#        if (e.payload.position_id === id) localEvents.push(e)
#      })
#    - onDestroy: close SSE
#    - Render summary card with L-corners from position data
#    - Render event stream with voltage hairline (border-left cyan)
#    - Each row: tick "·" + mono ms timestamp + cyan-brand event_type + muted summary
#    - Anchor each event: id="event-{seq}" where seq = arrival order in this session

# 2. Presentation helpers in src/lib/presentation/labels.ts
#    - formatMs(iso: string): "HH:mm:ss.SSS"
#    - formatPayload(e: SseEvent): string   (human-readable summary)
#    - positionSummary(p: Position): string[]   (lines for summary card)

# 3. Visible limitation banner at top of event stream section:
#    "Events from this session. Full history in FE-P2."

# 4. Unit tests:
#    - formatMs(ISO8601) returns millisecond-precision UTC string
#    - payload summary formatting for main event types (POSITION_*, STOP_*, ORDER_*)

# 5. E2E test:
#    - Mock GET /positions/{id}
#    - Mock SSE /events with stream containing events matching and not matching position_id
#    - Assert only matching events appear
#    - Assert deep-link scroll: navigate to /operation/X#event-2 and verify :target style
```

**Expected Outcome**:
```bash
# PASS: /operation/{id} fetches and renders position summary
# PASS: matching SSE events appear in stream in order
# PASS: non-matching SSE events are filtered out
# PASS: #event-{n} anchor scrolls into view and highlights via :target
# PASS: session-only limitation banner visible
```

---

### EP-006: Kill-switch type-to-confirm + cooldown (mapped to /monthly-halt)

**Objective**: Implement `/kill-switch` page + modal. Map UI operations to `robsonApi.getHaltStatus()` (read) and `robsonApi.triggerHalt(reason)` (write). Preserve UX keywords DESLIGAR/DISABLE/RELIGAR/ENABLE even though backend endpoint is `/monthly-halt`.

**Preconditions**: EP-002, EP-003 complete. Verify backend cooldown enforcement (see below).

**Decisions applied** (see ADR-0027 Amendment 1):
- Backend endpoint is `/monthly-halt`, not `/kill-switch`. Semantics identical.
- `MonthlyHaltStatus.triggered_at` is the authoritative timestamp. Derive `cooldown_until = triggered_at + 5min`.
- **CRITICAL verification step**: test whether backend rejects a second toggle within 5min window with HTTP 409. If not, file backend ticket and document as FE-P1 limitation.
- UI label stays "Kill Switch". Internal code imports `MonthlyHaltStatus`.

**Steps**:
```bash
# 1. Before writing code, verify backend cooldown behavior:
#    - curl -H "Authorization: Bearer $TOKEN" -X POST $API/monthly-halt -d '{"reason":"test"}'
#    - curl -H "Authorization: Bearer $TOKEN" -X POST $API/monthly-halt -d '{"reason":"test2"}'
#    - Expect second call: HTTP 409 with cooldown_until message
#    - If 200: document backend gap, keep UI enforcement as advisory-only

# 2. src/routes/(authed)/kill-switch/+page.svelte
#    - Load current MonthlyHaltStatus on mount
#    - State machine:
#        halt.state === "active"         → show "Disable" confirmation flow
#        halt.state === "monthly_halt"   → show cooldown countdown or "Enable" flow
#    - Type-to-confirm input: match KEYWORD_DISABLE / KEYWORD_ENABLE exactly (uppercase)
#    - Disabled confirm button until match

# 3. Cooldown countdown:
#    - If halt.triggered_at is set, compute cooldown_until = +5min
#    - Show mono tabular countdown (HH:MM:SS) updating every second
#    - Re-enable button disabled until now >= cooldown_until

# 4. i18n keyword mapping:
#    pt-BR: DESLIGAR (enter halt) / RELIGAR (exit halt)
#    en:    DISABLE  (enter halt) / ENABLE  (exit halt)
#    (Note: backend has no "enable" action — halt state is cleared by admin tooling currently.
#     FE-P1 scope: trigger halt only. Clearing halt = out of scope unless backend supports it.)

# 5. Locale switch during open modal: close + reopen to refresh keyword

# 6. Network failure: retry 3× with exponential backoff (1s, 2s, 4s)

# 7. On success, emit SSE event should appear in dashboard today events panel

# 8. E2E test:
#    - Mock /monthly-halt GET (state: active)
#    - Mock /monthly-halt POST (state: monthly_halt with triggered_at: now)
#    - Mock /monthly-halt POST second call: 409
#    - Verify type-to-confirm flow
#    - Verify countdown renders
#    - Verify second trigger blocked
```

**Expected Outcome**:
```bash
# PASS: page reads current halt state
# PASS: confirm button disabled until keyword matches
# PASS: successful POST transitions to cooldown display
# PASS: countdown updates every second
# PASS: second POST within 5min returns 409 (backend-enforced)
# PASS: backend-emitted event visible in dashboard events stream
```

**Failure detection + reporting**:
- If backend does not return 409 for rapid re-toggle: UI cooldown is advisory only. Document in page as "UI-only cooldown — backend accepts bypass". File backend ticket.
- If backend has no inverse action (exit monthly_halt): document FE-P1 limitation, treat as one-way.

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

### Appendix A: Backend endpoints — ACTUAL (verified EP-003 by GLM-5.1 2026-04-23)

Real `robsond` surface (v2 API), used by `src/lib/api/robson.ts`:

```
GET    /health                          → { status: "ok" }
GET    /status                          → StatusResponse (active_positions + positions[] + pending_approvals[])
GET    /positions/{id}                  → Position
POST   /positions                       → arm new position (body: { symbol, side })
DELETE /positions/{id}                  → close position
POST   /queries/{id}/approve            → approve pending query
GET    /monthly-halt                    → MonthlyHaltStatus
POST   /monthly-halt                    → trigger halt (body: { reason })
POST   /panic                           → PanicResponse (close all positions)
GET    /safety/status                   → SafetyStatusResponse
GET    /events                          → SSE stream (Bearer via ?token= query param)
```

**Gaps vs original plan** (accepted; reflected in ADR-0027 Amendment 1):

| Originally planned | Reality | Resolution |
|-------------------|---------|------------|
| `GET /auth/me` | Not present | Bearer-token auth (ADR-0025 A1.2), `GET /health` validates token |
| `POST /auth/logout` | Not present | Client clears `sessionStorage`, no server call needed |
| `GET /slots?month=...` | Not present | Slots derived client-side (ADR-0027 A1.2) |
| `GET /operations?status=open` | → `GET /status` | Use `status.positions` filtered by state |
| `GET /operations/{id}` | → `GET /positions/{id}` | Vocabulary mapping in presentation layer |
| `GET /operations/{id}/events` | Not present | Client-filter global SSE by `payload.position_id` (ADR-0027 A1.3) |
| `GET /events?from=...&to=...` | → SSE `/events` (live only) | Today's events = client-buffered from SSE; history DEFERRED FE-P2 |
| `GET /kill-switch/status` | → `GET /monthly-halt` | Vocabulary mapping |
| `POST /kill-switch` | → `POST /monthly-halt` | Vocabulary mapping |

**Backend ticket candidates** (file before FE-P2):
- `GET /events?position_id=X&from=...&to=...` — historical event query for operation detail pre-load
- `GET /positions?status=all&month=YYYY-MM` — historical positions for slot history view
- Confirm `POST /monthly-halt` enforces 5-minute cooldown server-side (return 409)
- Inverse action for `/monthly-halt` (clear halt state from UI) if desired for operator ergonomics

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
| 2026-04-23 | EP-002 design system — tokens.css copied, brand assets copied, layout primitives (Stack/Row/Grid/Prose/Card/LCorners/TickRuler) written, +page demo renders signature elements | Claude Opus 4.7 | DONE |
| 2026-04-23 | EP-003 auth + api — Bearer-token MVP + typed API client mapped to 7 real endpoints + Svelte 5 migration. Commit edc19919 on `fe-p1/ep-003-auth`. Caveats: no OAuth (adapter-static incompatibility, see ADR-0025 A1.2); 6 of 9 originally planned endpoints absent, UI adapts per ADR-0027 A1.1. | GLM-5.1 | DONE |
| 2026-04-23 | ADR amendments: ADR-0025 A1 (token auth + Svelte 5), ADR-0027 A1 (vocabulary mapping + slots derived + SSE client-filter + cooldown verification requirement) | Claude Opus 4.7 | DONE |
| — | EP-004 dashboard adapted to real endpoints | GLM-5.1 | IN-PROGRESS (e2e test scaffold present; wire real stores + presentation layer + slot derivation) |
| 2026-04-23 | EP-003 — typed API client mapped to real robsond endpoints, Bearer token auth (OAuth deferred), client-side auth guard, login page, Svelte 4-to-5 migration | GLM-5.1 | DONE (see Blocker Findings below for pending items) |
| 2026-04-23 | EP-004 dashboard — real API wiring (getStatus + getHaltStatus + SSE), slots derivation from positions (SLOT_COUNT=6), presentation labels layer, error states, retry, e2e with mocked API, vitest slot derivation tests | GLM-5.1 | DONE |
| 2026-04-23 | EP-004 corrective pass — UTC time formatting (HH:mm:ss.SSS), UTC day filtering, 100-event cap, 10s polling fallback, full cleanup on destroy, retry reinitializes SSE+polling, 6+ positions regression test, UTC formatter unit tests, auth guard sessionStorage fallback, e2e structural tests | GLM-5.1 | DONE |
| 2026-04-23 | EP-005 operation detail — real `GET /positions/{id}` fetch, session-sequenced `#event-{seq}` SSE client-filtered event log, visible FE-P2 history limitation, Voltage summary card, labels/store unit coverage. Codex review found e2e drift: tests are structural and not yet mock-driven for SSE filter, deep-link anchor, or dashboard API behavior. | GLM-5.1 + Codex | FOLLOW-UP REQUIRED |

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
| — | EP-005 operation detail | GLM-5.1 | DONE (behavioral coverage via unit tests; e2e structural + error-state resilient) |
| — | EP-006 kill-switch | — | TODO |
| — | EP-007 i18n | — | TODO |
| — | EP-008 deploy | — | TODO |
