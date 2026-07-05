# ADR-0043 — Budget-Metered Entry Admission

**Date**: 2026-07-05
**Status**: Decided
**Deciders**: RBX Systems (operator + architecture)

---

## Context

ADR-0024 Decision 5 replaced static position limits with a dynamic slot model:
`slots_available = floor(remaining_budget / risk_per_trade_amount)`, where
`risk_per_trade_amount` is the fixed 1% per-trade cap. Admission of a new entry
required `remaining_budget ≥ 1%` — the gate reserved the **worst case** for
every prospective trade, because at slot-counting time the next trade's stop
was unknown.

Two things have changed since:

1. **The actual risk of an entry is known at admission time.** The entry
   pipeline (ADR-0028) derives the technical stop before the risk gate runs,
   and position sizing (ADR-0039, ADR-0042) prices the full worst-case loss —
   stop distance, executable-stop buffer, gap allowance, round-trip taker
   fees — into the quantity. Margin capping and exchange quantity filters
   routinely make the planned worst-case loss **less than** the 1% cap.
2. **Budget accounting already charges actual amounts.** Realized losses and
   latent risk are tracked at their real values, not the cap. Only admission
   still reserved the full 1%.

The result was an asymmetry: a month could end with budget left on the table
that no trade was allowed to use, because the last fraction of the budget was
smaller than one full risk unit. The pessimistic reservation no longer
reflected the entry rules.

## Decision

### 1. Entries are admitted by their actual planned risk

The risk gate charges each proposed entry its **planned worst-case loss**
(cost-priced per ADR-0039: per-unit worst loss × final quantity), not the full
1% cap:

```
remaining_budget = monthly_budget − realized_loss − latent_risk
admit            ⇔ remaining_budget ≥ planned_risk
```

- The engine computes `planned_entry_risk` in `decide_entry` from the same
  `worst_case_loss_per_unit` the sizing used, clamped to
  `max_risk_amount` (quotient rounding on the risk-sized path can put the
  product a hair above the cap; sizing guarantees the cap by construction).
- `ProposedTrade.planned_risk` carries it to the gate. A non-positive value
  means "unpriced" and falls back to reserving the full 1% cap (pre-ADR-0043
  behavior), so legacy or degraded callers stay conservative.

### 2. Invariants that do NOT change

- **1% per-trade cap**: a planned risk above `risk_per_trade_amount` is never
  admitted, regardless of budget.
- **4% monthly budget**: hard invariant. Since latent risk is charged at
  admission and priced with execution costs, a sequence of stop-outs cannot
  breach it.
- **No static limits** (ADR-0024): no cap on concurrent positions, entries per
  day, or entries per month. The budget is the only constraint.

### 3. MonthlyHalt fires only on true exhaustion

`MonthlyHalt` triggers when `remaining_budget ≤ 0` (previously
`< risk_per_trade_amount`). Between zero and one full risk unit the system
stays live: a smaller planned-risk entry may still fit, and stops advancing to
breakeven may free latent budget.

A denial because *this specific trade* does not fit the remaining budget is a
new governed outcome, `risk_budget_insufficient`. It re-arms the detector with
the standard exponential backoff and **must not** trigger MonthlyHalt or close
positions — unlike `monthly_drawdown`, budget remains.

### 4. Slots become a guaranteed minimum, not a ceiling

`slots_available` (and the `/status` field `new_slots_available`) now means
**guaranteed full-cap entries remaining**: how many worst-case 1% trades the
budget still absorbs. It is a floor for display and communication. The actual
number of operations in a month can exceed it whenever trades risk less than
the cap.

### 5. Product framing

> **"No mínimo 4 chances por mês — risco economizado vira operação extra."**
> (At least 4 chances per month — saved risk becomes an extra operation.)

The guaranteed minimum of 4 is mathematically implied by the invariants
(4% budget ÷ 1% cap). Anything above 4 is earned by entries that risk less
than the cap — the product communicates the floor as the promise and the
extra operations as the reward.

## Consequences

### Positive

- Months with lower-risk entries support more operations at identical total
  risk — the product becomes more attractive without weakening any invariant.
- The budget tail (remaining < 1%) is usable instead of stranded.
- MonthlyHalt semantics get sharper: it now means "budget consumed", not
  "budget cannot fit a hypothetical worst-case trade".

### Negative / Trade-offs

- "How many slots do I have?" no longer has a fixed-total answer; the
  frontend slot-cell grid (ADR-0032/0034/0036) needs the budget bar
  (MIG-v3#14 Risk Dashboard) as the primary visualization, with slot cells as
  a derived floor.
- More entries per month means more fee churn; selectivity is no longer
  enforced by slot scarcity. Accepted deliberately — the operator decision
  (2026-07-04) is that the budget is the only constraint.
- Planned risk vs realized risk can still diverge on extreme gaps beyond the
  priced gap allowance; unchanged from ADR-0039, but with more entries the
  exposure to that tail is proportionally larger. The insurance stop layer
  (ADR-0039) remains the mitigation.

## Alternatives considered

### Keep 1% reservation, display budget only (rejected)

Purely cosmetic; leaves the budget tail stranded and the asymmetry in place.

### Admit by planned risk with a per-day entry cap (rejected)

Operator decision 2026-07-04: no operational ceiling on entries/day. A cap
would reintroduce a static limit ADR-0024 deliberately eliminated.

### Halt at remaining < smallest viable trade (rejected)

"Smallest viable trade" depends on symbol filters and price — not a stable
policy constant. `remaining ≤ 0` is exact, and the gate already denies trades
that do not fit.

## Related

- [ADR-0024](ADR-0024-trading-policy-layer.md) — trading policy layer, dynamic
  slots (Decision 5 superseded in part by this ADR)
- [ADR-0028](ADR-0028-entry-policy-strategy-engine.md) — entry policy engine
- [ADR-0034](ADR-0034-frontend-slot-count-api-only.md) — slot count from API
- [ADR-0036](ADR-0036-monthly-slot-inheritance-and-stop-visibility.md) —
  monthly slot semantics (visual model to be revisited for variable totals)
- [ADR-0039](ADR-0039-exchange-side-insurance-stop.md) — cost-priced sizing,
  insurance stop
- [ADR-0042](ADR-0042-invalidation-guard.md) — invalidation guard in effective
  stop
