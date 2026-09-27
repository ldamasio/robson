# ADR-0022 — Robson-Authored Position Invariant

**Date**: 2026-04-18
**Last Amended**: 2026-09-27 (legacy PositionMonitor physical removal)
**Status**: DECIDED - PARTIALLY IMPLEMENTED, FOLLOW-UP REQUIRED
**Deciders**: RBX Systems (operator + architecture)

---

## Context

Robson operates a Binance account via API keys loaded into the `robsond` daemon.
Historically, nothing in the architecture prevented a position from existing on that
account without a matching entry event in `event_log`. Such a position could arise
from:

1. An operator placing a manual order via the Binance website, mobile app, or a side
   script using the same API keys.
2. A leaked or shared API key placing an order from elsewhere.
3. A code path in a legacy service (Django monolith) that wrote orders to the
   exchange without persisting exchange-ack authorship evidence.
4. A partial deploy / race where the executor placed the order but the event-log
   append failed silently.

Every such position silently consumes the account's exposure, margin, and risk
budget that the Risk Engine assumes is free — with cascading effects under leverage.
It also has no technical stop, no span, no governance trail, and is invisible to the
monthly drawdown calculation.

Manual account changes can also corrupt the current month's risk base even after
position-level reconciliation closes the offending position. If the Futures wallet
balance has fallen materially below the stored monthly `capital_base`, Robson must
recalibrate the current month's base before allowing new entries. ADR-0038 defines
that account-level reconciliation rule.

The existing reconciliation flow (`v3-runtime-spec.md` — Recovery Procedure Scenario
4) reconciles `RuntimeState` against the exchange and adopts the exchange state
when they diverge. This is insufficient: adopting an UNTRACKED position as truth
whitewashes a policy breach. The adoption path was designed for missed fills on
Robson-authored entries, not for foreign positions.

---

## Decision

Establish a non-negotiable invariant on Robson-operated Binance accounts:

> **Every open position on the operated Binance account MUST be the direct result of
> an entry authored by `robsond` through a `GovernedAction`. Any open position that
> is not traceable to a `robsond`-authored entry is UNTRACKED and MUST be closed.**

This invariant has two operational components:

### I1 — Authorship (enforced at write time)

Every order placed by `robsond` is placed only after the Risk Engine has produced a
`GovernedAction` token. Every current entry produces `entry_order_requested`
before exchange submission and `entry_order_accepted` after acknowledgement.
The accepted event carries both the `cycle_id` and exchange-assigned order id.
The older `entry_order_placed` event is replay-only legacy history and must not
be interpreted as an exchange acknowledgement.

This is **already guaranteed by QueryEngine** for orders originating in `robsond`.
The new requirement is to make the exchange-order-id ↔ event-log link queryable in
O(1) for the reconciliation worker (new index / projection).

### I2 — Reconciliation (enforced at read time)

A **Position Reconciliation Worker** runs periodically in the runtime. On each scan:

1. Query Binance for all open positions across all account types (spot, margin,
   USD-M Futures) and all symbols.
2. For each open position, look up the matching `entry_order_accepted` event in
   `event_log` by exchange order id.
3. If no matching event exists → classify the position as **UNTRACKED**.
4. Persist `position_untracked_detected`, alert the operator, and **close the
   position at market** via the reconciliation close path.
5. Persist `untracked_position_closed` on the resulting fill.

The close is mandatory and runs unconditionally. The former
`ROBSON_POSITION_MONITOR_ENABLED` application setting has been removed and never
gates reconciliation. Its false value remains in production infrastructure only
as a rollback guard for an older image. Tracked positions are managed separately
by `PositionManager` and the ADR-0039 exchange-side insurance stop.

### Current implementation boundary (2026-09-27)

