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

All slices through 5B2B are implemented and merged:

- **Symmetric reconciliation**: `ReconciliationWorker` now iterates both sides —
  exchange→Robson (UNTRACKED close) and Robson→exchange (stale-Active close).
- **Evidence pipeline**: `ExchangePort` provides `get_order_by_exchange_id` and
  `get_user_trades_since`. Only `OrderFillRecord` and `UserTradeRecord` are valid.
  `Estimated` is permanently hard-blocked.
- **Startup abort gate**: exit code 78 on any stale-Active at startup (default policy).
- **Manual recovery path**: `robson-cli reconcile-close` + `POST /reconcile-close`.
- **Startup `auto_reconcile`**: implemented and opt-in. Production configuration
  was read-only verified as enabled on 2026-08-06. The no-evidence refusal is
  now corrected in repository source; operational validation and the remaining
  batch atomicity gap are tracked separately below.

### What Remains (v3)

- [x] Merge branch `feat/td-2026-05-05-001-slice-5b2b-auto-reconcile` to main.
- [x] Update this entry Status to `Closed` after merge.

### What is Deferred (v4)

- Testnet drill for startup `auto_reconcile` (MIG-v4#3).
- Startup reconciliation batch atomicity correction (TD-2026-08-06-002).

`Estimated` evidence remains permanently prohibited in `reconcile_close`. The
historical MIG-v4#7 roadmap item does not authorize an operator-confirmed
exception.

### References

- `docs/agents/td-2026-05-05-001-execution-memory.md`
- `docs/implementation/TD-2026-05-05-001-CORE-LIFECYCLE-DRIFT.md`
- `docs/runbooks/td-2026-05-05-001-stale-active-recovery.md`
- `robsond/src/reconciliation_worker.rs`
- `robsond/src/position_manager.rs`
- `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md`

---

## TD-2026-08-06-001: Startup auto-reconcile continues without evidence

**Status**: Source correction present; operational validation pending
**Severity**: High
**Area**: `robsond` startup reconciliation, financial lifecycle safety
**Discovered**: 2026-08-06
**Source correction recorded**: 2026-11-16

### Historical evidence

The 2026-08-06 implementation logged a warning and returned success when
startup `auto_reconcile` detected a stale-Active position but
`gather_real_evidence` returned no unambiguous match. Startup continued so the
periodic reconciliation worker might resolve the position. Production
configuration was read-only verified as `auto_reconcile` on that date.

This differed from ADR-0022 and the reconciliation policy, which require exit
78 with no partial close when any stale-Active position lacks real evidence.
The historical description above is not the current repository behavior.

### Repository correction and remaining acceptance

`run_startup_auto_reconcile` in `robsond/src/daemon.rs` now returns
`StartupStaleActiveDetected` if any required real evidence is absent, before
Phase 2 applies any close. Its existing startup error mapping is exit 78. The
first phase remains read-only. Only `OrderFillRecord` and `UserTradeRecord`
qualify; `AccountSnapshot` and `Estimated` remain rejected for automatic
startup and the manual recovery evidence rules are unchanged.

This source correction does not establish deployment or completed operational
validation. Applicable review, validation and the outstanding testnet drill
remain necessary before calling the operational path validated. It also does
not resolve Phase 2's partial-batch liability in TD-2026-08-06-002: the policy's
no-partial-close requirement remains in force. This documentation update does
not change runtime code.

---

## TD-2026-08-06-002: Startup reconciliation batch is not atomic

**Status**: Open
**Severity**: High
**Area**: `robsond` startup reconciliation, financial lifecycle safety
**Discovered**: 2026-08-06

### Evidence

`run_startup_auto_reconcile` gathers evidence for the full stale-Active set
before writing, but `apply_startup_auto_reconcile_batch` then calls
`reconcile_close` once per position in a sequential loop. Terminal events are
persisted per position, not as one batch. A rejection or persistence error on a
later position cannot roll back an earlier persisted close. In addition,
`record_venue_close(false)` can leave a position locally `Closed` under fencing;
a close result alone is not proof of a durable terminal event.

The read-only evidence phase is all-or-nothing, but the write phase is not a
batch transaction. Therefore the policy's no-partial-close guarantee is not
mechanically enforced for multi-position startup recovery.

### Required correction

- Choose and document an atomic batch strategy or an explicit resumable
  protocol whose state and operator semantics preserve the invariant.
- Add a regression test where a later apply fails after an earlier close would
  otherwise persist.
- Keep per-position idempotency and real-evidence validation.
- Complete the deferred testnet drill before declaring the operational path
  validated.

The correction changes a real-money lifecycle path and requires a separate,
explicitly approved PR. This documentation cleanup does not modify runtime code.

---

## TD-2026-08-15-001: Fee accounting drift on exits (issue #156)

**Status**: Open
**Severity**: Medium
**Area**: `robsond` order accounting, income ledger, reconciliation alerting
**Discovered**: 2026-08 (issue #156; confirmed by the 2026-08-15 doc-vs-code audit)

### Evidence

- When an order response carries `fills`, real commissions are summed; when it
  does not, `robson-connectors/src/binance/usdm/exchange.rs` falls back to an
  estimated fee of 0.1% of the cumulative quote.
- `PositionClosed` records `fees_paid + exit_fee` using whatever fee the order
  path produced; a close that used the fallback is persisted with an estimated
  fee, and no later path corrects the event.
- The `COMMISSION` income ledger is collected independently via
  `/fapi/v1/income` and matched to positions only approximately (symbol plus
  time window); it has no linkage back to the position manager to replace the
  estimate with the effective commission.
- The reconciliation worker emits a periodic financial-drift WARN (default
  60 s interval), which stays noisy while the estimate and the ledger disagree.

### Required correction

- Persist the effective exit commission (from fills or from a matched ledger
  row) or define an explicit reconciliation step that supersedes the estimate.
- Make ledger-to-position matching deterministic for robsond-authored exits
  (client order id, not symbol/time proximity).
- Bound or gate the drift WARN so an accepted, explained estimate does not
  alarm every minute.

The correction touches real-money accounting and requires its own explicitly
approved PR; this register entry only records the debt.
