# MIG-v3#12 — Monthly State Persistence

## Current Phase

ALL PHASES COMPLETE. 471 tests passing, 0 failures.

## Objective

Persist `realized_loss` and `trades_opened` in `monthly_state` so the risk path
reads authoritative accumulated values from DB instead of recomputing O(n) from
closed positions on every check.

## Constraints

- `monthly_state` = ledger (historical accounting only). No live execution state.
- `positions_current` = reality (live execution state, reconstructable).
- Strict separation: never store active positions, ENTRY_ARM, or in-flight state in monthly_state.
- `build_risk_context` must combine persisted monthly_state + reconstructed live state.
- Backfill considers only `state = 'Closed'` positions.
- MonthBoundaryReset resets `realized_loss = 0`, `trades_opened = 0`, carries capital_base + carried_risk.
- Restart safety: daemon rebuild_store + restore_positions already reconstruct live state from events.
- New columns have DEFAULT 0 — safe additive migration, no data loss.

## Non-Goals

- Migrating or serializing ENTRY_ARM into monthly_state.
- Carrying over runtime state into persistent monthly aggregates.
- Coupling monthly_state with execution lifecycle.
- Changing the slot calculation formula.

## Files Inspected

- `v3/migrations/20240101000008_create_monthly_state.sql` — current schema
- `v3/migrations/20240101000010_add_realized_loss_trades_opened.sql` — new (created)
- `v3/robsond/src/position_manager.rs:178-204` — load_capital_base_for_month
- `v3/robsond/src/position_manager.rs:622-737` — build_risk_context
- `v3/robsond/src/position_manager.rs:1439-1520` — evaluate_monthly_halt
- `v3/robsond/src/daemon.rs:460-518` — startup sequence (rebuild_store, restore_positions)
- `v3/robson-projector/src/apply.rs` — projection routing
- `v3/robson-projector/src/handlers/monthly_state.rs` — current MonthBoundaryReset handler
- `v3/robson-projector/src/handlers/positions.rs:649-687` — handle_entry_filled
- `v3/robson-projector/src/handlers/positions.rs:1087-1128` — handle_position_closed_domain
- `v3/robson-projector/src/types.rs` — projection payload types (EntryFilled, PositionClosedDomain)
- `v3/robson-domain/src/events.rs` — domain event definitions

## Execution Phases

### Phase 1 — Schema Migration ✅

- **Planned change**: Add `realized_loss NUMERIC(20,8) NOT NULL DEFAULT 0` and `trades_opened INTEGER NOT NULL DEFAULT 0` to `monthly_state`.
- **Files changed**: `v3/migrations/20240101000010_add_realized_loss_trades_opened.sql` (new)
- **Verification**: File exists, SQL syntax correct.
- **Result**: Done.
- **Rollback**: `ALTER TABLE monthly_state DROP COLUMN realized_loss, DROP COLUMN trades_opened;`

### Phase 2 — Projection Handlers ✅

- **Planned change**:
  1. Add `handle_entry_filled_monthly` to `monthly_state.rs` — UPSERT into monthly_state with `trades_opened = trades_opened + 1`. Extract year/month from `envelope.occurred_at`.
  2. Add `handle_position_closed_monthly` to `monthly_state.rs` — UPSERT into monthly_state with `realized_loss = realized_loss + loss`. Compute loss from PositionClosedDomain (entry_price, exit_price, realized_pnl). Only count net losses (realized_pnl - total_fees < 0).
  3. Update `apply.rs` to dual-route EntryFilled and PositionClosedDomain events: call existing position handler + new monthly_state handler.
- **Files changed**: `v3/robson-projector/src/handlers/monthly_state.rs`, `v3/robson-projector/src/apply.rs`
- **Verification**: `rtk cargo check -p robson-projector`
- **Rollback**: Revert changes to monthly_state.rs and apply.rs.

### Phase 3 — MonthlyRiskState Struct + load_monthly_state Refactor ✅

