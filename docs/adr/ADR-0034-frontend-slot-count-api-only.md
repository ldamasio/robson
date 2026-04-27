# ADR-0034: Frontend Slot Count — API Only (Option 2)

**Date**: 2026-04-27
**Status**: DECIDED — not yet implemented (MIG-v3#12 follow-up)

## Context

`apps/frontend/src/lib/config/slots.ts` hardcodes `INITIAL_MONTHLY_SLOT_BUDGET = 4`. MIG-v3#12 delivered authoritative monthly risk state with dynamic `slots_available` calculated by `TradingPolicy::slots_available()`. The frontend never reads it. Two options were evaluated:
- **Option 1**: Full Risk Dashboard (budget bar, realized-loss display, slot breakdown panel).
- **Option 2**: Expose `slots_available` in `/status` API response only; replace hardcoded constant; defer dashboard.

## Decision

Option 2 — API-only slot exposure. No new UI panels, no budget bar, no realized-loss display.

**Chose**: Backend exposes `slots_available: u32` in `StatusResponse`; frontend reads it via `/status` and replaces `INITIAL_MONTHLY_SLOT_BUDGET`.
**Rejected**: Option 1 (full dashboard — premature UI work for a correctness fix); Option 3 (no change — silently diverges from backend dynamic calculation).

## Rationale

- The immediate problem is a correctness mismatch (hardcoded 4 vs. dynamic calculation). Fixing it requires one API field and one frontend refactor — not a UI feature.
- A full Risk Dashboard (budget bar, realized-loss breakdown) is valuable but is a separate feature with its own UX design, testing, and iteration cycle. Bundling it with the correctness fix inflates scope and risk.
- The backend already computes `slots_available` correctly in `robson-engine::MonthlyRiskState`. Exposing it is a thin change in `status_handler`.

## Deferred to MIG-v3#14

Full Risk Dashboard (Option 1 features). Tentatively a dedicated dashboard story; no implementation date set.

## Breaks if wrong

If the frontend relies on the API value and the backend returns a stale or incorrect `slots_available` (e.g., after a month boundary reset race), the operator sees a wrong slot count. Mitigation: the backend reads from the persisted `monthly_state` projection (MIG-v3#12), not from an in-memory cache.

## Reversibility

Fully reversible. Removing `slots_available` from `StatusResponse` causes the frontend to fall back to the default `4` in `normalizeStatus`.

## Related

- ADR-0024 — Trading Policy Layer (dynamic slot calculation)
- `docs/architecture/v3-migration-plan.md` — MIG-v3#12 follow-up subsection
