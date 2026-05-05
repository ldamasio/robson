# 2026-05-05 v3 Slot Variation Fix

## Context

Production showed an occupied BTCUSDT Long slot with `0.00%` variation on the
home dashboard even though the position was expected to have stopped out as a
loss. The operation detail still showed the position as `Active`.

The immediate display bug had two causes:

1. `/status` returned `pnl: "0"` for the active position because the restored
   domain state had stale `current_price` equal to `entry_price`.
2. The frontend rendered `pnl` as a percentage, although the backend PnL field
   is an absolute quote-currency amount.

## Change

The backend now owns percentage variation business logic:

- `PositionSummary` includes `current_price`.
- `PositionSummary` includes `variation_pct`.
- Active and Exiting summaries attempt to fetch live exchange price.
- Closed summaries compute variation from `entry_price` and `exit_price`.
- Active summaries use live price before stop-hit, and use trailing stop price
  as valuation once the live price crosses the stop.

The frontend now:

- Normalizes `variation_pct` and `current_price`.
- Displays `variation_pct`, not `pnl`, on the home dashboard.
- Sorts occupied slots from oldest to newest by `created_at`.
- Keeps free slots on the right.

## Validation

Local validation before deployment:

```bash
pnpm test -- --run tests/unit/slots.test.ts tests/unit/dashboard-logic.test.ts
pnpm check
cargo test -p robsond api::tests::position_summary --features postgres
cargo test -p robsond --test api_contract --features postgres
```

Results:

- Frontend unit tests passed.
- `svelte-check` reported 0 errors and 4 pre-existing layout warnings.
- Backend focused summary tests passed.
- Backend API contract tests passed.

## Known Follow-Up

This fix improves the displayed variation, but does not close the underlying
core lifecycle drift where Robson can keep a core position as `Active` even when
the exchange position is already gone. That debt is tracked centrally as
`TD-2026-05-05-001` in `docs/technical-debt.md`.