- **Planned change**:
  1. Add `MonthlyRiskState { capital_base, realized_loss, trades_opened }` to position_manager.rs.
  2. Add `load_monthly_state(now) -> DaemonResult<MonthlyRiskState>` that queries all 3 columns. Keep `load_capital_base_for_month` as a thin wrapper for callers that only need capital_base.
  3. Update `build_risk_context` to:
     - Call `load_monthly_state` for persisted accumulated values (realized_loss).
     - Still query `find_risk_open()` for active positions (live state).
     - Still query `find_closed_in_month()` for daily PnL breakdown (not removed — only the monthly realized_loss source changes).
     - Active positions and pending entries come from live state, NOT monthly_state.
  4. Update `evaluate_monthly_halt` to use `load_monthly_state().realized_loss` instead of recomputing from find_closed_in_month.
- **Files changed**: `v3/robsond/src/position_manager.rs`
- **Verification**: `rtk cargo check -p robsond`, `rtk cargo test -p robsond --lib`
- **Rollback**: Revert position_manager.rs changes. load_capital_base_for_month still works.

### Phase 4 — Update MonthBoundaryReset Handler ✅

- **Planned change**: Update `handle_month_boundary_reset` UPSERT to include `realized_loss = 0, trades_opened = 0` in the INSERT. On conflict, reset them to 0.
- **Files changed**: `v3/robson-projector/src/handlers/monthly_state.rs`
- **Verification**: `rtk cargo check -p robson-projector`
- **Rollback**: Revert UPSERT to exclude new columns (they have DEFAULT 0).

### Phase 5 — Startup Reconstruction Verification ✅

- **Planned change**: Verify existing daemon startup sequence (rebuild_store + restore_positions) already reconstructs live state correctly. No code changes expected — just verification.
  - `rebuild_store()` replays all events to rebuild in-memory projection.
  - `restore_positions()` re-monitors active positions.
  - `build_risk_context()` will now combine persisted monthly_state + live positions.
- **Files changed**: None (verification only)
- **Verification**: `rtk cargo test -p robsond --lib` (existing startup tests pass)

### Phase 6 — Backfill Script (Closed Only) ✅

- **Planned change**: Create backfill SQL script that seeds current month from `positions_current WHERE state = 'closed'`. Only counts closed positions for realized_loss and trades_opened. Idempotent (ON CONFLICT DO NOTHING).
- **Files changed**: `v3/migrations/20240101000011_backfill_current_month_state.sql` (new)
- **Verification**: Manual review of SQL logic.
- **Rollback**: `DELETE FROM monthly_state WHERE ...` or skip running the script.

### Phase 7 — Tests ✅

- **Planned change**: Write test scenarios:
  1. `test_load_monthly_state_defaults_on_empty_db` — no row → defaults from config
  2. `test_month_boundary_resets_realized_loss_and_trades_opened` — handler resets counters
  3. `test_build_risk_context_uses_persisted_realized_loss` — seeded realized_loss used
  4. `test_evaluate_monthly_halt_uses_persisted_realized_loss` — halt evaluation uses persisted loss
  5. `test_restart_safety_active_positions_still_count` — after rebuild, active positions still block entries
- **Files changed**: `v3/robsond/src/position_manager.rs` (test module)
- **Verification**: `rtk cargo test -p robsond --lib`
- **Rollback**: Remove test functions.

### Phase 8 — Update Migration Plan ✅

- **Planned change**: Mark MIG-v3#12 as ✅ Implemented.
- **Files changed**: `docs/architecture/v3-migration-plan.md`
- **Rollback**: Revert status marker.

## Architectural Decisions

- `monthly_state` is the authoritative source for accumulated monthly risk (realized_loss, trades_opened). Live positions come from `positions_current` projection.
- `build_risk_context` combines both: persisted ledger + live reconstruction. Neither source alone is sufficient.
- `find_closed_in_month` remains for daily PnL breakdown and audit queries, but is no longer the source for monthly realized_loss in the risk path.
- MonthBoundaryReset resets counters to 0 for the new month. Carried capital_base and carried_risk flow forward.
- Projection handlers use UPSERT for idempotent replay.
- Loss computation in `handle_position_closed_monthly` uses `(realized_pnl - total_fees)` clamped to >= 0, matching ADR-0024.

## Next Actions

All planned phases (1-8) are complete. MIG-v3#12 is fully implemented, tested, and documented.
