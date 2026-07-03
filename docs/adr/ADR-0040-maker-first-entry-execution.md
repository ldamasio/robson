# ADR-0040: Maker-First Entry Execution

Status: Proposed
Date: 2026-07-03

## Context

Every Robson order is a market order today: entry, exit, and the triggered
insurance stop (ADR-0039) all take liquidity and pay the taker fee on both
legs of every trade. On the operated account the taker rate is 0.10% per
fill — the first buffered-sizing trade (2026-07-03, position `019f25cc`)
paid 1.23 USDT on a 1,229 USDT notional entry, and ADR-0039 sizing now
prices round-trip taker fees directly into the 1% risk budget, shrinking
position size accordingly.

The two legs are not symmetric:

- **Exit/stop leg**: not filling is an unbounded loss. Market-taker
  execution is the correct, non-negotiable choice.
- **Entry leg**: not filling costs nothing — a missed entry is a lost
  opportunity, never a realized loss. This leg can wait a bounded time to
  earn the maker rate.

"Immediate" and "maker" are definitionally incompatible: an order that
executes on arrival consumed liquidity. What is achievable is a
maker-first entry that fills as maker in the common case and degrades to
market within an explicit budget.

## Needs Input

- Operator maker/taker rates for the account (until the runtime
  `GET /fapi/v1/commissionRate` fetch lands, see Related Work) — determines
  whether the added complexity pays for itself at current trade frequency.
- Acceptable entry latency budget (`max_wait`) and adverse-drift bound
  (`max_drift_bps`) — these encode how much opportunity risk the operator
  trades for fee savings.

## Decision

Add a maker-first execution mode for the entry leg only:

1. **Post-only limit at top-of-book.** Instead of a market order, the entry
   is submitted as a post-only limit (Binance USD-M `timeInForce=GTX`) at
   the best price on the entry side (long → best bid, short → best ask).
   Post-only guarantees the order never executes as taker: if it would
   cross the spread, the exchange rejects it and the executor reprices.
2. **Bounded chase.** While unfilled, the order is cancel-replaced at the
   new top-of-book on a fixed interval (`chase_interval`, e.g. 500 ms–1 s),
   with deterministic per-reprice client ids for idempotency.
3. **Market escape.** The remainder converts to a market order when either
   bound is hit: total wait exceeds `max_wait`, or price drifts more than
   `max_drift_bps` against the signal reference (the detector's
   `entry_immediate` reference price anchors both the initial limit and the
   drift bound).
4. **Partial fills** accumulate as maker; only the unfilled remainder
   escapes to market. The position's entry price is the blended fill.
5. **Risk model unchanged.** Position sizing keeps assuming worst-case
   taker on both legs (Policy 10: the cap must hold on the worst path,
   which includes a full market escape). Fee savings therefore show up as
   realized-cost improvement, never as a larger position.
6. **Scope guard.** Exits, stops, and the insurance stop remain market/
   taker. Only the entry leg is eligible. Operator-facing mode selection:
   `Immediate` (current behavior) vs `MakerFirst { chase_interval,
   max_wait, max_drift_bps }`, operator-configured with conservative
   defaults.

## Consequences

Positive
- Entry-leg fee drops from taker to maker in the common case (on the
  operated account: 0.10% → maker tier, a 50–80% reduction on that leg)
  with zero change to the risk model.
- Bounded and explicit opportunity cost: the escape hatch caps both wait
  time and adverse drift.
- Composes with ADR-0039: cheaper entries widen the effective risk budget
  headroom the sizing buffer reserves.

Negative / Trade-offs
- `Entering` state gains a chase sub-machine (placed → repricing →
  partially filled → escaped) with more events to journal and reconcile.
- Some entries will fill later or partially at worse blended prices than
  an immediate market order would have; in fast markets the escape path
  pays taker anyway (plus the chase latency).
- More order churn against exchange rate limits (bounded by
  `chase_interval` and `max_wait`).
- Crash mid-chase leaves a resting post-only order: startup recovery must
  adopt-or-cancel it (same reconciliation discipline as ADR-0039 insurance
  orders, `ent-` client-id prefix).

## Alternatives

- **A. Status quo (always market).** Pays taker on the one leg where
  waiting is free. Rejected as the permanent answer; remains the fallback
  and the `Immediate` mode.
- **B. Pure passive limit, no escape.** Unbounded miss risk; contradicts
  the operator's decision to enter (the WHEN is theirs, Robson must not
  silently veto it by never filling). Rejected.
- **C. Single post-only attempt with GTD auto-expiry, no chase.** Simpler,
  but fill rate degrades quickly in moving markets and the miss/latency
  trade-off is worse than a bounded chase. Rejected.
- **D. Fee-tier optimization (BNB discount, VIP volume).** Orthogonal,
  account-level, and complementary — pursue independently of this ADR.

## Implementation Notes

- `robson-exec/ports.rs`: `place_post_only_order(symbol, side, quantity,
  price, client_order_id)` (GTX; rejected-on-cross surfaces as a typed
  error, not a failure) + existing `cancel_order`; entry chase client ids
  use an `ent-` prefix mirroring the `ins-` convention.
- `robson-engine`: entry decision emits the configured execution mode;
  `Entering` carries chase state (attempts, anchor price, filled-so-far).
- Executor or a dedicated entry-execution task owns the chase loop; every
  reprice/escape is journaled (intent) and audited (events
  `EntryChaseStarted/Repriced/Escaped/Completed`).
- Reconciliation + startup recovery: adopt-or-cancel resting `ent-` orders
  exactly like `ins-` insurance orders (ADR-0039 sweep extension).
- Config: `ROBSON_ENTRY_EXECUTION_MODE`, `ROBSON_ENTRY_CHASE_INTERVAL_MS`,
  `ROBSON_ENTRY_MAX_WAIT_MS`, `ROBSON_ENTRY_MAX_DRIFT_BPS`.
- Tests: chase reprice emission, escape on both bounds, partial-fill
  blending, crash-mid-chase recovery, sizing invariance (same quantity in
  both modes).

## Related Work

- Runtime account fee discovery (`GET /fapi/v1/commissionRate`) replacing
  the `ROBSON_TAKER_FEE_RATE` env default (ADR-0023 compliance: exchange
  metadata at runtime, env as conservative floor/override). Complements
  this ADR: live maker/taker rates feed both sizing estimates and the
  chase's cost/benefit telemetry.
- ADR-0039 (exchange-side insurance stop): established the fee-aware
  sizing and the authored-order sweep pattern this ADR reuses.
