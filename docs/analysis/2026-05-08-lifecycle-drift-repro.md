# TD-2026-05-05-001 Stale-Active Drift — Reproduction & Diagnostic

**Date**: 2026-05-08
**Author**: Claude Opus 4.7 (analysis pass), Leandro Damasio (operator)
**Status**: Final (Slice 0)
**Related**: TD-2026-05-05-001, ADR-0022, ADR-0023,
`docs/policies/UNTRACKED-POSITION-RECONCILIATION.md`,
`docs/implementation/TD-2026-05-05-001-CORE-LIFECYCLE-DRIFT.md`,
`docs/operations/2026-05-05-v3-slot-variation-fix.md`

---

## Executive Summary

**Problem Statement**: The v3 `ReconciliationWorker` walks the exchange-side
of the (Robson, Exchange) state pair only. It detects and closes positions
that exist on the exchange but are not tracked by Robson (UNTRACKED). It
does **not** detect or close positions that Robson tracks as `Active` but
are absent on the exchange.

**Key Findings**:

- The current loop in `robsond/src/reconciliation_worker.rs:73-87` is
  asymmetric by construction. There is no iteration over
  `store.positions().find_active()`.
- `PositionClosed` cannot be emitted from `Active` directly. The current
  paths require `Exiting` (`handle_exit_fill`) or go via `panic_close →
  PlaceExitOrder`, both of which assume the exchange position is real and
  that a market exit will fill it. Neither helps when the exchange has
  already zeroed the position.
- `ExitReason` has no semantics for "closed by reconciliation because
  exchange was missing"; the closest existing variants
  (`InsuranceStop`, `PositionError`) are wrong (no audit trail) or imply
  state Robson cannot prove (`InsuranceStop` requires a real fill).
- `ExchangePort` has `get_all_open_positions` but no method to retrieve a
  specific order's fill or per-symbol user trade history. Both are needed
  for high-quality close evidence. Both already exist at the connector
  layer (`BinanceRestClient::get_order_status`) or are trivial to add via
  `GET /fapi/v1/userTrades`.
- Slot accounting (`compute_slots_available` in
  `robsond/src/position_manager.rs:2823-2860`) and `/status.occupied_slots`
  derive from `find_active`/`find_risk_open`. They are correct **derivations**
  of the broken state — a stale `Active` automatically inflates
  `occupied_slots` and `latent_risk` until the underlying lifecycle is
  fixed. No separate accounting bug exists; the slot symptom is fully
  upstream.

**Recommended Action**: Proceed with the slice plan in
`docs/implementation/TD-2026-05-05-001-CORE-LIFECYCLE-DRIFT.md`,
incorporating Amendments §1 (Active-only), §2 (evidence ordering, no
silent fallback), §3 (startup gate policy with `abort` default).

**Estimated Effort**: ~5–8 days end-to-end (Slices 0–6), Slice 0 ≤ 2 hours.

---

## Current State

### System Overview

Robson v3 daemon (`robsond`) runs three reconciliation-adjacent components
during startup and steady state:

```
┌──────────────────────────────────────────────────────────────────┐
│ Daemon::run() (robsond/src/daemon.rs)                         │
│                                                                  │
│  1. rebuild_store + restore_positions (eventlog → projection)    │
│  2. startup ReconciliationWorker.scan_and_reconcile_blocking     │
│     ├── exchange → Robson check (UNTRACKED close)                │
│     └── exchange → Robson check ONLY (no reverse)         ◄── gap│
│  3b. startup_recovery (15m candle replay for missed stops)       │
│  4. position_monitor + WS clients + projector                    │
│  spawn ReconciliationWorker every reconciliation_interval (60s)  │
└──────────────────────────────────────────────────────────────────┘
```

The shared property of every existing close path is that it terminates in
`Event::PositionClosed { exit_reason: ExitReason::*, exit_price, realized_pnl,
... }` appended to the eventlog and applied to the projection. Downstream
consumers (`/status`, `/positions/:id`, `compute_slots_available`,
`monthly_state.realized_loss`, MonthlyHalt evaluator) all derive from
projected state via `find_active`, `find_risk_open`, `find_closed_in_month`,
`find_active_from_projection`.

