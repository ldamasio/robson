# TD-2026-05-08-002 — Monthly Halt Test Regression / Validation Gate Noise

**Date**: 2026-05-08
**Author**: Claude Opus 4.7 (diagnostic), Leandro Damasio (operator)
**Status**: Final (diagnostic only; minimal fix proposed below)
**Related**: TD-2026-05-05-001 (Slices 3+ depend on this gate being trustworthy),
MIG-v3#12 (introduced the persistent `monthly_state` table — origin of the
postgres-feature `load_monthly_state` branch),
[ADR-0024 — Dynamic Slots](../adr/),
`v3/robsond/src/position_manager.rs:225-284`.

---

## Executive Summary

**Problem**: `cargo test -p robsond --tests --features postgres` fails 8
out of 13 tests in `position_manager::tests` related to `monthly_halt`,
`realized_loss`, and `closed_losses`. The same tests **all pass** under
`cargo test -p robsond --lib` (no postgres feature).

**Cause**: A feature-gated branch in `load_monthly_state` returns
`realized_loss = 0` whenever `event_log_pool` is `None`. In unit tests
the manager is built via `create_test_manager`, which does NOT wire a
PG pool. So under `--features postgres` the in-memory closed positions
saved by the test setup are silently ignored, and `evaluate_monthly_halt`
never sees the loss it was given.

**Classification**:

- **It is**: a feature-gating bug in the **test path only**.
- **It is not**: a real production monthly-halt regression (production
  always wires the pool; the early-return branch is unreachable there).
- **It is not**: shared-state contamination between tests, an `Utc::now()`
  month-boundary race, or an unrelated change.

**Fix proposed (small, safe)**: in the `#[cfg(feature = "postgres")]`
arm of `load_monthly_state`, when `event_log_pool` is `None`, fall
through to the same in-memory aggregation the `#[cfg(not(feature =
"postgres"))]` arm already implements. This is achieved by extracting
a single shared helper `load_monthly_state_in_memory`. ~25 line refactor.
No production behavior change.

**Recommendation**: **Apply the fix in a single commit on this branch**
before opening Slice 3 of TD-2026-05-05-001. Rationale: Slice 4 will
exercise `evaluate_monthly_halt` end-to-end via reconciled closes; it
needs the gate green. Risk of the fix is bounded — no schema, no port,
no production code path changes.

---

## Reproduction

### Without `--features postgres` (in-memory branch)

```bash
cd /home/psyctl/apps/robson/v3
cargo test -p robsond --lib monthly_halt
```

**Result**: `cargo test: 13 passed, 166 filtered out`. ALL monthly_halt
tests pass.

### With `--features postgres` (postgres branch, no DB wired)

```bash
cargo test -p robsond --tests --features postgres monthly_halt
```

**Result**: `5 passed; 8 failed; 0 ignored; 0 measured; 169 filtered out`.
Failing tests:

```
position_manager::tests::test_evaluate_monthly_halt_accounts_for_fees
position_manager::tests::test_evaluate_monthly_halt_uses_persisted_realized_loss
position_manager::tests::test_monthly_halt_auto_trigger_blocks_subsequent_arm
position_manager::tests::test_monthly_halt_auto_triggers_at_exactly_4_pct_loss
position_manager::tests::test_monthly_halt_does_not_retrigger_if_already_halted
position_manager::tests::test_monthly_halt_triggered_at_399_pct_loss
position_manager::tests::test_monthly_halt_triggered_by_combined_realized_loss_and_latent_risk
position_manager::tests::test_monthly_halt_auto_trigger_closes_active_positions
```

(Also failing under the broader `--tests` selector, not just
`monthly_halt`: `test_load_monthly_state_reflects_closed_losses` and
`test_build_risk_context_uses_persisted_realized_loss`.)

### Isolated stack trace

```bash
RUST_BACKTRACE=1 cargo test -p robsond --tests --features postgres \
  test_evaluate_monthly_halt_accounts_for_fees -- --nocapture
```

```
thread 'position_manager::tests::test_evaluate_monthly_halt_accounts_for_fees'
  panicked at robsond/src/position_manager.rs:5030:9:
  net -400 (PnL -350, fees -50) must trigger MonthlyHalt
   0: __rustc::rust_begin_unwind
   1: core::panicking::panic_fmt
   2: robsond::position_manager::tests::test_evaluate_monthly_halt_accounts_for_fees::{{closure}}
   ...
```

