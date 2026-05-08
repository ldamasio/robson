# TD-2026-05-05-001 — Core Position Lifecycle Drift

**Status**: Draft (Slice 0 in progress)
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

### Slice 0 — Reproduction & analysis (no production code) — IN PROGRESS

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

### Slice 1 — Domain: ClosureEvidence + ExitReason

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

### Slice 2 — Policy I3 in documentation

- Update `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md`: add §I3,
  Active-only scope, evidence ordering, startup gate policy.
- Update `docs/adr/ADR-0022-robson-authored-position-invariant.md`:
  changelog entry referencing I3.
- Create `docs/runbooks/td-2026-05-05-001-stale-active-recovery.md`
  (skeleton; content finalized in Slice 5).
- No code.
- **Commit**: `docs(policies): add I3 reverse reconciliation invariant for TD-2026-05-05-001`

### Slice 3 — Evidence pipeline: `ExchangePort` extension

- Extend `ExchangePort` with two new methods:
  - `async fn get_order_by_exchange_id(symbol, order_id) -> Result<Option<OrderResult>, ExecError>`
  - `async fn get_user_trades_since(symbol, since: DateTime<Utc>, limit: u16) -> Result<Vec<UserTradeRecord>, ExecError>`
- Implement in `BinanceExchangeAdapter` using existing
  `BinanceRestClient::get_order_status` and a new
  `get_user_trades` wrapper around `GET /fapi/v1/userTrades`.
- Implement no-op variants in `StubExchange` plus a `set_user_trades` helper
  for tests.
- **Tests**: stub round-trips, Binance adapter contract tests
  (`#[ignore]`-gated against testnet credentials).
- **Commit**: `feat(exec): add evidence retrieval methods to ExchangePort`

### Slice 4 — `PositionManager::reconcile_close` + symmetric worker loop

- New `pub(crate) async fn reconcile_close(&self, position_id,
  evidence: ReconciliationEvidence) -> DaemonResult<()>` in
  `position_manager.rs`. Idempotent: returns Ok if already terminal.
- New helper `gather_evidence(position) -> Result<ReconciliationEvidence,
  EvidenceError>` that walks sources 1→2→3→4.
- Symmetric loop in `ReconciliationWorker::scan_and_reconcile`:
  1. Build `exchange_set: HashSet<(Symbol, Side)>` from
     `get_all_open_positions()` (existing).
  2. Iterate `store.positions().find_active().await?` (NEW). For each,
     match `(symbol, side)`:
     - Match → no action.
     - No match AND `position.state == Active` → push to candidate set
       with first-observation timestamp from a per-position grace map.
     - No match AND `position.state in {Entering, Exiting}` → emit
       `ReconciliationStaleNonActiveDetected` event, skip.
  3. Re-check candidates from the previous cycle. If still missing AND
     `now - first_seen >= grace_secs` → call `reconcile_close` with
     gathered evidence.
- Grace state lives inside `ReconciliationWorker` (in-memory map
  `position_id → first_seen_at`); cleared when the position reappears or
  is closed. Lost on restart — startup gate handles startup-time
  stale-active separately.
- **Tests** (in this slice):
  1. Stale-Active in store + empty exchange + 2 cycles past grace →
     reconciled close emitted with `Reconciled(AccountSnapshot)` evidence.
  2. Idempotency — running `scan_and_reconcile` twice after close emits
     nothing further.
  3. Grace race — exchange empty for 1 cycle then full for 1 cycle → no
     close.
  4. Cross-side false-positive — Long Active and Short Active for same
     symbol, exchange has only Long → only Short is reconciled.
  5. Entering/Exiting → log + skip + non-Active event emitted.
  6. Evidence preference — when `OrderFillRecord` is available, the close
     event carries it (not AccountSnapshot).
- **Commit**: `feat(robsond): symmetric reconciliation closes stale Active positions`

### Slice 5 — Startup gate + runbook

- Implement Path A (default `abort`) and Path B (`auto_reconcile`) per
  Amendment §3.
- New config field `reconciliation.on_startup_stale_active`.
- Runbook content in
  `docs/runbooks/td-2026-05-05-001-stale-active-recovery.md`.
- New CLI subcommand `robson-cli reconcile-close` for operator-driven
  closes (Path A path).
- **Tests**: integration (`#[sqlx::test]`) covering both paths.
- **Commit**: `feat(robsond): startup gate and operator runbook for stale-active drift`

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
| Reverse reconciliation closes stale Active after grace | 4 | Unit |
| Reverse reconciliation idempotent | 4 | Unit |
| Grace race does not close prematurely | 4 | Unit |
| Cross-side (Long/Short same symbol) false-positive guard | 4 | Unit |
| Entering/Exiting → log + skip, no close | 4 | Unit |
| Evidence preference order (Fill > Trade > Snapshot > Estimated) | 4 | Unit |
| Estimated evidence triggers CRITICAL alert and metric | 4 | Unit |
| MonthlyHalt evaluated after reconciled close | 4 | Unit |
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
