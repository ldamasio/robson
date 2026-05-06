# Technical Debt Register

Central register for known Robson technical debt that is relevant to production
readiness, operator trust, or future v4 migration work.

Each item should stay short and actionable. Long investigations belong in
`docs/analysis/`, `docs/operations/`, or an ADR, with a link from this file.

---

## TD-2026-05-05-001: Core Position Lifecycle Drift

**Status**: Open
**Severity**: High
**Area**: `robsond`, reconciliation, position lifecycle
**Discovered**: 2026-05-05

### Summary

Robson can continue projecting a core position as `Active` even after the
exchange position has already disappeared or been liquidated outside the normal
Robson `PositionClosed` flow.

The existing reconciliation worker protects one side of the invariant: it
detects exchange-open positions that Robson is not tracking and closes them as
UNTRACKED. It does not yet protect the opposite side: Robson-tracked core
positions that no longer exist on the exchange.

### Impact

- The dashboard can show an occupied slot for a position that should be terminal.
- Monthly slot accounting can remain conservative because the stale position
  still occupies a slot.
- Realized PnL, `closed_at`, and final lifecycle state are not authoritative
  until a proper close event is recorded.
- Operators may see `Active` on the operation detail page even when the exchange
  no longer has the position open.

### Current Mitigation

The 2026-05-05 slot variation fix makes `/status` and `/positions/:id` expose
`variation_pct` from backend business logic instead of letting the frontend
interpret `pnl` as a percentage.

For `Active` positions, the API now fetches a live exchange price for display.
If the observed price has crossed the trailing stop, the displayed variation is
valued at the stop price instead of remaining at the stale in-memory
`current_price`. This improves operator visibility but does not close the
position lifecycle drift.

The monthly slot history UI now renders historical months as full dashboard
snapshots. In past months, unused capacity is labeled `Expired Slot` instead of
`Free Slot`, which keeps the current-month live vocabulary separate from the
historical view.

### Required Fix

Add core-position reconciliation for tracked positions that are missing on the
exchange:

1. Query active Robson core positions.
2. Query open exchange positions.
3. For each Robson `Active` position missing on the exchange, determine the
   safest terminal event source:
   - Prefer exchange order/trade history if available.
   - Otherwise record an explicit reconciliation-close event with conservative
     reason and evidence.
4. Emit/persist `PositionClosed` through the same eventlog/projector path used
   by normal exits.
5. Ensure monthly accounting and slot availability are recalculated from the
   terminal projection.
6. Add regression coverage for "Robson active, exchange missing".

### References

- `v3/robsond/src/reconciliation_worker.rs`
- `v3/robsond/src/position_manager.rs`
- `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md`
- `docs/operations/2026-05-05-v3-slot-variation-fix.md`
