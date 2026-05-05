# ADR-0034: Frontend Slot Count — API Only (Option 2)

**Date**: 2026-04-27
**Status**: DECIDED — implemented with explicit slot semantics

## Context

`apps/frontend/src/lib/config/slots.ts` hardcoded the monthly slot budget. MIG-v3#12 delivered authoritative monthly risk state with dynamic new-entry capacity calculated by `TradingPolicy::slots_available()`. The frontend must not infer total rendered slots from that value because carried positions remain occupied across a month boundary. Two options were evaluated:
- **Option 1**: Full Risk Dashboard (budget bar, realized-loss display, slot breakdown panel).
- **Option 2**: Expose explicit slot fields in `/status`; replace hardcoded constants; defer dashboard.

## Decision

Option 2 — API-only slot exposure. No new UI panels, no budget bar, no realized-loss display.

**Chose**: Backend exposes explicit fields in `StatusResponse`:
- `new_slots_available`: slots available for new entries under the monthly risk budget.
- `occupied_slots`: open core positions already occupying slots.
- `slot_cells_total`: total cells the UI should render, equal to occupied slots plus newly available slots.

The frontend reads these fields via `/status` and does not compute slot capacity policy.
**Rejected**: Option 1 (full dashboard — premature UI work for a correctness fix); Option 3 (no change — silently diverges from backend dynamic calculation).

## Rationale

- The immediate problem is a correctness mismatch after month boundaries: carried positions remain occupied, while the new month can grant fresh entry capacity. Fixing it requires explicit API semantics, not a UI feature.
- A full Risk Dashboard (budget bar, realized-loss breakdown) is valuable but is a separate feature with its own UX design, testing, and iteration cycle. Bundling it with the correctness fix inflates scope and risk.
- The backend already computes new-entry capacity correctly from persisted monthly state. Exposing both capacity and rendered total keeps business policy out of the frontend.

## Deferred to MIG-v3#14

Full Risk Dashboard (Option 1 features). Tentatively a dedicated dashboard story; no implementation date set.

## Breaks if wrong

If the backend returns stale or incorrect slot fields (e.g., after a month boundary reset race), the operator sees a wrong slot count. Mitigation: the backend reads new-entry capacity from the persisted `monthly_state` projection (MIG-v3#12), not from an in-memory cache.

## Reversibility

Breaking API change. Reversal requires restoring the old `slots_available` field and frontend fallback behavior. The explicit contract is preferred because fallback-to-4 can mask production risk-state failures.

## Related

- ADR-0024 — Trading Policy Layer (dynamic slot calculation)
- `docs/architecture/v3-migration-plan.md` — MIG-v3#12 follow-up subsection
