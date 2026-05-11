# TD-2026-05-05-001 — Core Position Lifecycle Drift

**Status**: In progress — Slices 0/1/2/3/4A/4B/5A/5B1/hotfix-docker/5B2A done; 5B2B/5B2C planned
**Severity**: High
**Area**: `robsond` reconciliation, position lifecycle
**Discovered**: 2026-05-05
**Companion ADR**: [ADR-0022 — Robson-Authored Position Invariant](../adr/ADR-0022-robson-authored-position-invariant.md)
**Companion Policy**: [UNTRACKED-POSITION-RECONCILIATION.md](../policies/UNTRACKED-POSITION-RECONCILIATION.md)
**Technical Debt Entry**: `docs/technical-debt.md` → `TD-2026-05-05-001`

---

## Objective

Close the asymmetric reconciliation gap that allows a `robsond`-tracked core
position to remain in `Active` state in the local store after the corresponding
exchange position has disappeared (liquidation, manual close, fill of an
externally-resident insurance stop, partial liquidation). Restore the symmetry
between local lifecycle state and exchange reality, with provenance-grade
evidence for every reconciled close.

This guide does **not** change entry-side behavior, does **not** alter the
existing UNTRACKED close path, and is scoped strictly to Robson v3 (no v4/v5
work).

---

## Constraints