### Observed Behavior

In the production incident on 2026-05-05, the home dashboard rendered a
BTCUSDT Long slot with `0.00%` variation while the operation detail still
showed `Active`. The 2026-05-05 slot variation fix
(`docs/operations/2026-05-05-v3-slot-variation-fix.md`) made the displayed
variation honest by valuing Active positions at the trailing stop after
crossing — but the underlying Active state was already terminal in reality.
The TD entry calls this out explicitly:

> The dashboard can show an occupied slot for a position that should be
> terminal. Monthly slot accounting can remain conservative because the
> stale position still occupies a slot. Realized PnL, `closed_at`, and
> final lifecycle state are not authoritative until a proper close event
> is recorded.

### Expected Behavior

Per the proposed I3 invariant (Active-only):

- A reconciliation pass MUST detect any `Active` position whose
  `(symbol, side)` is not in the exchange's open-position set.
- After grace period and a second consecutive missing observation, the
  daemon MUST gather evidence from exchange APIs and emit
  `Event::PositionClosed { exit_reason: ReconciledMissingOnExchange,
  closure_evidence: Reconciled(...), exit_price, realized_pnl,
  ... }`.
- The eventlog convergence requirement of MIG-v2.5#2 must be preserved:
  append-then-apply with idempotent re-runs.
- Slot accounting and MonthlyHalt evaluation MUST automatically reflect
  the close on the next read, with no separate plumbing.

### Root Cause Analysis

`ReconciliationWorker::scan_and_reconcile` was authored against the
narrower invariant I2 (UNTRACKED close) before I3 was articulated.
Symmetry was implicitly assumed via the legitimate `Active → Exiting →
Closed` path triggered by trailing-stop hits or panic close. Three classes
of event break that assumption:

1. **Forced liquidation by Binance** under maintenance margin breach.
   Robson never receives a fill notification on its own outbound channels;
   only the income history reflects it.
2. **Manual close on Binance UI** by an operator (out-of-policy but
   physically possible while credentials are loaded).
3. **Insurance stop fill while daemon is offline beyond startup_recovery's
   15m window**. `startup_recovery` covers stop crossings via candle
   replay, but it does not consult `GET /fapi/v1/order` on
   `insurance_stop_id` to confirm an actual fill on the exchange.

In all three, the exchange is the source of truth and Robson's local
projection is stale. The current architecture has no mechanism that
re-orients local state to exchange truth in this direction.

---

## Gaps

### Documentation Gaps

| Priority | File/Location | Issue | Impact |
|----------|---------------|-------|--------|
| P0 | `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md` | Missing I3 (reverse reconciliation, Active-only) | HIGH |
| P0 | `docs/runbooks/td-2026-05-05-001-stale-active-recovery.md` | Does not exist; required by Amendment §3 Path A | HIGH |
| P1 | `docs/adr/ADR-0022-robson-authored-position-invariant.md` | No reference to I3 in the changelog | MED |

### Code Gaps

| Priority | Component | Issue | Blocker For |
|----------|-----------|-------|-------------|
| P0 | `robsond/src/reconciliation_worker.rs:73-87` | Worker iterates only the exchange side | Symmetric reconciliation (Slice 4) |
| P0 | `robson-domain/src/entities.rs:347-361` | No `ExitReason::ReconciledMissingOnExchange` | Reconciled close audit trail (Slice 1) |
| P0 | `robson-domain/src/events.rs:291-306` | `PositionClosed` lacks `closure_evidence` field | Evidence-bearing close events (Slice 1) |
| P0 | `robson-exec/src/ports.rs:108-124` | `ExchangePort` cannot retrieve order/trade evidence | Evidence pipeline (Slice 3) |
| P0 | `robsond/src/position_manager.rs` | No `reconcile_close` path that goes from `Active` directly to `Closed` with evidence | Reconciled close (Slice 4) |
| P1 | `robsond/src/daemon.rs:480-518` | Startup gate counts only UNTRACKED, not stale-Active | Path A startup gate (Slice 5) |
| P2 | `cli/src/commands/` | No `reconcile-close` subcommand for Path A operator workflow | Operator runbook (Slice 5) |

