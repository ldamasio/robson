ADR-0032: Frontend UX — Event Log as First-Class Audit Surface

Status: Accepted (amended 2026-04-23 after EP-003 backend contract review)
Date: 2026-04-23
Related: ADR-0007 (Robson is risk assistant), ADR-0021 (opportunity vs technical stop), ADR-0022 (Robson-authored position invariant), ADR-0030 (frontend stack), ADR-0031 (Voltage brand).

Context
- Robson's core value proposition is deterministic, auditable risk execution. The system emits an append-only event log as source of truth for every state transition (plan → validate → execute → fill → open → stop/close). Derived state (PnL, slot utilization, position roll-up) is projection over this log.
- The legacy frontend treated the event log as hidden backend detail. Users saw dashboards and aggregated metrics but could not easily trace how a specific PnL number was produced. This is anti-audit.
- The operator journey spans three natural temporal scales (current month operational use, year review, single-operation forensic audit) and must handle operations that span month boundaries (inherited).
- Kill-switch is a critical safety control that must resist accidental triggering and include mandatory reflection before reversal.

Decision
- **Event log is a first-class UI surface.** Operator sees full event streams by default. No progressive-disclosure collapse. Every event timestamped to the millisecond UTC, mono tabular, addressable by `#event-{seq}` URL anchor.
- **Three temporal scales, navigable, not mutually exclusive**:
  - Current month (dashboard default) — 90% of use.
  - Year view (calendar year primary + rolling 12 toggle) — monthly review.
  - Single operation — forensic drilldown.
- **Routes**:
  - `/` — dashboard, current month
  - `/history` — year view (FE-P2)
  - `/history/YYYY-MM` — month detail (FE-P2)
  - `/operation/{id}` — operation event log
  - `/operation/{id}#event-{n}` — deep-link to specific event
  - `/kill-switch` — dedicated disable/enable page with type-to-confirm
- **Slot visualization**: discrete cells grid (not donut, not bar). Each cell communicates state via glyph + color (accessibility: never color alone). Free `○`, occupied `●` (tinted by PnL / status / cooldown), inherited `◐`.
- **Inherited operation indicator**: text eyebrow `INHERITED FROM MAR` / `CARRIED INTO MAY` (no arrows — violates editorial rules). Op belongs to entry month for slot counting, appears in both month views.
- **Kill-switch pattern**: type-to-confirm (`DESLIGAR` pt-BR / `DISABLE` en) + 5-minute backend-enforced cooldown. Kill-switch prevents *new* positions; existing positions continue to be managed until they close naturally. Re-enable requires same type-to-confirm pattern.
- **Event log hard rules**:
  1. Zero default collapse.
  2. Millisecond UTC timestamps always, never relative time.
  3. Mono tabular numbers.
  4. Every event URL-addressable.
  5. Every derived value links to producing events.
  6. Export JSON + CSV in 1 click (FE-P2).
  7. UI is read-only; all mutations flow through Robson API which writes to event log.
- **Keyboard navigation**: `←/→` for months, `↑/↓` for years, `G D/G H/G O` go-to shortcuts, `?` for help. Command palette `⌘K` and grid overlay `⌘G` deferred to FE-P3.
- **Signature elements applied** (per ADR-0026): L-corners on all cards, voltage hairline connects event timelines, tick rulers on axes and scrubbers, compass mark in headers.
- **MVP (FE-P1) scope**:
  - Dashboard (current month only)
  - Operation detail (event log without hash chain)
  - Kill-switch
  - Text-based inherited indicators
- **Deferrals**:
  - FE-P2: year history, month detail, sparklines, export, rolling-12 toggle.
  - FE-P3: hash chain display, cryptographic receipts, command palette, grid overlay, slot timeline diagram.
  - Command palette `⌘K` deferred — small screen surface at MVP does not benefit from fuzzy search.
  - Export button deferred — single-operator MVP can screenshot / copy mono text.
  - Hash chain UI deferred — backend hash chain is not yet implemented; adding to P1 would block on backend work.