The panic is the test assertion itself — `evaluate_monthly_halt()`
returned `false` when `true` was expected. No internal error path was
hit; the function simply observed `realized_loss = 0`.

---

## Root Cause

`v3/robsond/src/position_manager.rs:225-284` defines two feature-gated
implementations of `load_monthly_state`:

```rust
// (lines 224-257)
#[cfg(feature = "postgres")]
pub(crate) async fn load_monthly_state(...) -> DaemonResult<MonthlyRiskState> {
    let configured_capital = self.configured_capital();
    let Some(pool) = &self.event_log_pool else {
        return Ok(MonthlyRiskState {
            capital_base: configured_capital,
            realized_loss: Decimal::ZERO,        // ← silently zero
            trades_opened: 0,
        });
    };
    // ... real DB query path ...
}

// (lines 259-284)
#[cfg(not(feature = "postgres"))]
pub(crate) async fn load_monthly_state(...) -> DaemonResult<MonthlyRiskState> {
    // No DB available: compute realized_loss from in-memory store for
    // correctness in tests and non-postgres mode.
    let monthly_closed =
        self.store.positions().find_closed_in_month(now.year(), now.month()).await?;
    // ... aggregate net losses ...
}
```

The intent of the non-postgres branch is correct: when there is no DB,
read from the in-memory store. The postgres branch implements the same
fallback when `event_log_pool` is `None`, **except** it forgets to read
the in-memory store and returns zero instead.

`create_test_manager` (`position_manager.rs:2958-2961`) constructs a
manager via `create_test_manager_with_approval_policy` and never calls
`with_event_log_pool` (line 365: `self.event_log_pool = Some(pool);`).
The default is `None`, set at line 345.

**Net effect**: with `--features postgres`, every unit test that uses
`create_test_manager` reads `realized_loss = 0` regardless of how many
closed positions it saved into the in-memory store. The 8 monthly-halt
tests that depend on `realized_loss` reflecting their fixtures all
panic on the first assertion.

In production, `daemon.rs` always wires the pool before constructing
the manager (`with_event_log_pool(pool)` is called unconditionally for
postgres-feature builds), so the broken branch is unreachable. **No
production regression exists.**

---

## What this is NOT

| Hypothesis (pre-investigation) | Verdict | Evidence |
|---|---|---|
| Real monthly-halt bug in production | **Rejected** | Production wires `event_log_pool` unconditionally; the buggy branch is unreachable. |
| `Utc::now()` month-boundary flake | **Rejected** | The bug reproduces at any time of any month with deterministic state. |
| Shared state across tests | **Rejected** | Each test calls `create_test_manager()` which constructs a fresh `MemoryStore` and `CircuitBreaker`. |
| MIG-v3#11 / MIG-v3#12 introduced an unintended cross-effect | **Rejected** | The early-return branch existed since MIG-v3#12 (2026-04-27). The bug pre-dates Slice 0 of TD-2026-05-05-001. |
| Slice 1 of TD-2026-05-05-001 broke it | **Rejected** | Verified by stashing Slice 1 work and reproducing the same failure on commit `28b7a58e` (Slice 0 / parent). |
| Test setup forgets to call a builder method | **Partial — root cause is upstream** | `create_test_manager` does not call `with_event_log_pool`. But the design intent of "if no pool, fall back to in-memory" is encoded in the non-postgres branch and merely missing from the postgres branch. Fixing test setup would require either provisioning a real PG for every unit test (heavy) or threading a mock through every `create_test_manager` callsite (invasive). The minimal fix lives in the function. |

---

## Proposed Fix

Single change to `v3/robsond/src/position_manager.rs`:

1. Extract the existing in-memory aggregation logic from the
   `cfg(not(feature = "postgres"))` arm into a private helper:

   ```rust
   async fn load_monthly_state_in_memory(
       &self,
       now: chrono::DateTime<chrono::Utc>,
   ) -> DaemonResult<MonthlyRiskState> {
       let monthly_closed =
           self.store.positions().find_closed_in_month(now.year(), now.month()).await?;
       let realized_loss: Decimal = monthly_closed
           .iter()
           .map(|p| {
               let net = p.realized_pnl - p.fees_paid;
               if net < Decimal::ZERO { net.abs() } else { Decimal::ZERO }
           })
           .sum();
       Ok(MonthlyRiskState {
           capital_base: self.configured_capital(),
           realized_loss,
           trades_opened: monthly_closed.len() as i32,
       })
   }
   ```