The startup and periodic `ReconciliationWorker` are implemented for the current
USD-M Futures adapter and close exchange positions that have no matching active
local position. Current authorship matching is by `(symbol, side)`, not by the
originating exchange order id. Exact order-id correlation, every account type,
durable I2 detection/close events, the audited suspend endpoint, and complete
alerting remain target architecture. The current close path broadcasts
`RoguePositionDetected` and `SafetyExitExecuted`/`SafetyExitFailed` on the
in-process event bus and SSE surface, but does not persist the I2-required
`position_untracked_detected` or `untracked_position_closed` events.
The legacy fixed-percentage `PositionMonitor`, `DetectedPosition` model, storage
adapters, monitor configuration, frontend client surface, and `/safety/*` routes
are physically removed in the repository. Applied migration files and their
legacy tables remain inert schema history. Deployment of the removal image is
not yet operationally verified.

### Scope

Applies to every Binance account whose credentials are configured for `robsond`:
both `robson-testnet` and `robson` (production). Applies to every symbol, without
exception (see [ADR-0023](ADR-0023-symbol-agnostic-policy-invariant.md)).

### Rejected Alternatives

- **Trust the exchange state and adopt UNTRACKED positions into the Runtime.**
  Rejected — whitewashes policy breaches and produces retroactive "governance" for
  trades that never passed risk evaluation. This is fraud-shaped even if the
  operator is the one who placed the order.
- **Advisory-only detection (alert, do not close).** Rejected — leaves the operator
  with an open policy-violating position while they decide what to do. Under
  leverage, seconds matter.
- **Gate the reconciliation worker behind `ROBSON_POSITION_MONITOR_ENABLED`.**
  Rejected — this former legacy setting has been removed from the application.
  An UNTRACKED position is a policy violation that must be closed
  unconditionally.
- **Single-user honor system.** Rejected — Robson must be architecturally correct
  against its own operator, not just against third parties. A single rushed manual
  order can destroy weeks of compounded gains.

---

## Consequences

### Positive

- Target state makes the Risk Engine's guarantees end-to-end: no shadow
  positions outside its scope. Current Futures `(symbol, side)` matching is a
  partial enforcement step, not proof of origin.
- Target state closes the audit trail: every open position has a matching
  governance event once exact order-id correlation and durable I2 events land.
- Reconciliation becomes proactive rather than passive adoption.
- Target failure mode for leaked / shared API keys: an attacker's position is
  identified by origin and closed within one reconciliation interval. Current
  automatic coverage is limited to USD-M Futures and can miss a foreign
  position that shares `(symbol, side)` with a local active position.

### Negative / Trade-offs

- The operator cannot use the Robson-operated account for manual trading.
  Workaround: operate manual trades on a separate account whose keys are never
  loaded into `robsond`.
- An engineering cost is incurred: reconciliation worker, exchange-order-id index,
  close path, alerting.
- Startup is slower: the daemon cannot accept new observations until the startup
  reconciliation pass is complete.
- A false positive (an `entry_order_accepted` event that should exist but is missing
  due to a bug) results in an auto-close of a legitimate position. Mitigation: the
  exchange-order-id ↔ event-log link must be written atomically with the order
  placement (follow-up required).
- A manual account loss or withdrawal now forces an additional reconciliation step:
  current-month `capital_base` must be recalibrated before entries resume.

### Operational

- `ROBSON_POSITION_MONITOR_ENABLED` is no longer an application setting. Its
  false GitOps value is retained only for rollback safety. The reconciliation
  worker is **always on**.
- VAL-001 gains a new pre-flight / phase: confirm zero UNTRACKED positions before
  starting the lifecycle validation.
- VAL-002 Safety Checks Before Flip explicitly include reconciliation-worker-scan
  cleanliness, not just `/status` reporting zero active positions.

---

## Implementation Notes

Implemented current scope and remaining follow-up:

1. **Implemented**: startup and periodic futures reconciliation plus a mandatory
   market-close path outside the entry-side risk gate.
2. **Follow-up required**: index and match the originating exchange order id in
   O(1); the current worker matches `(symbol, side)`.
3. **Follow-up required**: cover spot and margin account types in addition to the
   current USD-M Futures adapter.
4. **Follow-up required**: complete CRITICAL alert delivery and the audited,
   bounded `POST /reconciliation/suspend` target.
