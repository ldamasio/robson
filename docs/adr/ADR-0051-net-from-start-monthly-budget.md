# ADR-0051 — Net-From-Start, Non-Expanding Monthly Budget

**Date**: 2026-08-04
**Status**: Accepted (operator decision, 2026-08-04)
**Deciders**: RBX Systems (operator + architecture, with adversarial design
review by a secondary model)
**Supersedes**: [ADR-0046](ADR-0046-monthly-high-water-mark-budget.md)

---

## Context

ADR-0046 anchored the monthly budget to the month's governed **equity peak**
(high-water mark, unrealized P&L included): every new peak re-armed the 4%
cushion, and any give-back from the peak consumed budget.

August 2026, real account, exposed the product problem: two trades, both
closed profitable (+15.69 and +27.94 net), no open positions — yet the
dashboard showed 3 slots. The equity peak ($46.94) had been recorded
mid-trade while unrealized profit was on screen; the settled month
($40.63) sat $6.31 below it, consuming 0.4% of budget for a month with zero
losses.

The operator rejected the high-water-mark anchor outright (2026-08-04):

1. The monthly budget guarantees exactly one thing: the month's governed
   loss, measured against the month-start capital base, never exceeds 4%.
2. More than 4 operations per month is acceptable, even after losing trades,
   as long as invariant (1) holds.
3. A month containing only profitable closed trades and no open positions
   must show the full 4 guaranteed slots. Give-back of intra-trade
   unrealized profit is governed by the position's own trailing stop and the
   1% per-trade cap, not by the monthly budget.
4. **Pessimistic open-position assumption** (operator, verbatim intent):
   every open position is always assumed to walk to defeat, not victory. Its
   full worst-case loss is reserved against the budget the moment it exists;
   if it wins instead, so much the better, but a loss must already be
   expected and must not be able to push the month past the 4% cap.

A first redesign candidate ("equity-net-from-start": consumed =
`max(0, −month_equity_net)` with unrealized P&L **inside** the anchor) was
adversarially reviewed and found unsafe: an unrealized winner whose stop sits
at breakeven carries zero latent risk, so its paper profit can mask an
already-realized loss and let admission spend the full budget. Worked
breach: realized −$15.78, open winner +$20 unrealized with breakeven stop →
equity +$4.22, consumed $0, four new trades admitted at $62 total risk; if
the winner retraces to breakeven and the four trades gap to their stops, the
month ends at −$77.78 ≈ −4.93% of a $1,578 base. The promised floor was
−$63.12. Unrealized profit is not protected by any reservation, therefore it
must not appear in the budget anchor.

## Decision

### 1. The monthly budget is net-from-start and non-expanding

The monthly budget protects one invariant: governed monthly loss must not
exceed 4% of the monthly budget capital base. It does not protect unrealized
or realized profit peaks.

Let:

- `capital_base` be the persisted monthly budget basis (ADR-0024 §6);
- `monthly_budget = capital_base × 4%`;
- `risk_unit = capital_base × 1%`;
- `governed_realized_net` be the signed, settlement-complete governed result
  attributable to the current monthly basis, including fees and funding and
  excluding unrealized P&L and out-of-band account drift;
- `latent_risk` be the cost-priced worst-case loss reserved for every
  Active or Entering position and any unsettled governed liability — the
  pessimistic assumption of Context item (4) made mechanical.

The authoritative calculation is:

```
consumed          = max(0, −governed_realized_net)
remaining_budget  = monthly_budget − consumed − latent_risk
slots_available   = min(4, floor(max(0, remaining_budget) / risk_unit))
```

Realized gains can offset prior realized losses until net reaches zero.
Gains above zero do not increase the monthly budget. Unrealized P&L never
changes the monthly budget, in either direction.

**Safety argument.** Admission (ADR-0043) guarantees that total newly
admitted planned risk `A` satisfies `A + latent_risk ≤ monthly_budget −
consumed`. Worst-case final governed net is `governed_realized_net −
latent_risk − A`. If net ≥ 0 this is ≥ `net − monthly_budget ≥
−monthly_budget`; if net < 0 then `latent_risk + A ≤ monthly_budget + net`,
so the result is again ≥ `−monthly_budget`. The proof needs no unrealized
term, which is precisely why unrealized P&L is excluded from the anchor.

**August 2026 under this model**: consumed $0.00, open risk reserved $0.00,
remaining $63.12, **4 slots**.

### 2. Admission and halt use one canonical snapshot

A proposed trade is admitted only when its cost-priced planned risk is no
greater than both the 1% per-trade cap and `remaining_budget`. An unpriced
trade conservatively reserves the full 1% unit (ADR-0043 fallback).

MonthlyHalt triggers when `remaining_budget ≤ 0`. It latches, blocks new
entries, and closes risk-bearing positions. The −4% guarantee applies within
the execution cost and gap envelope used by planned-risk and latent-risk
pricing (ADR-0039); a breach outside that envelope latches MonthlyHalt and
raises a high-severity reconciliation alarm.

Admission, slot reporting, the status API, and MonthlyHalt must consume one
canonical monthly-budget snapshot. Active, Entering, partially filled, and
close-in-reconciliation positions may not be treated differently across
those paths. (This closes a live inconsistency: `evaluate_monthly_halt`
reserves latent risk for Active positions only, while
`compute_slots_available` and `monthly_budget_snapshot` also count
Entering.)