2. Replace the early-return-zero branch in the postgres arm with a
   delegating call to the helper:

   ```rust
   #[cfg(feature = "postgres")]
   pub(crate) async fn load_monthly_state(...) -> DaemonResult<MonthlyRiskState> {
       let Some(pool) = &self.event_log_pool else {
           // No DB pool wired — happens in unit tests built with
           // --features postgres. Mirror the non-postgres path so
           // realized_loss reflects in-memory closed positions.
           return self.load_monthly_state_in_memory(now).await;
       };
       // ... existing DB query unchanged ...
   }
   ```

3. Update the `cfg(not(feature = "postgres"))` arm to delegate to the
   same helper, removing the duplicated body.

**Lines changed**: ~25 (helper + two call sites).
**Files changed**: 1 (`position_manager.rs`).
**Production behavior**: Unchanged. In production, the pool is always
`Some`, the early-return branch never fires.
**Test behavior**: All 8 failing monthly_halt tests should pass under
`--features postgres`, matching their already-passing behavior under
the lib-only invocation.

### Why this is the minimal correct fix

- It does not change any data path that runs in production.
- It does not require schema migration, port additions, or new
  dependencies.
- It documents the intent in the existing function rather than scattering
  the assumption across test setup.
- It removes 12 lines of duplication between the two feature arms.
- It does not touch TD-2026-05-05-001 production logic.

### Alternative paths considered (and rejected)

| Alternative | Rejected because |
|---|---|
| Provision a real PG container for every unit test | Heavy and slow; the suite already separates `--lib` (no DB) from `#[sqlx::test] --ignored` (DB). The unit suite is supposed to run without DB. |
| Inject a mock `event_log_pool` in every `create_test_manager` call | Invasive — touches dozens of call sites and forces every test to know about a feature flag they shouldn't care about. |
| `#[ignore]` the failing tests under `--features postgres` | Hides a real signal. Slice 4 will rely on these tests as a regression gate. |
| Refactor `MonthlyRiskState` ownership end-to-end | Out of scope; the current design works correctly in production. |

---

## Recommendation

**Apply the proposed fix in a single commit on the current branch
(`fix/td-2026-05-05-001-core-lifecycle-drift`)** before advancing to
Slice 3 of TD-2026-05-05-001. Suggested commit message:

```
fix(robsond): fall back to in-memory monthly state when pg pool is unwired

When --features postgres is active but event_log_pool is None (the
default in create_test_manager), load_monthly_state was returning
realized_loss = 0 instead of reading the in-memory MemoryStore. This
silently masked any closed_in_month positions in 8 monthly_halt
unit tests, producing a chronic 8/13 failure rate on
`cargo test -p robsond --tests --features postgres`.

Production wires the pool unconditionally; the broken branch is
unreachable in deployed builds. The fix mirrors the existing
cfg(not(feature = "postgres")) path: when the pool is None,
aggregate realized_loss from the store via find_closed_in_month.

Resolves TD-2026-05-08-002. Restores the validation gate ahead of
TD-2026-05-05-001 Slice 3+.
```

The implementation guide for TD-2026-05-05-001 should record that
Slice 3 was preceded by this fix and that the validation gate is now
trusted.

---

## Test plan for the fix

After applying the fix, the following must all be green:

```bash
cd /home/psyctl/apps/robson/v3
cargo test -p robsond --lib monthly_halt        # already green; must stay green
cargo test -p robsond --tests --features postgres monthly_halt
cargo test -p robsond --tests --features postgres
cargo test -p robsond --lib                      # full lib suite
```

Specifically, the 13 monthly_halt tests must pass under both invocations
(no flag and `--features postgres`), with identical results.

The Slice 0 reconciliation_worker canary must still pass:

```bash
cargo test -p robsond --lib reconciliation_worker
```

`rustfmt --check` on `position_manager.rs` must show no diffs in the
edited region (pre-existing nightly-rustfmt diffs at line 1007 are out
of scope and unrelated).

---

## Changelog

| Date | Change | Author |
|---|---|---|
| 2026-05-08 | Initial diagnostic (this document). | Claude Opus 4.7 |
