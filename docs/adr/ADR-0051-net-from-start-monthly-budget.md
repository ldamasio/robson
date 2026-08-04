# ADR-0051 — Net-From-Start, Non-Expanding Monthly Budget

**Date**: 2026-08-04
**Status**: DECIDED — FOLLOW-UP REQUIRED (operator decision, 2026-08-04;
revised same day after adversarial secondary-model review; implementation and
operational rollout pending)
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
- `governed_realized_net` be the signed sum of settlement-complete governed
  operation results attributable to the current monthly basis by economic
  event timestamp. It includes evidenced realized P&L, commissions, funding,
  and other typed governed costs; it excludes unrealized P&L and out-of-band
  account drift;
- `latent_risk` be the non-negative aggregate residual cost-priced worst-case
  loss reserved from atomic admission until settlement completion — the
  pessimistic assumption of Context item (4) made mechanical. It covers the
  full committed quantity and unfilled remainder across Entering, partially
  filled, Active, Exiting, missing-on-exchange/close-in-reconciliation, and
  every other risk-bearing or unsettled governed lifecycle state, plus
  bounded liabilities not yet included in `governed_realized_net`. Carried
  positions use the rebased month-boundary treatment in §3.

For this ADR, `capital_base` is the monthly budget basis fixed at activation
or the UTC month boundary. It MUST NOT increase during that month. A
confirmed deposit may update margin and sizing capital, but it does not
expand the current monthly budget. A confirmed withdrawal or other adverse
account change may conservatively reduce the effective budget basis,
preserving accumulated governed net and immediately re-evaluating
MonthlyHalt.

This supersedes ADR-0024 §6A, ADR-0038, and ADR-0045 only to the extent that
they permit an intra-month increase of the monthly budget basis. Their typed
evidence, audit-event, drift, and fail-closed requirements remain in force.

The authoritative calculation is:

```
consumed          = max(0, −governed_realized_net)
remaining_budget  = monthly_budget − consumed − latent_risk
slots_available   = min(4, floor(max(0, remaining_budget) / risk_unit))
```

All policy amounts use `Decimal`; displayed or rounded values never feed
admission or halt decisions. When quantization is unavoidable, risk charges
and reservations round conservatively upward and available budget rounds
conservatively downward using runtime exchange metadata.

`latent_risk` and every risk charge MUST be non-negative. If
`capital_base ≤ 0`, the snapshot is invalid: `slots_available = 0`, admission
fails closed, MonthlyHalt latches, and the `risk_unit` division is not
evaluated. A high-severity accounting alarm is raised.

`remaining_budget` remains signed for admission and halt. Only slot reporting
clamps it to zero. Therefore, when
`0 < remaining_budget < risk_unit`, `slots_available = 0`, but ADR-0043 still
permits a priced trade whose risk is no greater than `remaining_budget`.

Realized gains can offset prior realized losses until net reaches zero.
Gains above zero do not increase the monthly budget. Unrealized P&L never
changes the monthly budget, in either direction.

**Safety argument.** Let `N = governed_realized_net`, let `L` be
`latent_risk` in the pre-admission canonical snapshot, and let `A` be only the
transaction-local aggregate risk of the proposal or proposals evaluated
against that snapshot. Serialized admission requires
`A + L ≤ monthly_budget − consumed` and, on success, atomically commits the
post-state `latent_risk = L + A` before admission becomes externally visible.
Worst-case final governed net is `N − L − A`. If `N ≥ 0`, this is at least
`N − monthly_budget ≥ −monthly_budget`; if `N < 0`, then
`L + A ≤ monthly_budget + N`, so the result is again at least
`−monthly_budget`. The proof needs no unrealized term, which is precisely why
unrealized P&L is excluded from the anchor.

**August 2026 under this model**: consumed $0.00, open risk reserved $0.00,
remaining $63.12, **4 slots**.

### 2. Admission and halt use one canonical snapshot

Let `effective_planned_risk` equal `planned_risk` when `planned_risk > 0`,
and `risk_unit` otherwise (ADR-0043 unpriced fallback). Admission requires:

```
capital_base > 0
0 < effective_planned_risk ≤ risk_unit
effective_planned_risk ≤ remaining_budget
```