### Infrastructure Gaps

| Priority | Resource | Issue | Impact |
|----------|----------|-------|--------|
| P1 | Prometheus dashboards | No metric for `robson_reconciliation_stale_active_total` / `robson_reconciliation_estimated_closes_total` | MED |
| P2 | Alertmanager | No CRITICAL route for `position_reconciled_with_estimated_evidence` | MED |

---

## Priority Tracks

### Track 1: Slice 0 — Reproduction & analysis (THIS DOCUMENT)
**Effort**: ≤ 2 hours
**Dependencies**: None
**Deliverables**:
- Branch `fix/td-2026-05-05-001-core-lifecycle-drift`.
- Implementation guide
  `docs/implementation/TD-2026-05-05-001-CORE-LIFECYCLE-DRIFT.md`.
- This analysis document.
- Unit test
  `test_reconciliation_does_not_close_active_missing_on_exchange` in
  `robsond/src/reconciliation_worker.rs` documenting current (buggy)
  behavior as a baseline canary.

**Tasks**:
1. Read TD entry, policy, ADR-0022, and current worker code.
2. Inspect `ExchangePort`, `ExitReason`, and projector handlers for
   evidence-related capabilities.
3. Author guide with Amendments §1/§2/§3 baked in.
4. Add Slice 0 reproduction test.
5. Run `cargo fmt --all --check`, `cargo test -p robsond
   reconciliation_worker --lib`, `cargo clippy -p robsond --lib -- -D
   warnings`.
6. Hand-off: report diff and recommendation; do not advance to Slice 1
   without operator approval.

### Track 2: Slices 1–4 — Domain → Evidence pipeline → Symmetric loop
**Effort**: 3–5 days
**Dependencies**: Slice 0 approved.
**Deliverables**:
- `ClosureEvidence` + `ReconciledMissingOnExchange` (Slice 1).
- Policy I3 in docs (Slice 2).
- Evidence-retrieval methods on `ExchangePort` (Slice 3).
- Symmetric `ReconciliationWorker` loop and `reconcile_close` (Slice 4).

### Track 3: Slices 5–6 — Startup gate + operator workflow + regression
**Effort**: 1–2 days
**Dependencies**: Slice 4 merged.
**Deliverables**:
- Config knob `reconciliation.on_startup_stale_active`.
- CLI subcommand `robson-cli reconcile-close`.
- Runbook `docs/runbooks/td-2026-05-05-001-stale-active-recovery.md`.
- E2E regression for slot reclamation, monthly accounting, eventlog
  replay convergence.

---

## Execution Selector

| Objective | Entry Point | Effort |
|---|---|---|
| Reproduce drift in tests | EP-001 | 30m–1h |
| Land domain types and policy | EP-002 | 4–6h |
| Land symmetric loop with evidence | EP-003 | 1–2 days |
| Land startup gate + runbook | EP-004 | 1 day |

### Default Execution Order
1. EP-001 (Slice 0 — locks the bug in)
2. EP-002 (Slice 1 + 2 — domain + policy)
3. EP-003 (Slice 3 + 4 — pipeline + symmetric loop)
4. EP-004 (Slice 5 + 6 — gate + regression)

---

## Entry Points

### EP-001: Slice 0 — Reproduce stale-Active drift in tests

**Objective**: Add a unit test that documents current (buggy) reconciliation
behavior as a baseline canary, plus this analysis doc and the
implementation guide.

**Preconditions**:
```bash
# Inside repo root
git rev-parse --show-toplevel | grep -q '/robson$'

# On the Slice 0 branch
git branch --show-current | grep -q '^fix/td-2026-05-05-001-core-lifecycle-drift$'

# Tree clean before starting (besides the three Slice 0 files)
git status --short
```