### 3. Accounting lifecycle

A position's latent-risk reservation remains in force until its evidenced
realized P&L, commissions, funding, and other governed costs have replaced
that reservation atomically. If authoritative monthly accounting is
unavailable, or an unsettled liability cannot be bounded, new entries fail
closed.

`robson_month_net` retains its raw signed semantics for drift attribution
(ADR-0045); budget accounting reads its own settlement-complete projection.
Out-of-band account changes never enter governed monthly net.

At the UTC month boundary, carried-position downside is absorbed into the
new capital base exactly once (ADR-0024 §6) and carried positions are
rebased against that pessimistic valuation. The reset and carried basis are
durable and idempotent.

### 4. Peak state is retired

`month_peak_net` is no longer an input to risk decisions or reporting.
`refresh_month_peak_net` and its cache are removed after the compatibility
window. The database column is frozen at cutover and dropped in a later
migration.

The frontend replaces peak give-back reporting with: governed realized net,
net loss consumed, open risk reserved, and monthly budget remaining.
Card copy: "MONTHLY NET-LOSS BUDGET · net governed loss since month start,
plus risk reserved for open positions. Profits do not increase the 4%
limit." Mark-to-market month equity may remain only as a separately labelled
informational metric.

### 5. Rollout (live production)

1. **DB expansion**: add `monthly_budget_model` to `monthly_state`
   (default `hwm_v1`); keep `month_peak_net` intact.
2. **Daemon, dormant dual-model release**: one canonical snapshot function
   shared by admission/status/slots/halt; shadow-calculate both models and
   alarm on unexplained divergence.
3. **Frontend**: model-aware rendering of the new card; stop depending on
   peak/give-back fields.
4. **Activation**: reconcile the August ledger gap (governed closes sum
   +$43.63 vs month net $40.63 — the ~$3.00 must be explained: late
   fees/funding or gross-vs-net figures), verify the account is flat, then
   atomically switch the persisted model to
   `net_from_start_non_expanding_v1` without resetting the month.
5. **Cleanup**: remove peak refresh/cache/API code, then drop
   `monthly_state.month_peak_net` in a separate migration.

## Consequences

- Positive: a profitable month always shows 4 guaranteed slots; slot count
  can no longer flap on mark noise (only governed accounting events move
  it); no persisted peak state; the −4% floor gets a closed-form proof.
- Trade-off (accepted knowingly): realized gains are not protected by the
  monthly mechanism. Win +3% early and the month may give those gains back
  before the −4% floor binds. Per operator decision, protecting profit is
  the job of each position's trailing stop, not of the monthly budget.
- Halting while month-positive can no longer happen (halt requires
  `consumed + latent_risk ≥ 4%`, and consumed > 0 implies realized net
  loss), removing ADR-0046's main UX surprise.

## Alternatives

- **Equity-net-from-start** (unrealized inside the anchor) — rejected:
  unsafe; masked-loss breach shown in Context.
- **Realized-only HWM** (peak over realized closes only) — rejected: still a
  trailing peak anchor; a late $3 fee after a $43.63 realized peak recreates
  the exact August problem (3 slots).
- **Partial profit-lock** (floor = −4% + λ × realized gains) — rejected for
  now: introduces a policy knob the operator has not asked for; can be
  revisited as a future ADR if profit protection at month level is ever
  wanted.
- **Keep ADR-0046 HWM** — rejected by operator: the budget must not protect
  peaks.

## Implementation Notes

- Code paths: `robsond/src/position_manager.rs`
  (`compute_slots_available`, `evaluate_monthly_halt`,
  `monthly_budget_snapshot`, `refresh_month_peak_net`, `month_equity_net`,
  `governed_monthly_realized_loss`), `robson-domain/src/policy.rs`
  (`TradingPolicy::slots_available`), `robsond/src/api.rs` (`/status`
  fields), `frontend` dashboard MONTHLY LIMIT card.
- Test invariants to pin: `0 ≤ consumed`; `remaining ≤ monthly_budget`;
  `slots ≤ 4`; unrealized mark changes alone never change consumed,
  remaining, slots, admission, or halt; every admitted sequence within the
  priced envelope ends ≥ −4%; `Entering → Active → Closed` never leaves an
  interval where neither latent risk nor settled result is charged;
  admission/status/slots/halt read byte-equivalent snapshots; restart
  reproduces the same snapshot; month reset idempotent, carried risk charged
  exactly once; out-of-band drift never enters governed net.
- Highest-value regression tests: (a) August case — flat account, positive
  net, 4 slots; (b) masked-loss case — realized −1%, unrealized winner with
  breakeven stop, remaining reflects the realized loss and marks do nothing;
  (c) Entering-position consistency across all four consumers; (d) atomic
  close settlement (reservation held until fees/funding evidenced);
  (e) month-boundary restart idempotency.
- Related: ADR-0024 (policy layer, month boundary), ADR-0039 (cost-priced
  worst case), ADR-0043 (planned-risk admission, slots as guaranteed
  minimum, halt at ≤ 0), ADR-0045 (governed-only flow), ADR-0046
  (superseded).
