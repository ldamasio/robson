# Technical Debt Register

Central register for known Robson technical debt that is relevant to production
readiness, operator trust, or future v4 migration work.

Each item should stay short and actionable. Long investigations belong in
`docs/analysis/`, `docs/operations/`, or an ADR, with a link from this file.

---

## TD-2026-05-05-001: Core Position Lifecycle Drift

**Status**: Closed
**Severity**: High → Resolved
**Area**: `robsond`, reconciliation, position lifecycle
**Discovered**: 2026-05-05
**Resolved by**: TD-2026-05-05-001 Slices 0–5B2B (commits `28b7a58e`→`5458c36e`)

### Resolution Summary

All slices implemented and merged (pending final PR merge from branch
`feat/td-2026-05-05-001-slice-5b2b-auto-reconcile`):

- **Symmetric reconciliation**: `ReconciliationWorker` now iterates both sides —
  exchange→Robson (UNTRACKED close) and Robson→exchange (stale-Active close).
- **Evidence pipeline**: `ExchangePort` provides `get_order_by_exchange_id` and
  `get_user_trades_since`. Only `OrderFillRecord` and `UserTradeRecord` are valid.
  `Estimated` is permanently hard-blocked.
- **Startup abort gate**: exit code 78 on any stale-Active at startup (default policy).
- **Manual recovery path**: `robson-cli reconcile-close` + `POST /reconcile-close`.
- **Startup `auto_reconcile`**: implemented (opt-in), deferred to production use in v4
  (MIG-v4#3 in `docs/architecture/v4-backlog.md`).

### What Remains (v3)

- [x] Merge branch `feat/td-2026-05-05-001-slice-5b2b-auto-reconcile` to main.
- [x] Update this entry Status to `Closed` after merge.

### What is Deferred (v4)

- Testnet drill for startup `auto_reconcile` (MIG-v4#3).
- Estimated-evidence operator-confirmed close path (MIG-v4#7).

### References

- `docs/agents/td-2026-05-05-001-execution-memory.md`
- `docs/implementation/TD-2026-05-05-001-CORE-LIFECYCLE-DRIFT.md`
- `docs/runbooks/td-2026-05-05-001-stale-active-recovery.md`
- `robsond/src/reconciliation_worker.rs`
- `robsond/src/position_manager.rs`
- `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md`