Consequences
- Positive
  - Operator and future auditors can trace any derived number (PnL, position size, slot utilization) back to the producing event sequence by URL.
  - "Instrument, not dashboard" philosophy reinforced by discrete slot cells, mono tabular data, tick ruler signature, and event-log primacy.
  - Type-to-confirm + cooldown eliminates accidental kill-switch triggers and the panic-reversal cycle.
  - Inherited operation handling gives a coherent cross-month narrative without violating editorial rules (no arrows).
  - Single codebase i18n: operator can switch languages mid-session without modal keyword mismatch (handled by close-and-reopen on locale change).
- Negative / Trade-offs
  - Dense information display has a learning curve. Mitigation: MVP user is the author; public documentation arrives with FE-P2 / FE-P3.
  - Event log visible by default means longer operation detail pages. Mitigation: virtualized scroll if performance requires; summary card at top gives high-level outcome.
  - Deferring hash chain display to FE-P3 means MVP audit story is weaker than the full vision. Mitigation: every event has timestamp + type + summary; hash is additive integrity, not core audit information.

Alternatives
- **Rolling 12 months as only option** — rejected. Calendar year aligns with tax / regulatory review; operator thinks in calendar quarters and years.
- **"Only last month"** — rejected. Context too narrow for pattern emergence.
- **Donut / bar chart for slot utilization** — rejected. Discrete cells are instrument-aesthetic, align with Swiss grid, scale cleanly regardless of slot count, and communicate per-slot state rather than only aggregate.
- **Arrow glyphs (`→`, `↓`) for carry indicators** — rejected. Violates editorial rules (no arrows in prose or UI).
- **Modal OK/Cancel for kill-switch** — rejected. Reflex-clicked, offers no safety.
- **Hold-to-confirm for kill-switch** — rejected. Finger-fatigues on desktop, unclear affordance on touch.
- **2FA/TOTP for kill-switch** — rejected for MVP. Adds external dependency; revisit in FE-P4 multi-tenant with operator-configurable security level.
- **Progressive disclosure of event log** — rejected. Anti-audit. Operator and auditor expect full visibility.
- **No cooldown on kill-switch** — rejected. Panic-kill → regret → re-enable → disaster cycle is well-documented in trading behavior.

Implementation Notes
- Routes implemented in `apps/frontend-v2/src/routes/`.
- Domain stores: `src/lib/stores/{auth,slots,operations,events}.ts`.
- API client: `src/lib/api/robson.ts` — typed wrapper over Robson REST endpoints. SSE helper for live event stream.
- Kill-switch flow validated end-to-end against backend endpoints `/kill-switch` (GET status, POST toggle) and event log emission for `KILL_SWITCH_TRIGGERED`, `COOLDOWN_STARTED`, `COOLDOWN_EXPIRED`, `KILL_SWITCH_RE_ENABLED`.
- i18n keyword enforcement: `DESLIGAR`/`RELIGAR` (pt-BR), `DISABLE`/`ENABLE` (en). Backend normalizes both.
- Accessibility: operation event stream traverses as an ordered list for screen readers; modal uses `role="alertdialog"`; countdown uses `aria-live="polite"`.
- Related agnostic docs (source of truth):
  - `~/docs/rbx-frontend-ux-slots-events.md`
  - `~/docs/rbx-frontend-kill-switch.md`
- Implementation guide: `docs/implementation/FE-P1-FRONTEND-MVP.md`

---

## Amendment 1 — 2026-04-23 (backend contract alignment)

During EP-003 the API client was mapped to the actual `robsond` REST surface. Several concepts this ADR named do not exist in the backend under those names, or do not exist at all. This amendment aligns UX vocabulary with backend reality while preserving user-facing language.

### A1.1 Vocabulary mapping (backend ↔ UI)