5. **Follow-up required**: add a controlled VAL-001 UNTRACKED-position scenario.

### Invariants (non-negotiable)

1. Every open exchange position MUST correspond to an `entry_order_accepted`
   event whose `cycle_id` references a `GovernedAction` and whose
   `exchange_order_id` identifies the acknowledged entry.
2. The reconciliation worker MUST NOT use the `allowed_symbols` whitelist when
   scanning.
3. The close path for UNTRACKED positions MUST NOT be gated by any feature flag.
   The former `ROBSON_POSITION_MONITOR_ENABLED` setting is not part of the
   application configuration.
4. Back-filling a synthetic `entry_order_accepted` event for a position that did not
   pass the Risk Engine is a policy violation.

### Related Components

- `robsond/src/reconciliation_worker.rs` — current startup and periodic worker
- `robsond/src/position_manager.rs` — tracked-position lifecycle and close path
- `robson-eventlog/` — exchange-order-id index on events
- `robson-exec/src/executor.rs` — exchange query for open positions (all symbols)

---

## References

- [docs/policies/UNTRACKED-POSITION-RECONCILIATION.md](../policies/UNTRACKED-POSITION-RECONCILIATION.md) — full policy text
- [docs/architecture/v3-runtime-spec.md](../architecture/v3-runtime-spec.md) —
  Zero-Bypass Guarantee, Recovery Procedures
- [docs/architecture/v3-control-loop.md](../architecture/v3-control-loop.md) —
  Crash Recovery §Reconciliation
- [docs/architecture/v3-risk-engine-spec.md](../architecture/v3-risk-engine-spec.md)
- [docs/runbooks/val-001-testnet-e2e-validation.md](../runbooks/val-001-testnet-e2e-validation.md)
- [docs/runbooks/val-002-real-capital-activation.md](../runbooks/val-002-real-capital-activation.md)
- [ADR-0007 — Robson is a Risk Assistant, not an Autotrader](ADR-0007-robson-is-risk-assistant-not-autotrader.md)
- [ADR-0021 — Opportunity Detection vs Technical Stop Analysis](ADR-0021-opportunity-detection-vs-technical-stop-analysis.md)
- [ADR-0023 — Symbol-Agnostic Policy Invariant](ADR-0023-symbol-agnostic-policy-invariant.md) (companion)
- [ADR-0038 — Capital Base Recalibration After Manual Account Change](ADR-0038-capital-base-recalibration-after-manual-account-change.md)
- [TD-2026-05-05-001 — Core Position Lifecycle Drift](../technical-debt.md) and its
  [Implementation Guide](../implementation/TD-2026-05-05-001-CORE-LIFECYCLE-DRIFT.md)
- [Runbook td-2026-05-05-001-stale-active-recovery](../runbooks/td-2026-05-05-001-stale-active-recovery.md) — operator recovery when the startup gate aborts under I3

---

## Amendments

### 2026-05-08 — I3: Reverse Reconciliation (TD-2026-05-05-001)

**Context.** The original ADR (2026-04-18) established the invariant
`Open positions on the operated account ⊆ Robson-authored entries`. This
covers the **exchange-to-Robson direction** of the symmetric relation
between local lifecycle state and the exchange's view of the account: any
foreign open position on the operated account is closed (UNTRACKED).

The **opposite direction** was implicitly assumed but not enforced:
positions tracked locally as `Active` were assumed to remain present on
the exchange because every exit was supposed to flow through Robson's
own `Active → Exiting → Closed` pipeline. In practice, three classes of
event break that assumption:

1. **Forced liquidation by Binance** under maintenance margin breach.
2. **Manual close on the Binance UI** by the operator.
3. **Insurance-stop fill while the daemon is offline** beyond the 15-minute
   `startup_recovery` candle-replay window.

In all three the exchange is the source of truth and the local projection
remains stale `Active`. None of I1/I2 detect this. The reconciliation
worker as built today walks only the exchange side.

**Amendment.** Add the symmetric component:

