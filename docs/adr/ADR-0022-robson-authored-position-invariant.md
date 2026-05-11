# ADR-0022 — Robson-Authored Position Invariant

**Date**: 2026-04-18
**Last Amended**: 2026-05-11 (5B1 live + 5B2A merged; see Amendments)
**Status**: DECIDED — IN PROGRESS (I3 runtime steady-state and startup abort live; manual recovery 5B1 live; startup auto_reconcile 5B2B planned)
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
   exchange without persisting an `entry_order_placed` event.
4. A partial deploy / race where the executor placed the order but the event-log
   append failed silently.

Every such position silently consumes the account's exposure, margin, and risk
budget that the Risk Engine assumes is free — with cascading effects under leverage.
It also has no technical stop, no span, no governance trail, and is invisible to the
monthly drawdown calculation.

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
`GovernedAction` token. Every such order produces an `entry_order_placed` (or
equivalent v3) event in `event_log` with the exchange-assigned order id.

This is **already guaranteed by QueryEngine** for orders originating in `robsond`.
The new requirement is to make the exchange-order-id ↔ event-log link queryable in
O(1) for the reconciliation worker (new index / projection).

### I2 — Reconciliation (enforced at read time)

A **Position Reconciliation Worker** runs periodically in the runtime. On each scan:

1. Query Binance for all open positions across all account types (spot, margin,
   USD-M Futures) and all symbols.
2. For each open position, look up the matching `entry_order_placed` event in
   `event_log` by exchange order id.
3. If no matching event exists → classify the position as **UNTRACKED**.
4. Persist `position_untracked_detected`, alert the operator, and **close the
   position at market** via the Safety Net close path.
5. Persist `untracked_position_closed` on the resulting fill.

The close is mandatory and runs unconditionally — it is not gated by
`ROBSON_POSITION_MONITOR_ENABLED` (which only gates the trailing-stop monitor).

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
  Rejected — that flag gates the trailing-stop monitor for active, tracked
  positions. An UNTRACKED position is a policy violation that must be closed
  regardless of whether the operator has enabled live trailing-stop management.
- **Single-user honor system.** Rejected — Robson must be architecturally correct
  against its own operator, not just against third parties. A single rushed manual
  order can destroy weeks of compounded gains.

---

## Consequences

### Positive

- The Risk Engine's guarantees hold end-to-end: no shadow positions outside its
  scope.
- Audit trail is closed: every open position has a matching governance event.
- Reconciliation becomes proactive rather than passive adoption.
- Failure mode for leaked / shared API keys: an attacker's position is closed within
  one reconciliation interval.

### Negative / Trade-offs

- The operator cannot use the Robson-operated account for manual trading.
  Workaround: operate manual trades on a separate account whose keys are never
  loaded into `robsond`.
- An engineering cost is incurred: reconciliation worker, exchange-order-id index,
  close path, alerting.
- Startup is slower: the daemon cannot accept new observations until the startup
  reconciliation pass is complete.
- A false positive (an `entry_order_placed` event that should exist but is missing
  due to a bug) results in an auto-close of a legitimate position. Mitigation: the
  exchange-order-id ↔ event-log link must be written atomically with the order
  placement (follow-up required).

### Operational

- `ROBSON_POSITION_MONITOR_ENABLED` gates trailing-stop management only. A new
  conceptual flag — the reconciliation worker — is **always on**.
- VAL-001 gains a new pre-flight / phase: confirm zero UNTRACKED positions before
  starting the lifecycle validation.
- VAL-002 Safety Checks Before Flip explicitly include reconciliation-worker-scan
  cleanliness, not just `/status` reporting zero active positions.

---

## Implementation Notes

Follow-up work required (tracked as `MIG-v3#TBD — Reconciliation Worker`):

1. **Event indexing**: projector indexes `entry_order_placed` and `exit_order_placed`
   events by exchange order id for O(1) lookup.
2. **Reconciliation worker**: new long-lived task inside `robsond` scanning every
   60–300 s (configurable). Scans all account types (spot, margin, futures) and
   all symbols.
3. **Close path**: dedicated Safety Net path tagged `UNTRACKED_ON_EXCHANGE`. Does
   not go through the entry-side risk gate (closing is always allowed).
4. **Alerting**: `position_untracked_detected` emits a CRITICAL operator alert.
5. **Startup gating**: daemon enters `StartupReconciling` state before accepting
   observations; blocks new entries until UNTRACKED set is empty.
6. **Operator override**: `POST /reconciliation/suspend` with max TTL 300 s for
   exceptional cases (e.g., a human-in-the-loop migration). Audited end-to-end.
7. **VAL-001 scenario**: open an UNTRACKED position manually on testnet, confirm
   detection and auto-close.

### Invariants (non-negotiable)

1. Every open exchange position MUST correspond to an `entry_order_placed` event
   whose `cycle_id` references a `GovernedAction`.
2. The reconciliation worker MUST NOT use the `allowed_symbols` whitelist when
   scanning.
3. The close path for UNTRACKED positions MUST NOT be gated by
   `ROBSON_POSITION_MONITOR_ENABLED` or any feature flag.
4. Back-filling a synthetic `entry_order_placed` event for a position that did not
   pass the Risk Engine is a policy violation.

### Related Components

- `v3/robsond/src/position_manager.rs` — will own the reconciliation loop
- `v3/robsond/src/safety_net.rs` (target) — close path for UNTRACKED positions
- `v3/robson-eventlog/` — exchange-order-id index on events
- `v3/robson-exec/src/executor.rs` — exchange query for open positions (all symbols)

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
- [`docs/runbooks/td-2026-05-05-001-stale-active-recovery.md`](../runbooks/td-2026-05-05-001-stale-active-recovery.md) — recovery procedure (skeleton; finalized in Slice 5)

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
Startup `auto_reconcile` (Slice 5B2B) remains planned.

**Invariant preserved**: auto-close at startup requires real exchange evidence
(`OrderFillRecord` or `UserTradeRecord`) and is all-or-nothing — any position
lacking evidence aborts startup with exit 78, consistent with the Robson-authored
position invariant's fail-closed guarantee.