**Inputs**:
- `BRANCH`: `fix/td-2026-05-05-001-core-lifecycle-drift`

**Steps** (already executed in Slice 0; re-runnable for verification):
```bash
# Step 1: format check
cd . && cargo fmt --all --check

# Step 2: run the new reproduction test in isolation
cargo test -p robsond --lib reconciliation_worker::tests::test_reconciliation_does_not_close_active_missing_on_exchange -- --nocapture

# Step 3: run the full reconciliation_worker test module
cargo test -p robsond --lib reconciliation_worker

# Step 4: clippy (no new warnings)
cargo clippy -p robsond --lib -- -D warnings
```

**Expected Outcome**:
```bash
# PASS condition 1: test compiles and passes
cargo test -p robsond --lib reconciliation_worker::tests::test_reconciliation_does_not_close_active_missing_on_exchange 2>&1 | grep -q "test result: ok"

# PASS condition 2: existing tests in the module still pass
cargo test -p robsond --lib reconciliation_worker 2>&1 | grep -q "test result: ok"

# PASS condition 3: no new clippy warnings
cargo clippy -p robsond --lib -- -D warnings 2>&1 | grep -qv 'warning'
```

**Failure Detection**:
- FAIL if the new test `panic!`s or fails — current behavior would have to
  match the post-fix world, which it cannot before Slice 4.
- FAIL if `cargo fmt --all --check` reports diffs — push of unformatted
  Rust to CI is forbidden.
- FAIL if any other `reconciliation_worker::tests` regressed.

**Rollback**:
```bash
git restore robsond/src/reconciliation_worker.rs
git restore docs/analysis/2026-05-08-lifecycle-drift-repro.md
git restore docs/implementation/TD-2026-05-05-001-CORE-LIFECYCLE-DRIFT.md
git checkout main
git branch -D fix/td-2026-05-05-001-core-lifecycle-drift
cd . && cargo build --workspace
```

---

### EP-002: Slice 1 + 2 — Domain types + Policy I3

**Objective**: Land `ClosureEvidence`, `ExitReason::ReconciledMissingOnExchange`,
and the I3 documentation update. Pre-requisite for any worker change.

(Detailed once Slice 0 is approved; structured identically to EP-001.)

---

### EP-003: Slice 3 + 4 — Evidence pipeline + symmetric loop

(Defined in the implementation guide §Slice plan §Slices 3, 4. Skeleton
EP entry deferred until EP-002 lands.)

---

### EP-004: Slice 5 + 6 — Startup gate + regression

(Defined in the implementation guide §Slice plan §Slices 5, 6.)

---

## Verification Commands Reference

**Check on the right branch**:
```bash
git branch --show-current | grep -q '^fix/td-2026-05-05-001-core-lifecycle-drift$' && echo PASS || echo FAIL
```

**Check Slice 0 baseline test exists**:
```bash
grep -q 'test_reconciliation_does_not_close_active_missing_on_exchange' robsond/src/reconciliation_worker.rs && echo PASS || echo FAIL
```

**Check workspace formatted**:
```bash
(cd . && cargo fmt --all --check) && echo PASS || echo FAIL
```

**Check the reconciliation worker tests pass**:
```bash
(cd . && cargo test -p robsond --lib reconciliation_worker 2>&1 | tail -1 | grep -q 'test result: ok') && echo PASS || echo FAIL
```

---

## Rollback Notes

### Rollback Pattern 1: Slice 0 only

```bash
git restore robsond/src/reconciliation_worker.rs
git restore docs/analysis/2026-05-08-lifecycle-drift-repro.md
git restore docs/implementation/TD-2026-05-05-001-CORE-LIFECYCLE-DRIFT.md
git checkout main
git branch -D fix/td-2026-05-05-001-core-lifecycle-drift
```

### Rollback Pattern 2: Slice plan abandonment