> **I3 — Reverse Reconciliation Invariant.** Every position the local
> store holds in `Active` MUST have a corresponding open position on the
> exchange, matched by `(symbol, side)` and within the configured
> `quantity` tolerance. If the position is missing on the exchange after
> a grace period and a second consecutive observation, Robson MUST
> gather evidence from the exchange and transition the local position
> to `Closed` via reverse reconciliation.

**Symmetry, in summary.**

| Direction | Invariant | Detection target | Action |
|---|---|---|---|
| Exchange has, Robson does not | I1 / I2 (UNTRACKED) | Foreign open position on the operated account | Close at market, tag `UNTRACKED_ON_EXCHANGE` |
| Robson `Active`, Exchange does not | I3 (stale-Active) | Local lifecycle drift after liquidation, manual close, externally-resident insurance stop fill, etc. | Gather evidence (`OrderFillRecord` → `UserTradeRecord` → `AccountSnapshot` → `Estimated`), close locally with `ReconciledMissingOnExchange` |

**Scope clarification (Active-only).** I3 only auto-closes `Active`. The
worker MUST detect and structurally log `Entering` and `Exiting`
positions whose exchange counterpart is missing, but MUST NOT auto-close
them in this TD. A separate technical debt entry will design a safe
auto-close for those after the `Active` path is proven in production.

**Detection rule.** Single-observation drift is insufficient — placement
latency, websocket vs REST snapshot inconsistencies, and exchange
maintenance windows can all produce transient absence. The worker MUST
require a grace period plus a second consecutive observation before
invoking the close path. See policy §I3 §B.

**Evidence ordering, no silent fallback.** Every reconciliation-close
event MUST carry a `ClosureEvidence::Reconciled(...)` payload (introduced
in Slice 1 of TD-2026-05-05-001) populated in priority order:
`OrderFillRecord` > `UserTradeRecord` > `AccountSnapshot` > `Estimated`.
`Estimated` is never silently substituted for a real fill — every
`Estimated` close emits a `CRITICAL` operator alert and increments
`robson_reconciliation_estimated_closes_total`. At startup, an
`Estimated`-only close path NEVER runs automatically; the daemon aborts
and defers to the operator runbook. See policy §I3 §C and §D.

**Authoritative source documents.** The full operational rules,
configuration knobs, and rollback semantics for I3 live in:

- [`docs/policies/UNTRACKED-POSITION-RECONCILIATION.md` §I3](../policies/UNTRACKED-POSITION-RECONCILIATION.md) — policy text
- [`docs/implementation/TD-2026-05-05-001-CORE-LIFECYCLE-DRIFT.md`](../implementation/TD-2026-05-05-001-CORE-LIFECYCLE-DRIFT.md) — slice plan
- [`docs/runbooks/td-2026-05-05-001-stale-active-recovery.md`](../runbooks/td-2026-05-05-001-stale-active-recovery.md) - current recovery procedure

This ADR remains the canonical authority for the existence and
non-negotiability of the invariant; the policy holds the operational
detail and may evolve as I3's mechanics are refined without re-amending
this ADR.

### 2026-05-09 — Slice 5B1: manual recovery path live

Operator-driven manual recovery is live via `robson-cli reconcile-close` and
`POST /reconcile-close`. Runbook §Recovery Command is now operational for
`OrderFillRecord` and `UserTradeRecord` evidence. `AccountSnapshot` and
`Estimated` remain rejected for the operator-CLI path.

### 2026-05-11 — Slice 5B2A: evidence helper refactor merged

`reconciliation_worker.rs` evidence helpers refactored (no behavior change).

### 2026-08-06: startup `auto_reconcile` operational status

Startup `auto_reconcile` is implemented and production configuration was
read-only verified as enabled on 2026-08-06. The invariant still requires real
exchange evidence and no partial close.

Current source does not preserve the documented exit-78 behavior when evidence
gathering returns no unambiguous match. It logs a warning and continues startup
so periodic reconciliation may resolve the stale position. In addition, Phase 2
persists closes sequentially without a transaction spanning the batch, so a
later apply failure cannot roll back an earlier close. These are open policy
drifts, not approved relaxations of the invariant.
