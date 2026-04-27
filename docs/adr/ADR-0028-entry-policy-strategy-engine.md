# ADR-0028 — Entry Policy Strategy Engine

**Date**: 2026-04-27
**Status**: Accepted
**Deciders**: RBX Systems (operator + architecture)

---

## Context

Robson's entry detection was embedded in `DetectorTask` as an implicit SMA 9/21
crossover detector with no explicit policy model. The operator could not select
a different detection strategy or control the approval workflow independently.
Approval was coupled to a notional-threshold adapter in `query_engine.rs`, which
made human-review gating dependent on position size rather than operator intent.
Position cancellation was modelled as `Closed`, conflating a pre-entry disarm
with a post-fill exit and losing semantic correctness in projections and audit
logs.

These problems share a root cause: the entry pipeline lacked an explicit policy
layer. Opportunity detection, strategy evaluation, approval gating, and
lifecycle tracking were all implicitly wired without named abstractions.

---

## Decision

Introduce a four-part model that separates **what** the operator wants
(entry policy), **how** the system detects it (strategy), **who** approves it
(approval policy), and **where** it is in its lifecycle (computed stage).

### 1. EntryPolicyConfig

`EntryPolicyConfig { mode: EntryPolicy, approval: ApprovalPolicy }` is set at
ARM time. `EntryPolicy` selects the detection strategy; `ApprovalPolicy`
controls the human-review gate. Both are independent.

- `EntryPolicy`: `ConfirmedTrend`, `ConfirmedReversal`, `ConfirmedKeyLevel`,
  `Immediate`.
- `ApprovalPolicy`: `Automatic`, `HumanConfirmation`.
- Default: `ConfirmedTrend + Automatic`.

### 2. StrategyRegistry (deterministic strategy resolution)

`StrategyRegistry` maps each `EntryPolicy` to a deterministic `StrategyId` and
concrete `SignalStrategy` implementation. Resolution is a pure function:

| Policy                | StrategyId             | Strategy                       |
|-----------------------|------------------------|--------------------------------|
| `ConfirmedTrend`      | `sma_crossover:v1`     | `SmaCrossoverStrategy`         |
| `ConfirmedReversal`   | `reversal_patterns:v1` | `ReversalPatternStrategy`      |
| `ConfirmedKeyLevel`   | `key_level:v1`         | `KeyLevelStrategy`             |
| `Immediate`           | none                   | No strategy (system stop only) |

Each strategy is deterministic: same OHLCV candles produce the same
`SignalDecision`. No ML, probabilistic logic, or external data sources.

### 3. EntryLifecycleStage (computed projection)

`EntryLifecycleStage` is never stored. It is always recomputed from the event
sequence via `entry_lifecycle_stage(events: &[Event])`. Stages:

`IntentCreated` -> `AwaitingSignal` -> `SignalConfirmed` ->
`AwaitingApproval` -> `OrderSubmitted` -> `Active` -> `Cancelled`

The `AwaitingApproval` stage is evidenced by `EntryApprovalPending` domain
events in the event log (see ADR-v3-027).

### 4. PositionState::Cancelled (terminal pre-entry state)

`Cancelled` is a terminal state for positions disarmed before the entry order
was placed. It is distinct from `Closed` (position that traded and exited).
This distinction preserves semantic correctness: a `Cancelled` position never
had a fill, never contributed to P&L, and should not appear in trading
performance metrics.

---

## Approval policy independence

`DomainApprovalPolicy` is authoritative over the legacy notional-threshold
adapter in `query_engine.rs`. The adapter remains for backward compatibility,
but `check_approval_with_domain_policy` always respects the operator-selected
approval mode:

- `Automatic`: execution proceeds without human review regardless of notional
  amount.
- `HumanConfirmation`: execution always waits for operator approval regardless
  of notional amount.

---

## Cancelled vs Closed

| Property            | `Cancelled`              | `Closed`                    |
|---------------------|--------------------------|-----------------------------|
| Entry order placed  | No                       | Yes                         |
| Fill received       | No                       | Yes                         |
| P&L contribution    | None                     | Recorded                    |
| Risk budget impact  | None (slot freed)        | Realized P&L accounted      |
| Projection column   | `state = 'cancelled'`    | `state = 'closed'`          |

---

## Replay safety

`EntryLifecycleStage` is never persisted. It is recomputed from the event
sequence on every call. `EntryApprovalPending` provides the audit evidence
needed for the `AwaitingApproval` stage. This ensures:

1. The same event sequence always produces the same stage.
2. No migration is needed when stages are added or renamed.
3. Event log is the sole source of truth; projections are derived.

---

## Non-negotiables (carried from ADR-0021)

1. **TechnicalStopDistance** is always derived from chart analysis. Never
   `entry * (1 - pct)`.
2. **Risk gate is mandatory** before any entry. No code path reaches the
   executor without a prior risk-approved `GovernedAction`.
3. **Opportunity detection and technical stop analysis are separate
   responsibilities**. Strategies decide whether there is a signal;
   `TechnicalStopAnalyzer` determines where the system-defined stop is.
4. No ML, probabilistic logic, partial entries, or user-defined stops.

---

## Consequences

- Operator can select entry strategy and approval mode independently at ARM
  time.
- Strategy evaluation is deterministic and auditable via
  `SignalStrategyEvaluated` events.
- Approval flow is independent of position size.
- `Cancelled` positions are semantically distinct from `Closed` positions in
  all projections and queries.
- Lifecycle stage is always derivable from the event log without stored state.

---

## Related

- [ADR-0021](ADR-0021-*.md) — Technical stop analysis separation
- [ADR-0022](ADR-0022-robson-authored-position-invariant.md) — Robson-authored
  position invariant
- [ADR-0023](ADR-0023-symbol-agnostic-policy-invariant.md) — Symbol-agnostic
  policy invariant
- [ADR-v3-027](../architecture/v3-architectural-decisions.md) —
  EntryApprovalPending dual emission
- [Implementation guide](../implementation/entry-policy-strategy-engine.md)