- **Capital is real.** Production runs against the operated Binance Futures
  account. Every change must fail closed and be append-then-apply via the
  existing eventlog → projector path (MIG-v2.5#2).
- **No invented PnL.** A reconciled close must carry exchange evidence; if
  evidence is unavailable, the close is recorded as an *estimated* terminal
  event with explicit provenance, never as a real fill.
- **Idempotency.** Reconciliation runs every 60s and at startup; every code
  path must be safe to re-run without producing duplicate events or duplicate
  PnL.
- **Symbol-agnostic.** Per ADR-0023, no special-casing of `BTCUSDT` or any
  other pair.
- **No bypass of governance gates.** Closing a position is always permitted
  (per UNTRACKED policy §Enforcement); but the close MUST emit full audit
  events.
- **Append-then-apply** order in `Store::apply_event` must be preserved.
- **Reverse reconciliation applies only to `Active`** (see Amendment §1).

---

## Non-Goals (out of scope for this TD)

- Auto-closing `Entering` positions whose exchange fill never materialized.
  Detection + structured log + skip is in scope; auto-close is a follow-up TD.
- Auto-closing `Exiting` positions whose exit order disappeared from the
  exchange. Same treatment.
- Detecting **divergent** quantity (Robson sees 0.010, exchange sees 0.005).
  Out of scope here; covered by `ReconciliationEvent` flow per the existing
  policy. A follow-up TD will address quantity drift.
- Adding a panic-suspend endpoint (`POST /reconciliation/suspend`). Mentioned
  in the policy as a v3 target; deferred.
- Cross-account or per-tenant credentials. Single account assumed.
- Frontend changes. UI already consumes `/status` and `/positions/:id` and
  will reflect the new state automatically once the eventlog converges.

---

## Amendments to the original conceptual plan

These three amendments are mandatory and override anything in the original
proposal.

### Amendment 1 — `I3` is `Active`-only

The new invariant in `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md` is:

> **I3 — Robson-Active ⊆ Exchange-Open.** Toda posição local em estado
> `Active` deve possuir posição correspondente na exchange, considerando
> `symbol`, `side` e tolerância de `quantity`. Se a posição estiver ausente
> após grace period e segunda observação consecutiva, o Robson DEVE registrar
> evidência e transitar a posição local para `Closed` por reconciliação.

Detection still applies to `Entering` and `Exiting` — but the worker MUST log
a structured warning and skip them (no auto-close). Skipped states emit
`DaemonEvent::ReconciliationStaleNonActiveDetected { position_id, state,
symbol, side, observed_at }` so the operator alerting layer can flag them.

Rationale: `Entering` positions might have a real exchange order that is
queued or being placed (placement→fill latency). `Exiting` positions are
already mid-flight to terminal state via the legitimate path. Auto-closing
either risks double-close or hides a real entry-side bug.

### Amendment 2 — Evidence ordering, no silent fallback

A reconciled close MUST attach an explicit `ClosureEvidence` payload.
Evidence sources, in strict priority order:

1. **`OrderFillRecord`** — A specific exchange order (typically the
   `insurance_stop_id` that was resident on the exchange, but also the
   originating entry order if it was reverse-traded) was filled. Source:
   `GET /fapi/v1/order` by exchange `orderId`. Fields captured: fill price,
   filled quantity, fee, fee asset, filled timestamp, exchange order id.

2. **`UserTradeRecord`** — Per-symbol user trades within a window covering
   the gap between last-known-active and the missing observation. Source:
   `GET /fapi/v1/userTrades`. Used when no specific order id is known
   (e.g., manual close on UI placed a separate market order). Captures the
   same fields as `OrderFillRecord` plus the originating order id.

3. **`AccountSnapshot`** — Confirms the position is zeroed without supplying
   a fill price. Source: the same `get_all_open_positions` call that
   triggered detection (the position is absent from the response). Captures:
   observation timestamp, two consecutive snapshots proving zero, account
   balance delta if available via `get_futures_balance`. Used when no fill
   record can be retrieved (rate limit, history outside the API window,
   etc.).

4. **`Estimated`** — Last resort. Captures: timestamp, the conservative
   exit price chosen, an explicit `estimation_basis` enum
   (`TrailingStopAtDetection`, `ExchangeMarkPrice`, `LastObservedPrice`).
   Realized PnL computed from this evidence MUST be flagged as
   `pnl_provenance: Estimated` in the event payload, never as `RealFill`.

The `PositionClosed` event gains a new field `closure_evidence:
ClosureEvidence` (default `RealFill { source: ExitOrderFill }` for the
existing exit path — backward-compatible because all existing callers
already supply real fill data). New variant `ClosureEvidence::Reconciled`
carries the four sub-cases above.

If a reconciliation pass would have to use `Estimated`, the daemon emits
`CRITICAL` alert and the operator runbook (see Amendment §3) is invoked
before the close is finalized. Estimated closes are always permitted — the
alternative (leaving an Active stale forever) is worse — but they are
visibly flagged in the audit trail and Prometheus
(`robson_reconciliation_estimated_closes_total`).

### Amendment 3 — Startup gate policy

When `restart→reconcile` finds at least one `stale_active` (Robson `Active`,
exchange empty) the daemon MUST take one of two paths, controlled by a
**single config knob**:

```
[reconciliation]
on_startup_stale_active = "auto_reconcile" | "abort"
# Default: "abort". Operator opt-in flips to "auto_reconcile".
```

#### Path A — `abort` (default, fail closed)

- Daemon refuses to enter the normal control loop.
- Logs `CRITICAL` with the list of `(position_id, symbol, side, since)`.
- Exit code 78 (`EX_CONFIG`, distinct from existing exit codes).
- Runbook: `docs/runbooks/td-2026-05-05-001-stale-active-recovery.md`
  (created in Slice 5). Operator MUST:
  1. Confirm exchange state via `binance-cli` or web UI.
  2. Decide the evidence source (preferred fill record, user trades,
     snapshot, or estimated).
  3. Run `robson-cli reconcile-close --position-id <UUID> --evidence
     <SOURCE> [--exit-price <DEC>]`. CLI emits the same event the worker
     would have emitted, with operator identity in `evidence.evaluator`.
  4. Restart daemon. Startup gate now passes.

#### Path B — `auto_reconcile` (opt-in)

- Daemon runs an explicit `startup_reverse_reconciliation()` phase between
  step 3a (`scan_and_reconcile_blocking` for UNTRACKED) and step 3b
  (`startup_recovery` for missed-stop catch-up).
- For each stale-Active, attempt evidence sources 1→2→3 in order.
- If only `Estimated` is available, abort startup (downgrade to Path A
  semantics for that specific position) — never auto-close on Estimated.
- Emit `position_closed` per success; refuse to start if any stale-Active
  remains unresolved after the phase.

Both paths share the same evidence pipeline; only the trigger differs
(automatic vs operator-driven).

The default is **`abort`** per the user's "fail closed" requirement. The
operator opts in to `auto_reconcile` only after the runbook has been
exercised at least once and the alerting/observability surface has been
verified in production.

---

## Files Inspected

Slice 0 only reads existing code. No production files are modified.

- `v3/robsond/src/reconciliation_worker.rs` — current asymmetric scanner.
- `v3/robsond/src/position_manager.rs:2470-2557` (`handle_exit_fill`),
  `:2682-2776` (`panic_close_position_internal`), `:2823-2860`
  (`compute_slots_available`).
- `v3/robsond/src/daemon.rs:480-518` — startup sequence: rebuild → restore
  → UNTRACKED scan → recovery → workers.
- `v3/robsond/src/startup_recovery.rs` — missed-stop catch-up via candle
  replay (does NOT cover stale-Active).
- `v3/robsond/src/api.rs:614-692` — `/status`, `/month` slot accounting.
- `v3/robson-domain/src/entities.rs:240-306` (`PositionState`),
  `:347-361` (`ExitReason`).
- `v3/robson-domain/src/events.rs:240-306` (`ExitTriggered`,
  `ExitOrderPlaced`, `ExitFilled`, `PositionClosed`).
- `v3/robson-exec/src/ports.rs:108-124` (`get_all_open_positions`,
  `close_position_market`). NOTE: no port method to fetch order/trade
  history today.
- `v3/robson-connectors/src/binance_rest.rs:427-440` (`get_order_status`).
  Already at the connector layer; needs to be exposed at the
  `ExchangePort` trait level for the evidence pipeline.
- `v3/robson-store/src/repository.rs:35-90` (`find_active`, `find_risk_open`,
  `find_active_by_symbol_and_side`, `find_closed_in_month`).
- `v3/robson-store/src/postgres.rs:145-150` (ExitReason parser),
  `:407` (`find_active_from_projection`).
- `v3/robson-projector/src/handlers/positions.rs` — `handle_position_closed`
  path that the new evidence-bearing event MUST flow through.
- `v3/robson-projector/src/handlers/monthly_state.rs` —
  `handle_position_closed_monthly` path. Realized loss must be updated by
  reconciled closes too.
- `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md` — current policy
  (I1, I2). Will receive I3.

---

## Slice plan

Each slice is one PR, mergeable, must leave the workspace clean and tests
green. Slices 0 → 6 below; Slice 0 is the only one being executed in this
session.

### Slice 0 — Reproduction & analysis (no production code) — DONE (2026-05-08, commit `28b7a58e`)

- **Goal**: deterministic test that demonstrates current asymmetric
  reconciliation as a noop for stale-Active. Lock in the diagnostic.
- **Files added/changed**:
  - `v3/robsond/src/reconciliation_worker.rs` — append a unit test
    `test_reconciliation_does_not_close_active_missing_on_exchange`
    inside the existing `mod tests`. Test asserts the **current (buggy)
    behavior**: store keeps the position Active, exchange returns empty,
    worker reports 0 reconciled, store still shows the position as Active
    afterward. The test has a header comment marking it as a Slice 0
    baseline canary, to be inverted in Slice 4 when the symmetric loop
    ships.
  - `docs/analysis/2026-05-08-lifecycle-drift-repro.md` — analysis doc
    using `docs/templates/analysis-document-template.md`.
  - `docs/implementation/TD-2026-05-05-001-CORE-LIFECYCLE-DRIFT.md` (this
    file).
- **No production code** in this slice. No changes to `daemon.rs`,
  `position_manager.rs`, `ports.rs`, `entities.rs`, or `events.rs`.
- **Verification**:
  - `cargo fmt --all --check` (workspace `v3/`).
  - `cargo test -p robsond reconciliation_worker --lib` — new test passes.
  - `cargo clippy -p robsond --lib -- -D warnings`.
- **Rollback**: `git restore` the three files; nothing else touched.
- **Branch**: `fix/td-2026-05-05-001-core-lifecycle-drift`.
- **Commit (suggested)**:
  `test(robsond): reproduce TD-2026-05-05-001 stale-active drift baseline`

### Slice 1 — Domain: ClosureEvidence + ExitReason — DONE (2026-05-08, commit `fbdc8f0e`)

Outcome: 6 files changed (+500 / −5). New domain types `ClosureEvidence`,
`ReconciliationEvidence`, `RealFillEvidence`, `OrderFillEvidence`,
`UserTradeEvidence`, `AccountSnapshotEvidence`, `EstimatedEvidence`,
`ExitOrderFillSource`, `EstimationBasis`. `Event::PositionClosed` extended
with `closure_evidence: ClosureEvidence` (`#[serde(default)]` for
retro-compat — legacy events deserialize to `RealFill` default). Postgres
parser maps the new `ReconciledMissingOnExchange` variant in both
PascalCase and snake_case. 117 + 37 + 179 + 4 + 3 tests green; Slice 0
canary unchanged.

- Add `ExitReason::ReconciledMissingOnExchange`.
- Add `ClosureEvidence` enum in `robson-domain/src/entities.rs` (or
  `events.rs`):
  ```rust
  pub enum ClosureEvidence {
      RealFill { source: ExitOrderFillSource, exchange_order_id: Option<String> },
      Reconciled(ReconciliationEvidence),
  }
  pub enum ReconciliationEvidence {
      OrderFillRecord(OrderFillEvidence),
      UserTradeRecord(UserTradeEvidence),
      AccountSnapshot(AccountSnapshotEvidence),
      Estimated(EstimatedEvidence),
  }
  ```
- Extend `Event::PositionClosed` with `closure_evidence: ClosureEvidence`.
  Backward-compat default: `RealFill { source: ExitOrderFillSource::ExitFill,
  exchange_order_id: None }`.
- Map enum strings in `robson-store/src/postgres.rs:145-150`.
- No behavior change — only vocabulary expansion. All existing callsites
  populate `closure_evidence` with the `RealFill` default.
- **Tests**: round-trip serialization, projector backfill, parser fallback.
- **Commit**: `feat(domain): add ClosureEvidence and ReconciledMissingOnExchange ExitReason`

### Slice 2 — Policy I3 in documentation — DONE (2026-05-08, awaiting commit)

Outcome:

- `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md` gained §I3 with the
  Active-only scope, grace-period + second-observation detection rule,
  evidence ordering (`OrderFillRecord` → `UserTradeRecord` →
  `AccountSnapshot` → `Estimated`), and the `Estimated`-never-silent rule
  (CRITICAL alert + Prometheus counter + startup-gate downgrade to
  `abort`).
- `docs/adr/ADR-0022-robson-authored-position-invariant.md` carries a
  2026-05-08 amendment recording I3 as the symmetric counterpart of
  I1/I2 and pointing to the policy + implementation guide + runbook.
- `docs/runbooks/td-2026-05-05-001-stale-active-recovery.md` exists as a
  policy-and-decision-flow skeleton. Sections present: Symptoms, Safety
  Principle, Preconditions, Evidence Collection Order (all four
  sources), Manual Verification Checklist, Recovery Command (placeholder
  for Slice 5), Post-Recovery Validation, Rollback / When to Stop. The
  `robson-cli reconcile-close` command is explicitly marked as not yet
  implemented.

- Update `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md`: add §I3,
  Active-only scope, evidence ordering, startup gate policy.
- Update `docs/adr/ADR-0022-robson-authored-position-invariant.md`:
  changelog entry referencing I3.
- Create `docs/runbooks/td-2026-05-05-001-stale-active-recovery.md`
  (skeleton; content finalized in Slice 5).
- No code.
- **Commit**: `docs(policies): add I3 reverse reconciliation invariant for TD-2026-05-05-001`

### Slice 3 — Evidence pipeline: `ExchangePort` extension — DONE (commit `0835150b`)

- Extended `ExchangePort` with two new methods:
  - `async fn get_order_by_exchange_id(symbol, order_id) -> Result<Option<OrderResult>, ExecError>`
  - `async fn get_user_trades_since(symbol, since: DateTime<Utc>, limit: u16) -> Result<Vec<UserTradeRecord>, ExecError>`
- Implemented in `BinanceExchangeAdapter` using existing
  `BinanceRestClient::get_order_status` and a new
  `get_user_trades` wrapper around `GET /fapi/v1/userTrades`.
- Implemented no-op variants in `StubExchange` plus a `set_user_trades` helper
  for tests.
- **Tests**: stub round-trips, Binance adapter contract tests
  (`#[ignore]`-gated against testnet credentials).
- **Commit**: `feat(exec): add evidence retrieval methods to ExchangePort`

### Slice 4A — `PositionManager::reconcile_close` for real evidence — DONE (commit `93db6bb9`)

- Added `ReconciledCloseInput`, `ReconcileCloseOutcome`, and
  `PositionManager::reconcile_close`.
- `reconcile_close` accepts only real evidence:
  `OrderFillRecord` and `UserTradeRecord`.
- `AccountSnapshot` and `Estimated` are rejected and do not emit
  `PositionClosed`.
- Inconsistent real evidence is rejected before close.
- Terminal positions are idempotent state-first noops.
- `PositionClosed` uses `ExitReason::ReconciledMissingOnExchange` and
  `ClosureEvidence::Reconciled(...)`.
- **Commit**: `feat(robsond): add reconciled close path for real evidence`

### Slice 4B — Symmetric stale-Active worker loop — DONE (commit `2a87fb8e`)

- Added the Robson Active → Exchange Open symmetric loop to
  `ReconciliationWorker`.
- Matching is by `(symbol, side)`, never by symbol alone.
- Grace state lives in memory inside `ReconciliationWorker`; first
  observation only records state, and close is attempted only after a second
  observation past grace.
- `Active`-only auto-close calls `PositionManager::reconcile_close` only
  when real evidence is available.
- `OrderFillRecord` has priority over `UserTradeRecord`.
- `UserTradeRecord` closes only when there is exactly one strong candidate:
  same symbol by query, `filled_at >= first_observed_missing_at`, and
  `filled_quantity == expected_quantity`. Zero candidates, multiple
  candidates, or quantity mismatch stay unresolved.
- `Entering` and `Exiting` are detected, logged, emitted as
  `ReconciliationStaleNonActiveDetected`, and skipped.
- Stale Active with no unambiguous real evidence emits
  `ReconciliationStaleActiveUnresolved`, remains `Active`, and does not call
  `reconcile_close`.
- **4B limitation accepted for safety**: `UserTradeRecord` lookup starts at
  `first_observed_missing_at`. If a manual close or liquidation occurred
  immediately before the first Robson observation, the real trade can fall
  outside the lookup window and the case can become `unresolved`. Slice 4B
  intentionally does not add automatic lookback to avoid associating an
  unrelated trade with a local position.
- **Commit**: `feat(robsond): add symmetric stale-active reconciliation loop`

### Slice 5A — Startup gate (abort path) + config + docs — DONE (2026-05-09)

Outcome:

- `ReconciliationConfig` extended with two new fields:
  - `missing_grace_secs: u64` (default 60) — governs the periodic worker;
    parsed from `ROBSON_RECONCILIATION_MISSING_GRACE_SECS`.
  - `on_startup_stale_active: StartupStaleActivePolicy` (default `Abort`) —
    parsed from `ROBSON_RECONCILIATION_ON_STARTUP_STALE_ACTIVE`. Only `abort`
    accepted in 5A; unknown values produce a config error.
- New `DaemonError::StartupStaleActiveDetected { count, positions }` variant.
- New `StartupStaleActiveInfo` struct carrying `position_id`, `symbol`, `side`,
  `quantity`, optional `entry_price` for structured log payload.
- `Daemon::run()` startup sequence updated:
  1. After `restore_positions()` and before `scan_and_reconcile_blocking()`,
     calls `run_startup_stale_active_gate()`.
  2. Gate checks local `Active` positions against exchange immediately (no
     grace). `Entering` / `Exiting` are ignored.
  3. If any stale-Active found: emits CRITICAL log per position, returns
     `StartupStaleActiveDetected`.
  4. `map_daemon_result` in `main.rs` converts this to `std::process::exit(78)`.
- `ReconciliationWorker` instantiation now always uses explicit
  `missing_grace_secs` from config (no longer implicitly equal to
  `scan_interval`).
- `ReconciliationWorker::new_with_missing_grace` promoted to `pub(crate)`.
- Runbook `docs/runbooks/td-2026-05-05-001-stale-active-recovery.md` updated
  with Slice 5A operational content (abort flow, exit code, immediate actions).

Scope of 5A (fail-closed only):
- Exit code 78 on stale-Active detection ✅
- Structured log per position ✅
- Explicit `missing_grace_secs` in worker ✅
- `abort` policy enforced by default ✅
- Runbook updated ✅

Not in 5A (deferred to 5B):
- `auto_reconcile` policy (Path B)
- `robson-cli reconcile-close` command
- Evidence-driven auto-close at startup

Tests added (210 lib tests passing):
- `config::test_reconciliation_config_defaults` — `missing_grace_secs == 60`,
  `on_startup_stale_active == Abort`
- `config::test_load_missing_grace_secs_from_env`
- `config::test_load_startup_policy_abort_from_env`
- `config::test_load_startup_policy_unknown_is_config_error`
- `error::test_startup_stale_active_exit_code_is_78`
- `daemon::test_startup_gate_clean_no_positions`
- `daemon::test_startup_gate_stale_active_returns_typed_error`
- `daemon::test_startup_gate_stale_active_does_not_mutate_store`
- `daemon::test_startup_gate_entering_does_not_abort`
- `daemon::test_startup_gate_exiting_does_not_abort`

- **Commit**: `feat(robsond): startup gate abort path and config for stale-active drift (Slice 5A)`

### Slice 5B1 — `robson-cli reconcile-close` (operator-driven manual path) — DONE (2026-05-09, PR #60)

Outcome:

- New crate `v3/robson-cli` with `reconcile-close` subcommand.
  Sends `POST /reconcile-close` to `robsond`; validates evidence shape
  locally before the network call.
- New `POST /reconcile-close` API endpoint in `robsond`.
  Validates Bearer token, deserializes evidence, calls
  `PositionManager::reconcile_close()`. Returns `realized_pnl` and
  `exit_price` on success.
- Only `OrderFillRecord` and `UserTradeRecord` evidence accepted.
  `AccountSnapshot` and `Estimated` are rejected at both CLI and API level
  (design decision: unambiguous real evidence only for operator-driven path).
- Exit codes 0–6 documented in the runbook.
- Runbook `docs/runbooks/td-2026-05-05-001-stale-active-recovery.md`
  updated with sample commands and evidence JSON.

Tests added (via `test(robsond): add /reconcile-close API tests`):
API validation, evidence rejection, position-not-found (404), position-not-active
(409), and evidence-inconsistent (400) paths covered.

Scoped PR: #60.

### Hotfix — Docker `robson-cli` workspace member — DONE (2026-05-09, PR #61)

Outcome:

- `v3/Dockerfile` updated to include `robson-cli` in the workspace build so
  the `robson-cli` binary is present inside the `robsond` container image.
- No algorithm change. Build-system fix only.
- Commit `1b275638 fix(docker): include robson-cli workspace member in robsond build`.

### Slice 5B2A — Evidence helper refactor in `reconciliation_worker.rs` — DONE (2026-05-11)

Outcome:

- Extracted shared evidence helper functions in
  `v3/robsond/src/reconciliation_worker.rs` into dedicated private helpers.
- Simplified the user-trade evidence helper (deduplication, cleaner logic).
- **No behavior change.** Pure refactor — all existing tests continue passing.
- Commits:
  - `20283d9e refactor(robsond): extract reconciliation evidence helpers`
  - `26e82837 refactor(robsond): simplify user trade evidence helper`

5B2A is the preparatory refactor that makes `reconciliation_worker.rs` ready
to receive the `auto_reconcile` startup logic in Slice 5B2B without conflating
the refactor with the behavior change.

---

#### 5B2 Architectural Decision — Sub-slice breakdown and auto_reconcile algorithm

5B is broken into three sub-slices:

| Sub-slice | Scope | Status |
|---|---|---|
| 5B2A | Refactor evidence helpers — no behavior change | DONE |
| 5B2B | Config + startup `auto_reconcile` opt-in algorithm | PLANNED |
| 5B2C | Docs/runbook/testnet drill | PLANNED |

**Startup `auto_reconcile` algorithm (for 5B2B) — two-phase / all-or-nothing:**

1. Detect all stale-Active positions (same gate as the 5A abort path).
2. Collect and validate real evidence for each position (`OrderFillRecord` or
   `UserTradeRecord`) — no partial or estimated substitution.
3. If any position lacks real unambiguous evidence → abort startup with exit
   code 78 (same as `abort` policy). No position is closed.
4. If all positions have real evidence → call `reconcile_close` for each.

**Policy decisions for auto_reconcile (5B2B):**

- `abort` remains the **default** and the safe baseline.
- `auto_reconcile` is **opt-in** via
  `ROBSON_RECONCILIATION_ON_STARTUP_STALE_ACTIVE = "auto_reconcile"`.
- `auto_reconcile` may only auto-close using `OrderFillRecord` or
  `UserTradeRecord`. No `AccountSnapshot` auto-close at startup.
- `Estimated` evidence at startup always downgrades to `abort` behavior —
  never auto-closes.
- No partial closes: if any stale-Active lacks evidence, the entire startup
  is aborted without closing any position.

### Slice 5B2B — Startup `auto_reconcile` algorithm + config — PLANNED

- Implement two-phase all-or-nothing `startup_reverse_reconciliation()`.
- Accept `auto_reconcile` in `StartupStaleActivePolicy` (currently only
  `abort` is parsed; unknown values produce a config error).
- Add integration tests covering the auto_reconcile happy path, the
  Estimated-downgrade path, and the partial-evidence abort path.
- **Commit (suggested)**: `feat(robsond): startup auto-reconcile two-phase path (Slice 5B2B)`

### Slice 5B2C — Runbook finalization + testnet drill — PLANNED

- Finalize `docs/runbooks/td-2026-05-05-001-stale-active-recovery.md` with
  the Path C (auto_reconcile) section and a drill checklist.
- Execute one testnet drill (stale-Active seeded manually → daemon starts
  with `auto_reconcile` → validates close outcome).
- Only after testnet drill passes consider enabling `auto_reconcile` in
  production config.
- **Commit (suggested)**: `docs(runbooks): add auto_reconcile path and testnet drill checklist`

### Slice 6 — Slot/monthly accounting regression coverage

- No production code changes expected. `compute_slots_available`,
  `/status`, and `/month` already derive from the projection — once the
  reconciled close lands in the eventlog, all downstream views
  recompute correctly.
- E2E test (`#[sqlx::test]`):
  1. Seed Active position.
  2. Exchange returns empty.
  3. Reconciliation worker closes with evidence after grace.
  4. Assert `/status.occupied_slots` decremented, `/positions/:id` returns
     `state: closed`, monthly_state.realized_loss reflects the loss.
- **Commit**: `test(robsond): regression coverage for slot reclamation post-reconcile`

---

## Mandatory tests (consolidated)

All asserted via `cargo test -p robsond` plus targeted Postgres tests under
`scripts/test-pg.sh`.

| Test | Slice | Type |
|---|---|---|
| Stale-Active baseline (current bug — passes in Slice 0, inverted in Slice 4) | 0 | Unit |
| ClosureEvidence/ExitReason round-trip serialization | 1 | Unit |
| Postgres parser fallback for new `ExitReason` value | 1 | Postgres integration |
| `get_order_by_exchange_id` happy path on stub | 3 | Unit |
| `get_user_trades_since` returns ordered trades | 3 | Unit |
| `reconcile_close` closes Long/Short with real `OrderFillRecord` / `UserTradeRecord` evidence only | 4A | Unit |
| `reconcile_close` rejects `AccountSnapshot`, `Estimated`, and inconsistent real evidence | 4A | Unit |
| Reverse reconciliation first observation does not close | 4B | Unit |
| Reverse reconciliation closes stale Active after grace when `OrderFillRecord` exists | 4B | Unit |
| Reverse reconciliation closes stale Active after grace with exactly one strong `UserTradeRecord` candidate | 4B | Unit |
| Reverse reconciliation idempotent | 4B | Unit |
| Grace race does not close prematurely | 4B | Unit |
| Cross-side (Long/Short same symbol) false-positive guard | 4B | Unit |
| Entering/Exiting → log + skip, no close | 4B | Unit |
| `UserTradeRecord` zero/multiple/quantity-mismatch candidates stay unresolved | 4B | Unit |
| Snapshot/Estimated evidence does not auto-close stale Active | 4B | Unit |
| MonthlyHalt evaluated after reconciled close | 4A | Unit |
| Startup gate Path A aborts with exit code 78 | 5 | Integration |
| Startup gate Path B reconciles when evidence available | 5 | Integration |
| Startup gate Path B aborts when only Estimated evidence available | 5 | Integration |
| Slot reclamation E2E (Active → reconcile → /status decremented) | 6 | Postgres integration |
| Eventlog replay convergence after reconciled close | 6 | Postgres integration |

---

## Regression risks

| Risk | Mitigation |
|---|---|
| Fill latency between `place_market_order` and `get_all_open_positions` reflecting the new position causes false positives | Grace period ≥ 1 reconciliation cycle (60s default) + second consecutive observation; symmetric loop only operates on `Active` (not `Entering`) |
| Race with legitimate `Active → Exiting → Closed` already in flight | `Exiting` is excluded from auto-close; `find_active` includes Exiting but the candidate filter checks `position.state == Active` |
| Evidence retrieval fails (rate limit, network) → would force Estimated | Retry with exponential backoff inside `gather_evidence`; only fall through to next source after exhausting retries; Estimated requires explicit operator path or alert |
| New `ClosureEvidence` field breaks existing event consumers | All existing callsites populate `RealFill` default; projector handler is updated in Slice 1 to read the new field with `unwrap_or(default)` for legacy events; backfill migration not required |
| Concurrent `reconcile_close` for same position id | Reuse existing `entry_flow_lock` or add per-position `Mutex`; `reconcile_close` checks terminal state first and exits early when already Closed |
| Estimated closes inflate realized losses incorrectly | Estimated PnL is flagged in event payload and surfaced as `pnl_provenance: Estimated` in API; monthly_state uses the reported PnL as-is but the operator-facing dashboards mark estimated closes |
| Postgres projector falls behind eventlog and `/status` shows stale-Active even though `PositionClosed` was emitted (existing projection-bug class) | Slice 0 explicitly tests for this divergence; if observed, blocks Slice 4 until projection convergence is verified |
| Loss of in-memory grace state on restart | Startup gate (Slice 5) handles startup-time stale-active separately; in-flight grace timing only protects steady-state |

---

## Branch / commit naming

- **Branch (root)**: `fix/td-2026-05-05-001-core-lifecycle-drift`
- **Per-slice commits** (suggested, conventional commits, valid scopes per
  v3 CLAUDE.md):
  - Slice 0: `test(robsond): reproduce TD-2026-05-05-001 stale-active drift baseline`
  - Slice 1: `feat(domain): add ClosureEvidence and ReconciledMissingOnExchange ExitReason`
  - Slice 2: `docs(policies): add I3 reverse reconciliation invariant for TD-2026-05-05-001`
  - Slice 3: `feat(exec): add evidence retrieval methods to ExchangePort`
  - Slice 4: `feat(robsond): symmetric reconciliation closes stale Active positions`
  - Slice 5: `feat(robsond): startup gate and operator runbook for stale-active drift`
  - Slice 6: `test(robsond): regression coverage for slot reclamation post-reconcile`
- **PR title (final)**: `fix(robsond): close Robson-Active positions missing on exchange (TD-2026-05-05-001)`
- **PR body must reference**: TD-2026-05-05-001, ADR-0022, ADR-0023,
  `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md`, the runbook from
  Slice 5.

---

## Definition of done (whole TD)

- [ ] All slices merged; no open follow-ups in this TD.
- [ ] `docs/technical-debt.md` entry status flipped to **Resolved** with a
      pointer to this guide.
- [ ] Policy I3 effective; ADR-0022 changelog updated.
- [ ] Runbook exercised at least once on testnet (drill).
- [ ] Prometheus dashboards include
      `robson_reconciliation_stale_active_total` and
      `robson_reconciliation_estimated_closes_total`.
- [ ] One full production release cycle without a stale-Active CRITICAL
      alert that required operator intervention.

---

## Changelog

| Date | Change | Author |
|---|---|---|
| 2026-05-08 | Initial draft (Slice 0). Amendments §1, §2, §3 incorporated. | Claude Opus 4.7 |
| 2026-05-09 | Slice 5A done: startup gate abort path, config, exit code 78, runbook. | Claude Sonnet 4.6 |
| 2026-05-09 | Slice 5B1 done (PR #60): `robson-cli reconcile-close` + `POST /reconcile-close`. Operator-driven manual recovery live. | Claude Opus 4.7 |
| 2026-05-09 | Hotfix done (PR #61): Dockerfile includes `robson-cli` workspace member. | Claude Sonnet 4.6 |
| 2026-05-11 | Slice 5B2A done: evidence helper refactor in `reconciliation_worker.rs`. No behavior change. 5B2 architectural decision recorded (two-phase auto_reconcile algorithm, opt-in, all-or-nothing). 5B2B/5B2C marked PLANNED. | Claude Sonnet 4.6 |