The snapshot check and addition of `effective_planned_risk` to `latent_risk`
as a durable reservation are one serialized, atomic state transition.
Concurrent proposals and concurrent accounting or position updates cannot
spend the same remaining budget; a snapshot-version change forces admission
to retry. A successful admission is not externally visible until its
reservation is included in the canonical `latent_risk` snapshot. Lifecycle
transitions atomically replace the basis of that same reservation; no
transition may leave an interval where the risk appears in both
representations or in neither.

MonthlyHalt triggers when `remaining_budget ≤ 0`. It latches, blocks new
entries, and closes risk-bearing positions. The −4% guarantee applies within
the execution cost and gap envelope used by planned-risk and latent-risk
pricing (ADR-0039); a breach outside that envelope keeps MonthlyHalt latched
and raises a high-severity, durable reconciliation alarm.

Admission, slot reporting, the status API, and MonthlyHalt must consume one
canonical monthly-budget snapshot, produced by a constant-count bulk read
(consumers never perform per-position database or exchange calls). Active,
Entering, partially filled, and close-in-reconciliation positions may not be
treated differently across those paths. (This closes a live inconsistency:
`evaluate_monthly_halt` reserves latent risk for Active positions only,
while `compute_slots_available` and `monthly_budget_snapshot` also count
Entering.)

```
typed income + governed fills + durable lifecycle/reservations + basis/model
                                  |
                                  v
                    CanonicalMonthlyBudgetSnapshot
                        K, N, L, B, R, slots
                         /      |       \
                admission    /status    slots/halt
```

### 3. Accounting lifecycle

A governed operation is settlement-complete only when authoritative evidence
for its realized P&L, commissions, attributable funding, and every other
typed cost required by its settlement contract is durable and matched. Late
evidence is attributed by economic event timestamp, not ingestion or
daemon-restart time.

Until settlement is complete, no partial result may create admission
capacity: the operation's reservation and every bounded, unprojected
liability remain in `latent_risk`. A single atomic transaction removes that
reservation and adds the complete signed result to `governed_realized_net`.
No lifecycle transition may leave an interval where neither is charged.
Production must not fall back to an incomplete in-memory close projection.

If authoritative accounting is unavailable, stale beyond its contract, or an
unsettled liability cannot be bounded, new entries fail closed. Existing
positions remain protected and managed, and the failure is visible through a
durable alarm and status state.

`robson_month_net` retains its raw signed semantics for drift attribution
(ADR-0045); budget accounting reads a separate settlement-complete
projection. Out-of-band account changes never enter governed monthly net.

At the UTC month boundary, let `carried_downside` be the aggregate
cost-priced downside from boundary mark-to-market equity to each carried
operation's pessimistic stop fill, including the execution-cost envelope.
Then:

```
new_capital_base = current_governed_equity − carried_downside
```

Persist a pessimistic boundary basis for every carried operation. The
absorbed `carried_downside` is not also charged as new-month `latent_risk`;
only adverse movement beyond the persisted basis is reserved. When a carried
operation settles, only its result relative to that boundary basis enters
the new month's `governed_realized_net` — never its full lifetime P&L. The
model version, new capital base, reset ledger, and carried bases are
committed atomically, durably, and idempotently. A non-positive result
follows the invalid-base rule in §1.

### 4. Peak state is retired

`month_peak_net` is no longer an input to risk decisions or reporting.
`refresh_month_peak_net` and its cache are removed after the compatibility
window. The database column is frozen at cutover and dropped in a later
migration.

When `net_from_start_non_expanding_v1` is active, `/status` exposes
`monthly_budget_model`, `month_governed_realized_net`,
`monthly_net_loss_consumed`, `monthly_net_loss_pct_of_base`,
`monthly_open_risk_reserved`, `monthly_budget_amount`,
`monthly_budget_remaining`, `monthly_budget_utilization_pct`, and
`new_slots_available`.

`monthly_realized_loss` may remain as a clearly labelled gross informational
metric. Mark-to-market month equity, if retained, is exposed as
`month_mark_to_market_net` and explicitly marked informational. HWM fields
are served only while `hwm_v1` or the compatibility window is active and are
removed during cleanup.

Frontend card copy: "MONTHLY NET-LOSS BUDGET · net governed loss since month
start, plus risk reserved for open positions. Profits do not increase the 4%
limit."

### 5. Rollout (live production)

1. **DB expansion**: add `monthly_budget_model` to `monthly_state`,
   backfilling existing rows as `hwm_v1`; add the durable settlement
   projection and per-position carried-basis state required above. `hwm_v1`
   is an expansion default only. After activation, every `MonthBoundaryReset`
   inherits the active model atomically so a new row cannot revert to HWM.