| Backend (code, types) | UI label (user-facing) | Notes |
|-----------------------|------------------------|-------|
| `Position`            | "Operation"            | UI keeps "Operation" label for operator familiarity; code imports `Position` |
| `PositionState` variants (`Armed`, `Entering`, `Active`, `Exiting`, `Closed`, `Error`) | various chips | Map via presentation layer `src/lib/presentation/labels.ts` |
| `/monthly-halt`       | "Kill Switch"          | Endpoint differs but semantics identical (prevent new entries) |
| `MonthlyHaltStatus`   | "Kill switch status"   | UI labels stay; code imports backend type |
| SSE `/events` (global stream) | "Event log"    | Global stream; per-operation view is client-filtered by `payload.position_id` |
| n/a (not implemented) | "Slots"                | Frontend-derived projection; see A1.2 |
| n/a (not implemented) | "Inherited from"       | Requires historical query; DEFERRED to FE-P2 |

A presentation layer module (`src/lib/presentation/labels.ts`) owns all backend→UI translations. Components import from the presentation layer, never raw backend strings. This keeps the UI vocabulary stable even if backend naming evolves.

### A1.2 Slots are a frontend-derived projection

Robson backend does not model slots. Slots are a UX concept (monthly risk allocation buckets). For FE-P1, slots are computed client-side:

```
SLOT_COUNT = 6                                    # config constant in src/lib/config.ts
occupied   = count of positions whose state is one of Armed | Entering | Active
free       = SLOT_COUNT - occupied
```

Inherited slots (operations carried over from a previous calendar month) require a historical positions query. The backend does not expose one. This is **deferred to FE-P2** alongside the year/month history view.

FE-P1 dashboard renders only current-state cells. No cross-month carryover indicators are shown.

### A1.3 Event log per operation — client-filtered SSE

Backend has no per-position event endpoint. The global SSE `/events` stream emits `SseEvent` records with `payload.position_id` (and other payload fields per event type).

**Operation detail page (`/operation/{id}`) strategy for FE-P1**:
- Subscribe to global SSE stream on mount.
- Client filters events by `payload.position_id === route.params.id`.
- No pre-mount history: only events received after page load are shown.
- Document this limitation visibly: "Events from this session only. Full history in FE-P2 (pending backend `/events?position_id` endpoint)."

If/when backend adds `GET /events?position_id=X` (historical query), FE-P2 pre-loads and merges with live stream.

### A1.4 Kill-switch cooldown — server-side verification required

The 5-minute cooldown principle in this ADR mandates backend enforcement. `MonthlyHaltStatus` exposes `triggered_at`, so the frontend can compute `cooldown_until = triggered_at + 5min`. But if the backend accepts a second toggle within the window without returning 409, the cooldown is only advisory client-side (bypassable via devtools, refresh, curl).

**Action**: during EP-006, GLM-5.1 must verify by test (rapid toggle → expect 409). If backend does not enforce, file a backend ticket and document the gap as a FE-P1 limitation in the dashboard kill-switch entry UX.

### A1.5 Hash chain UI — already deferred, unchanged

`Position` type has no hash field currently. SSE events carry `event_id` but not a hash chain. This aligns with the original FE-P3 deferral. No amendment needed.

### A1.6 Event log hard rules — still apply, scope narrowed for FE-P1

The hard rules (zero default collapse, ms UTC, mono tabular, URL-addressable, 1-click export, UI read-only) remain normative. FE-P1 scope:

- ✅ Zero default collapse — applied.
- ✅ ms UTC — SSE `occurred_at` is ISO8601; render as millisecond UTC in mono.
- ✅ Mono tabular — applied.
- ✅ URL-addressable events — `#event-{seq}` anchors where `seq` derives from event arrival order within the page session (FE-P1) or backend sequence (FE-P2 when history endpoint arrives).
- ⏳ Export 1-click — FE-P2 (deferred; also aligns with history view).
- ✅ UI read-only — applied.

- Pending decisions (tracked in implementation guide):
  - Slot count default (6? configurable per operator?)
  - Operation ID format (UUID, ULID, human-readable `20260412-btc-long-a7c2`)
  - Hash chain algorithm for FE-P3 (SHA-256 straight vs Merkle tree)
  - Time zone display (UTC only vs operator-local with UTC badge)
  - Cooldown configurable with minimum floor
  - Kill-switch scope: prevent new entries only, or also freeze stop-updates on existing positions (implied: only new entries)
