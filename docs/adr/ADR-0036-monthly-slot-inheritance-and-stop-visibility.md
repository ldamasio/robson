# ADR-0036: Monthly Slot Inheritance and Stop Visibility

**Date**: 2026-05-05
**Status**: DECIDED
**Deciders**: RBX Systems (operator + architecture)

---

## Context

Robson v3 now exposes monthly slot state and current position summaries in production.
During operational review, several product rules were clarified:

1. A slot is not a one-month snapshot. Once a slot becomes active, it must remain
   visible in every later month while it is still alive.
2. A slot that was armed but never received an entry signal still counts as occupied
   for future months. It is inherited forward pessimistically because capital was
   committed to the possibility of that trade.
3. A slot opened in one month and closed in a later month must appear in every month
   in which it was active.
4. The UI must distinguish a slot opened in the current month from a slot inherited
   from a previous month.
5. For active positions, the frontend must be able to show both the original stop and
   the current trailing stop, with a visual distinction when they differ.

The system currently exposes live state well, but month navigation and inherited slot
semantics are not yet explicit enough as product contract. This ADR fixes the contract.

---

## Decision

Robson adopts the following monthly slot semantics:

### 1. Monthly inheritance

A slot is visible in every month from its activation until its terminal deactivation.

- If a slot is armed in month `M` and remains alive in `M+1`, it appears in both months.
- If the slot survives across multiple months, it appears in all of them.
- If the slot is armed but never executes an entry, it still inherits into later months
  and continues to count pessimistically against future capital calculations.
- Terminal removal from the monthly view happens only when the slot reaches a terminal
  state (`Closed`, `Cancelled`, or equivalent terminal lifecycle state).

### 2. Month navigation

The UI must support navigation across historical months in which Robson was running.
Each month view should render the set of slots that were alive during that month, not
only slots created during that month.

### 3. Visual differentiation

The UI must distinguish between:

- a slot opened in the current month
- a slot inherited from a previous month
- a slot armed but not yet executed
- a slot that has been deactivated or closed

This distinction is purely presentational. It must not alter slot ownership or risk
accounting.

### 4. Stop visibility

For live active positions, the UI should expose:

- original stop at activation
- current trailing stop
- current stop as the authoritative live stop for execution and risk

If the current stop differs from the original stop, the UI should make that difference
visible. The backend remains the source of truth for the current stop.

---

## Consequences

### Positive

- Month views become operationally correct instead of being limited to creation-time
  snapshots.
- Pessimistic capital accounting remains intact for armed-but-unfilled slots.
- Operators can reason about inheritance without inferring it from raw timestamps.
- Stop evolution becomes visible, which reduces ambiguity in active-position review.

### Negative / Trade-offs

- The product now needs a clearer historical slot model, not only a current-state view.
- The implementation will likely need an explicit monthly projection or history query.
- The UI becomes slightly denser because it must explain inherited versus newly opened
  slots.

---

## Alternatives Considered

### Snapshot by creation month only

Rejected. This hides live slots in later months and breaks the inherited-capital rule.

### Snapshot by closing month only

Rejected. This also loses the fact that a position occupied capital in previous months
while it was still active.

### Show only current month

Rejected. This would erase the operating history that users explicitly need to inspect.

### Hide original stop and show only current stop

Rejected. It loses the distinction between thesis origin and trailing evolution.

---

## Implementation Notes

- Backend/API should remain the source of truth for current active stop values.
- Frontend should not infer month inheritance from rendered slot order alone.
- Historical month rendering should be driven by a backend contract or projection that
  can answer: "which slots were alive during month M?"
- Related code paths already involved:
  - `v3/robsond/src/api.rs`
  - `apps/frontend/src/lib/config/slots.ts`
  - `apps/frontend/src/routes/(authed)/dashboard/+page.svelte`
  - `v3/robson-store/src/memory.rs`
  - `v3/robson-store/src/postgres.rs`

---

## References

- [ADR-0034: Frontend Slot Count - API Only](ADR-0034-frontend-slot-count-api-only.md)
- [docs/architecture/OPERATION-LIFECYCLE.md](../architecture/OPERATION-LIFECYCLE.md)
- [docs/requirements/robson-api-requirements.md](../requirements/robson-api-requirements.md)