```bash
# After commits exist on the branch:
git checkout main
git branch -D fix/td-2026-05-05-001-core-lifecycle-drift   # destructive — confirm with operator first
```

### Rollback Pattern 3: Database

Slice 0 introduces no schema changes. Slices 1–6 are additive (new event
fields default to `RealFill` for legacy events; no new tables) and require
no migration revert.

---

## Appendices

### Appendix A — Current asymmetric reconciliation flow

```text
ReconciliationWorker::scan_and_reconcile (robsond/src/reconciliation_worker.rs:73-87)

  exchange_positions ← exchange.get_all_open_positions()
  for each exchange_position:
    if NOT is_tracked(exchange_position):
        handle_untracked_position(exchange_position)   # CLOSE on exchange
        reconciled += 1
  return reconciled

is_tracked (line 94-101):
  store.positions().find_active_by_symbol_and_side(symbol, side).is_some()
```

The store side is queried only as a yes/no membership test for items found
on the exchange. The store is never iterated in its own right.

### Appendix B — Why `find_active_by_symbol_and_side` is not enough

`find_active_by_symbol_and_side` returns Some when ANY of `Entering`,
`Active`, or `Exiting` matches by symbol+side. Two consequences:

1. The membership test is broader than `Active`-only — exchange-side
   reconciliation correctly tolerates positions whose entry order is
   in-flight (`Entering`) or whose exit order is in-flight (`Exiting`).
   Acceptable for I2.
2. For the reverse direction (I3) we cannot reuse this method symmetrically.
   We need a method like `find_by_state("active")` (already exists on
   `PositionRepository`, `robson-store/src/repository.rs:60`) or a
   simple filter on `find_active().filter(state == Active)`. The new loop
   in Slice 4 will use the latter for clarity.

### Appendix C — Decisions log

| Decision | Alternative | Rationale |
|---|---|---|
| `I3` is `Active`-only | Apply to `Active`, `Entering`, `Exiting` | `Entering`/`Exiting` could be mid-flight on the exchange (placement and cancel/exit latency); auto-closing them risks double-close. Operator can address those manually until a follow-up TD designs a safe path. |
| Add `closure_evidence` field to `PositionClosed` | New event type `PositionReconciledClosed` | A single event type keeps projector handlers, monthly_state aggregation, and replay paths unchanged in shape. The new field is opt-in (default `RealFill`). |
| Default startup policy is `abort` | Default `auto_reconcile` | Operator's "fail closed by default" requirement. `auto_reconcile` is opt-in after the runbook has been exercised on testnet. |
| `Estimated` evidence triggers CRITICAL alert and never auto-closes at startup | Auto-close on `Estimated` with operator notification | Capital safety: a wrong estimated PnL pollutes monthly accounting and may delay or prematurely trigger MonthlyHalt. Operator confirmation is cheap; pollution is expensive. |
| Add `get_order_by_exchange_id` and `get_user_trades_since` to `ExchangePort` | Use the existing connector layer directly from `position_manager` | Hexagonal boundary preservation. The reconciliation pipeline depends on `ExchangePort`, not on `BinanceExchangeAdapter` concretely. The stub gains test seams. |

### Appendix D — Reference materials

- `docs/technical-debt.md` (TD-2026-05-05-001 entry)
- `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md`
- `docs/policies/SYMBOL-AGNOSTIC-POLICIES.md`
- `docs/adr/ADR-0022-robson-authored-position-invariant.md`
- `docs/operations/2026-05-05-v3-slot-variation-fix.md`
- `robsond/src/reconciliation_worker.rs`
- `robsond/src/position_manager.rs:2470-2860`
- `robson-exec/src/ports.rs`
- `robson-domain/src/entities.rs`
- `robson-domain/src/events.rs`
- `robson-connectors/src/binance_rest.rs:427-440`

---

## Changelog

| Date | Change | Author |
|---|---|---|
| 2026-05-08 | Initial draft (Slice 0). | Claude Opus 4.7 |