2. **Daemon, dormant dual-model release**: one canonical snapshot function
   shared by admission/status/slots/halt; shadow-calculate both models and
   alarm on unexplained divergence.
3. **Frontend**: model-aware rendering of the new card; stop depending on
   peak/give-back fields.
4. **Activation**: require a healthy canonical-snapshot shadow period and
   repository-verified handling for every reservation lifecycle, including
   cancellation of an Entering order and exit of its filled portion (today
   `trigger_monthly_halt` documents that Entering positions cannot be
   cancelled; activation cannot proceed while that remains true). Reconcile
   the August ledger gap (governed closes sum +$43.63 versus month net
   $40.63; the approximately $3.00 must be evidenced), and verify the
   account is flat: no risk-bearing local or exchange position, no in-flight
   entry/exit or protective order, no stale reconciliation item, and no
   unsettled governed liability. Then atomically switch the persisted model
   to `net_from_start_non_expanding_v1` without resetting the month or
   changing its audited budget basis.
5. **Cleanup**: remove peak refresh/cache/API code, then drop
   `monthly_state.month_peak_net` in a separate migration.

## Failure modes

| Failure | Required behavior |
| --- | --- |
| Postgres/projection unavailable | No new entries; no incomplete in-memory production fallback; existing protection continues; durable visible alarm. |
| Income endpoint unavailable or stale | Settlement reservations remain; new entries fail closed after the freshness bound; retry with backoff. |
| Exchange position inventory unavailable | Use the full durable local risk-bearing set conservatively; do not omit positions; deny new entries and expose staleness. |
| Concurrent admissions/accounting updates | Serialized reservation or versioned compare-and-swap; loser retries from a new snapshot. |
| Daemon dies between check and reservation | Both commit or neither commits; startup rebuilds the identical snapshot before admission resumes. |
| Close evidence incomplete | Reservation remains until complete evidence atomically replaces it. |
| Crash during month reset | Idempotent transaction restores one model/base/basis set and never charges carried downside twice. |
| Loss outside the priced execution envelope | MonthlyHalt remains latched and a high-severity reconciliation alarm is durable. |

## Consequences

- Positive: a settlement-complete profitable month with no risk-bearing
  positions or unsettled liabilities shows 4 guaranteed slots. Unrealized
  mark changes alone cannot change consumed budget, remaining budget, slots,
  admission, or halt; governed settlements and reservation lifecycle changes
  still can.
- Positive: no persisted peak state; the −4% floor has a closed-form proof
  under the normative premises of §§1–3.
- Accepted consequence inherited from ADR-0043: MonthlyHalt can still
  trigger while `governed_realized_net ≥ 0` when `latent_risk` alone
  exhausts the budget. In particular, an admission that exactly reserves the
  final budget amount produces `remaining_budget = 0` and satisfies the
  `≤ 0` halt trigger. What disappears is halting caused solely by give-back
  from a realized or unrealized monthly peak.
- Trade-off (accepted knowingly): realized gains are not protected by the
  monthly mechanism. Win +3% early and the month may give those gains back
  before the −4% floor binds. Per operator decision, protecting profit is
  the job of each position's trailing stop, not of the monthly budget.

## Supersession boundaries

This ADR supersedes ADR-0046's high-water-mark anchor, peak persistence,
peak/give-back API contract, HWM-specific failure behavior, and rejection of
net-from-start. ADR-0046's gross governed realized-loss value may remain
only as a separately labelled informational metric.

For budget decisions, this ADR supersedes ADR-0024's gross-loss,
wins-do-not-offset formula and its "four errors" framing. It preserves and
refines ADR-0024's month-boundary rule as specified in §3.

For the monthly budget basis, this ADR also supersedes ADR-0024 §6A,
ADR-0038, and ADR-0045 only to the extent that they permit a confirmed
transfer or other recalibration to increase that basis intra-month. Confirmed
transfers may update margin and sizing capital, and adverse changes may
conservatively reduce the effective budget basis, but no intra-month event
may expand the current monthly budget. Their typed-evidence, audit-event,
drift, and fail-closed requirements remain in force.

ADR-0043 remains authoritative for actual-planned-risk admission, the
full-cap guaranteed-minimum meaning of slots, the ability to admit a smaller
trade while slots are zero, and the `remaining_budget ≤ 0` MonthlyHalt
threshold.

