# ADR-0046 — Monthly High-Water-Mark Budget (Trailing Month)

**Date**: 2026-07-05
**Status**: Decided (implementation pending)
**Deciders**: RBX Systems (operator + architecture)

---

## Context

ADR-0024 made the monthly budget **gross**: only losses consume it, wins never
replenish (`realized_loss = Σ |losing closes|`). The month can only tighten.
After ADR-0043 (budget-metered admission) and the first profitable
insurance-stop close (2026-07-05), the operator decided gains must free
operating room — a month that wins early should not run out of chances.

Three designs were compared on the give-back axis:

| Model | Floor vs month start | Max give-back from month peak | Gains free budget? |
| --- | --- | --- | --- |
| Gross (ADR-0024) | −4% | ≤ 4% | No — month only tightens |
| Net-from-start | −4% | gains + 4% | Yes, fully |
| **High-water mark** | **−4%** | **≤ 4%, always** | **Yes — every new peak re-arms the 4% cushion** |

Net-from-start satisfies "gains free budget" but exposes realized profit: win
+3% and the month may give back 7% gross. The high-water mark applies the
system's own trailing-stop concept to the month itself: realized month profit
gets a floor, exactly like an open position's profit does.

## Decision

### 1. Budget is drawdown from the month's governed EQUITY peak

The high-water mark tracks **equity**, not just realized closes (operator
decision, 2026-07-05): realized P&L, unrealized P&L of open positions, fees,
and the risk reserved down to the stops all enter the math.

```
month_equity_net(t) = governed_realized_net(t)            # Σ (realized_pnl − fees), governed closes
                    + Σ unrealized_pnl(open positions at t)

month_peak_net      = max(0, running max of month_equity_net)   # persisted, monotonic

consumed            = month_peak_net − month_equity_net_now     # give-back so far
remaining_budget    = capital_base × 4% − consumed − latent_risk
```

`latent_risk` remains the loss-if-every-stop-fills from current marks
(ADR-0024 §5). Since unrealized P&L is inside `month_equity_net` and the
distance from mark to stop is reserved separately, `remaining_budget ≤ 0`
means precisely: *if every open stop fills now, the month's equity lands 4%
of capital base below its peak*. The cushion is therefore honored even in a
worst-case stop-out cascade.

- **MonthlyHalt** fires when `remaining_budget ≤ 0` (trigger shape unchanged,
  ADR-0043/0045).
- **Admission** (ADR-0043 `can_admit`) and **slots** (guaranteed-minimum
  floor) read the same `remaining_budget` — realized gains re-arm capacity
  and raise the monthly floor at the same time.
- **The 1% per-operation worst-case cap is untouched**: it remains the unit
  limit; the monthly trailing is the aggregate drawdown limit.

### 2. Guarantees (product contract)

- Give-back from the month's equity peak is **never more than 4% of capital
  base** — including unrealized peaks: profit a position showed on the screen
  gets the same class of floor the position's own trailing stop gives it.
- The month never ends below **−4% of capital base** (peak ≥ 0 always).
- Accepted consequence: the month can halt while still net-positive (peak
  +3%, then −4% from peak halts at −1%). The user-facing phrasing: *"você
  nunca devolve mais de 4% do topo do mês"*.

### 3. Peak tracking and persistence

Because the peak includes unrealized P&L, it cannot be recomputed from closed
positions alone:

- The running peak updates wherever equity is already marked: on every
  monitor tick (20 s cadence, which already computes unrealized P&L) and on
  every governed close.
- `month_peak_net` persists in `monthly_state` (new column, migration) and
  only ever increases within a month. On restart the daemon resumes from the
  persisted peak — conservative by construction (a peak reached during an
  outage window that was never observed is at worst under-counted, never
  over-counted; the 20 s cadence bounds the observation gap).
- Out-of-band drift stays excluded — the peak moves only on governed flow
  (ADR-0045 discipline): governed closes plus marks of robsond-authored open
  positions, never raw wallet balance.

### 4. Reporting split (FE contract)

- `GOVERNED REALIZED LOSS` (gross Σ of losing closes) remains as an
  informational metric — it is no longer the limit driver.
- The `MONTHLY LIMIT` gauge becomes **give-back from the equity peak**:
  `consumed / (capital_base × 4%)`.
- `/status` gains: `month_equity_net`, `month_peak_net`,
  `monthly_giveback_pct`, `monthly_budget_remaining`. Slot fields keep their
  ADR-0043 semantics.

### 5. Month boundary

Unchanged from ADR-0024 §6: at the boundary, capital base re-snapshots from
equity and the accumulators reset — `governed_net = 0`, `month_peak_net = 0`.

## Failure modes

| Failure | Behavior |
| --- | --- |
| Closed-positions projection unavailable | Realized-net computation falls back to in-memory month state (same fallback path as `robson_month_net`); alarm on divergence |
| Market data silent (no marks) | Unrealized P&L freezes at last mark; the peak cannot inflate, latent risk stays reserved — conservative; ADR-0044 fallback restores marks |
| Daemon outage across an unrealized peak | The unobserved peak is under-counted (never over-counted); the 20 s persistence cadence bounds the gap; the position's own trailing stop still floors the realized outcome |
| Unreconciled close in flight | Its P&L is not yet governed — peak and net do not move until the evidenced close lands (ADR-0045 hotfix already blocks drift absorption in that window) |
| Clock/month-boundary restart bug (known) | Peak resets with the same `MonthBoundaryReset` machinery; the pre-existing skip bug affects this ADR no differently |

## Trade-offs — what we are leaving on the table

- A hot month keeps trading on its own winnings; total *gross* loss across a
  month is no longer capped at 4% (it is capped at `peak + 4%`, which the
  peak itself earned). We give up the "month only tightens" austerity.
- Halting while positive will surprise users at least once; the FE copy must
  carry the high-water-mark explanation, not bury it.

## Alternatives considered

- **Net-from-start** (rejected): give-back of `gains + 4%` contradicts the
  instinct that made the operator adopt trailing stops in the first place.
- **Keep gross** (rejected): a winning month starves; contradicts the ADR-0043
  product direction.
- **Net with a separate gross cap** (rejected): two coupled limits to explain,
  no additional guarantee beyond what the high-water mark already gives.

## Supersedes / Related

- Supersedes ADR-0024's "wins do not offset losses" clause for budget
  purposes (the gross metric survives as reporting).
- Refines ADR-0043 (`remaining_budget` input becomes give-back from peak).
- [ADR-0045](ADR-0045-income-ledger-reconciliation.md) — governed-only flow
  feeding the peak.