ADR-0045 remains authoritative for typed income evidence, item-level
reconciliation, raw drift attribution, and the prohibition on absorbing
unexplained residuals. Under `net_from_start_non_expanding_v1`, ADR-0045's
"income endpoint unavailable: trading unaffected" behavior is superseded for
new-entry admission: stale or unavailable authoritative accounting fails
closed. Exit and protective-stop management continue.

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

### Decision matrix

| Option | Budget anchor | Unrealized P&L affects budget? | Treatment of gains | Principal trade-off | Decision |
| --- | --- | --- | --- | --- | --- |
| Net-from-start, non-expanding | Settlement-complete governed realized net plus latent risk | No | Offsets prior realized losses down to zero; never expands capacity above 4% | Realized profit may later be given back before the −4% floor binds | Accepted |
| Equity-net-from-start | Governed realized plus unrealized equity net | Yes | Paper profit can free capacity | An unrealized winner can mask realized loss and breach the month-start floor | Rejected |
| Realized-only HWM | Peak of settlement-complete realized net | No | Every realized peak re-arms and protects capacity | Late fees or costs recreate peak give-back behavior rejected by the operator | Rejected |
| Partial profit-lock | Net-from-start with a configurable retained-profit fraction | No | Partially protects realized gains | Adds an unapproved policy parameter and explanation burden | Rejected for now |
| ADR-0046 equity HWM | Governed equity peak | Yes | Every equity peak re-arms and protects capacity | Protects intra-trade peaks instead of only the month-start loss floor | Rejected |

## Implementation Notes

- Code paths: `robsond/src/position_manager.rs`
  (`compute_slots_available`, `evaluate_monthly_halt`,
  `monthly_budget_snapshot`, `refresh_month_peak_net`, `month_equity_net`,
  `governed_monthly_realized_loss`, `trigger_monthly_halt`),
  `robsond/src/daemon.rs` (month-boundary capital base),
  `robsond/src/income_ledger.rs` (recalibration; must stop raising the
  budget basis intra-month per §1), `robson-domain/src/policy.rs`
  (`TradingPolicy::slots_available`), `robsond/src/api.rs` (`/status`
  fields), `frontend` dashboard MONTHLY LIMIT card.
- Known pre-existing gaps this ADR turns into activation blockers: Entering
  positions cannot be cancelled by `trigger_monthly_halt` (documented in
  code); carried Active positions are absorbed into the new capital base at
  the boundary and then counted again by `compute_slots_available` (no
  per-position carried basis exists yet).
- Test invariants to pin: `0 ≤ consumed`; `remaining ≤ monthly_budget`;
  `slots ≤ 4`; unrealized mark changes alone never change consumed,
  remaining, slots, admission, or halt; every admitted sequence within the
  priced envelope ends ≥ −4%; `Entering → Active → Closed` never leaves an
  interval where neither latent risk nor settled result is charged;
  admission/status/slots/halt read byte-equivalent snapshots; restart
  reproduces the same snapshot; month reset idempotent, carried risk charged
  exactly once; out-of-band drift never enters governed net;
  `capital_base ≤ 0` never divides and always denies and halt-latches;
  `0 < remaining_budget < risk_unit` reports zero slots but may admit a
  smaller priced trade; exact final-budget reservation exercises the
  `remaining = 0` halt behavior; conservative Decimal/quantization boundary
  cases; a property-based invariant over concurrent admissions, settlement
  updates, and lifecycle transitions proving no budget is double-spent;
  carried-position closure contributes only its delta from the persisted
  pessimistic boundary basis; model selection survives the next month
  boundary; database, income, and exchange outages exercise the documented
  fail-closed behavior.
- Highest-value regression tests: (a) August case — flat account, positive
  net, 4 slots; (b) masked-loss case — realized −1%, unrealized winner with
  breakeven stop, remaining reflects the realized loss and marks do nothing;
  (c) Entering-position consistency across all four consumers; (d) atomic
  close settlement (reservation held until fees/funding evidenced);
  (e) month-boundary restart idempotency.
- Related: ADR-0024 (policy layer, month boundary), ADR-0038 (recalibration,
  partially superseded per §1), ADR-0039 (cost-priced worst case), ADR-0043
  (planned-risk admission, slots as guaranteed minimum, halt at ≤ 0),
  ADR-0045 (governed-only flow, partially superseded per Supersession
  boundaries), ADR-0046 (superseded).
