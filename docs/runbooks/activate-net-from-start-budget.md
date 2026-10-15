# Activate the net-from-start monthly budget (ADR-0051)

**Scope**: switch the persisted monthly budget model of the current month
from `hwm_v1` (ADR-0046 high-water mark) to
`net_from_start_non_expanding_v1` (ADR-0051), without resetting the month or
changing its audited `capital_base`.

**Why**: under `hwm_v1` a net-positive month with an empty book can show
fewer than 4 slots, because the give-back from an intra-trade equity peak is
charged against the 4% limit (August 2026 case in ADR-0051; repeated on
2026-10-15 with 2 slots on a month at +83.50 USDT). ADR-0051 is the operator
decision that profit is protected by each position's trailing stop, not by
the monthly budget, and that such a month shows the full 4 slots.

**Time**: 10 minutes. **Risk**: low, reversible by the inverse UPDATE. The
daemon reads `monthly_state.monthly_budget_model` on every canonical budget
snapshot, so no restart or deploy is needed for the switch itself.

## What the switch changes, in one table

| | `hwm_v1` (before) | `net_from_start_non_expanding_v1` (after) |
|---|---|---|
| Consumed | `month_peak_net − month_equity_net` (give-back from the peak, unrealized included) | `max(0, −governed_realized_net)` (net realized loss since month start) |
| Remaining | `4% × capital_base − consumed − latent_risk` | same formula, different `consumed` |
| Slots | `min(4, floor(remaining / 1% × capital_base))` | same formula |
| MonthlyHalt | `remaining ≤ 0` | same trigger |
| Profit | each new peak re-arms the 4% cushion | gains offset earlier losses down to zero, never expand the 4% |
| Floor | never more than 4% below the month's peak | never more than 4% below month start |

Accepted consequence (ADR-0051 trade-off): realized month profit is **not**
protected by the monthly mechanism. With `governed_realized_net = +83.50` and
`capital_base = 1,710.81`, the month can give back up to
`83.50 + 68.43 ≈ 152 USDT` before MonthlyHalt binds.

## Prerequisites

- `kubectl` with the production kubeconfig (`KUBECONFIG=~/.kube/config-rbx`)
  and access to the `robson` namespace.
- Daemon image and frontend at or after the release that made `/status` and
  the MONTHLY LIMIT card model-aware (`monthly_budget_utilization_pct`,
  HWM fields withheld under net-from-start) and that enforces the
  non-expanding basis (`MonthlyBudgetModel::allows_capital_base_change`:
  under net-from-start a confirmed deposit or manual drift can no longer
  raise `capital_base` intra-month; decreases still apply). Verify:

  ```bash
  curl -s https://api.robson.rbx.ia.br/status | jq '{monthly_budget_model, monthly_budget_utilization_pct, monthly_giveback_pct}'
  ```

  `monthly_budget_utilization_pct` must be present. If it is `null`/absent,
  the deployed daemon predates this runbook: stop.

## Preconditions (ADR-0051 §5 step 4), all must hold

Run from a shell with the production kubeconfig.

0. **Policy gate.** ADR-0051 §5.4 lists blockers that this runbook cannot
   remove (see item 5). Activation requires the operator to have approved
   the "Activation exceptions" amendment in ADR-0051 (status line of the
   ADR says so, with a date). Without that approval, stop here.

1. **Account flat, nothing in flight, halt not latched.**

   ```bash
   curl -s https://api.robson.rbx.ia.br/status | jq '{active_positions, occupied_slots, stale_active_count, pending_approvals: (.pending_approvals|length), reconciliation_blockers: (.reconciliation_blockers|length), monthly_open_risk_reserved}'
   curl -s https://api.robson.rbx.ia.br/monthly-halt
   ```

   Expected: every count `0`, `monthly_open_risk_reserved == "0"`, and
   `monthly-halt.state == "active"` (normal operation). A latched halt is
   **not** cleared by switching models: the budget and slots would read
   healthy while arming stays rejected until the month rollover.

   ```bash
   PARADEDB_POD=$(kubectl get pod -n robson -l app.kubernetes.io/name=robson-paradedb -o jsonpath='{.items[0].metadata.name}')
   kubectl exec -n robson "$PARADEDB_POD" -- psql -U robson -d robson -c \
     "SELECT position_id, symbol, side, state FROM positions_current WHERE state IN ('armed', 'entering', 'active', 'exiting') ORDER BY updated_at DESC;"
   ```

   Expected: zero rows. Also confirm on Binance that the USDⓈ-M account has
   no open position and no open order for the traded symbols.

2. **Income ledger reconciled with evidence, not just acknowledged.**
   `unmatched_income_count` in `/status` excludes acknowledged rows and rows
   inside the evidence-lag grace period, and reads `0` when the query
   fails, so it is necessary but not sufficient. Check the ledger directly:

   ```bash
   kubectl exec -n robson "$PARADEDB_POD" -- psql -U robson -d robson -c \
     "SELECT COUNT(*) AS unmatched_any_age FROM income_ledger WHERE matched_at IS NULL AND acked_at IS NULL;"
   kubectl exec -n robson "$PARADEDB_POD" -- psql -U robson -d robson -c \
     "SELECT income_type, COUNT(*), SUM(amount) FROM income_ledger WHERE income_time >= date_trunc('month', now() AT TIME ZONE 'UTC') AND (matched_at IS NULL) GROUP BY 1 ORDER BY 1;"
   ```

   Expected: `unmatched_any_age == 0` (acknowledge with
   `robson-cli income ack <id>` only after the item is understood, ADR-0045).
   The second query lists this month's items that are **outside governed
   net** (acknowledged items and funding). Their sum is the settlement gap
   the budget does not see; write it down. If `|sum|` exceeds 10% of the
   month's remaining budget, do not activate until it is evidenced.

3. **Governed net cross-check.** `month_governed_realized_net` must agree
   with the closed-position projection for the month:

   ```bash
   curl -s https://api.robson.rbx.ia.br/status | jq '{month_governed_realized_net, monthly_net_loss_consumed, monthly_budget_amount, monthly_budget_remaining, new_slots_available}'
   kubectl exec -n robson "$PARADEDB_POD" -- psql -U robson -d robson -c \
     "SELECT COUNT(*) AS closes, SUM(realized_pnl) AS pnl, SUM(total_fees) AS fees, SUM(realized_pnl - total_fees) AS net FROM positions_current WHERE state = 'closed' AND closed_at >= date_trunc('month', now() AT TIME ZONE 'UTC');"
   ```

   Expected: `net` equals `month_governed_realized_net` within rounding.

4. **Shadow sanity.** The dormant release computes both models on every
   snapshot and exports `robsond_budget_model_shadow_slots_delta =
   slots_nfs − slots_hwm` (the metric keeps that meaning after activation;
   it does not go to zero). Compute by hand from the values above:
   `remaining_nfs = monthly_budget_amount − max(0, −month_governed_realized_net)
   − monthly_open_risk_reserved`; `slots_nfs = min(4, floor(remaining_nfs /
   (capital_base × 1%)))`. The metric must equal
   `slots_nfs − new_slots_available`.

   ```bash
   curl -s https://api.robson.rbx.ia.br/metrics | grep robsond_budget_model_shadow_slots_delta
   ```

5. **Known gaps versus ADR-0051 §5.4** (covered by the amendment in item 0,
   never silently):

   - `month_governed_realized_net` excludes funding and other typed income
     (settlement completeness, §3). Bounded by item 2; at the operator's
     sizes (≤ 0.02 BTC, intraday to a few days) funding is cents per 8 h.
   - `trigger_monthly_halt` cannot cancel an `Entering` order. The switch
     happens on a flat book, so no cutover exposure; the gap applies to
     later entries under both models and is tracked separately.
   - The August 2026 ledger discrepancy (+43.63 closes vs 40.63 month net)
     is not re-opened by this switch; October has its own basis.
   - Non-expanding basis (§1) is enforced from the release named in
     Prerequisites; before it, a confirmed deposit could raise the basis.

## Procedure

1. Read the current row and keep the output (it is the rollback evidence):

   ```bash
   kubectl exec -n robson "$PARADEDB_POD" -- psql -U robson -d robson -c \
     "SELECT year, month, capital_base, realized_loss, month_peak_net, trades_opened, monthly_budget_model, boundary_reset_at FROM monthly_state ORDER BY year DESC, month DESC LIMIT 3;"
   ```

2. Switch the **current** month only. The `WHERE` guard makes the statement
   idempotent and refuses to touch a row that is not on `hwm_v1`:

   ```bash
   kubectl exec -n robson "$PARADEDB_POD" -- psql -U robson -d robson -c \
     "UPDATE monthly_state SET monthly_budget_model = 'net_from_start_non_expanding_v1' WHERE year = EXTRACT(YEAR FROM now() AT TIME ZONE 'UTC')::smallint AND month = EXTRACT(MONTH FROM now() AT TIME ZONE 'UTC')::smallint AND monthly_budget_model = 'hwm_v1' RETURNING year, month, monthly_budget_model;"
   ```

   Expected: `UPDATE 1` with the current `year, month`. `UPDATE 0` means the
   row is already switched or does not exist; investigate before retrying.

   Later months inherit the model: every `monthly_state` insert path
   (`MonthBoundaryReset` projector, `CapitalBaseRecalibrated`, entry/close
   counters, the daemon's peak upsert) copies the most recent earlier row's
   `monthly_budget_model`, and no `ON CONFLICT` clause overwrites it.

3. No restart. The next `/status` snapshot (and the next admission or halt
   evaluation) reads the new model.

## Validation

- [ ] `/status` reports the new model and the HWM outputs are withheld:

  ```bash
  curl -s https://api.robson.rbx.ia.br/status | jq '{monthly_budget_model, new_slots_available, slot_cells_total, monthly_budget_utilization_pct, monthly_budget_remaining, month_peak_net, monthly_giveback_pct, monthly_net_loss_consumed}'
  ```

  Expected on a net-positive empty book: `new_slots_available == 4`,
  `monthly_budget_utilization_pct == "0"`, `month_peak_net == null`,
  `monthly_giveback_pct == null`.
- [ ] `robsond_budget_model_shadow_slots_delta` still equals
  `slots_nfs − slots_hwm` (now `new_slots_available − slots_hwm`); a
  non-zero value is expected whenever the month has a give-back from its
  peak. It is informational after activation.
- [ ] Dashboard MONTHLY LIMIT card shows the net-from-start copy ("Net
  governed loss since month start · Profits do not raise the limit") with
  `net / reserved / left` instead of `peak / now`, and the SLOTS row shows
  4 free cells.
- [ ] Daemon log shows no `Unknown persisted monthly budget model` warning.
- [ ] First admitted entry after the switch: an **Armed** position reserves
  nothing (arm is not admission); once it is `Entering`/`Active`,
  `monthly_open_risk_reserved` equals its priced worst-case risk and
  `new_slots_available` reflects `floor((remaining − reserved) / risk_unit)`.
- [ ] Deposit path (only if one happens): a confirmed TRANSFER does **not**
  raise `capital_base`; the daemon logs
  `Capital base increase withheld: non-expanding monthly budget basis` and
  `WALLET BALANCE` moves while `capital_base` stays.

## Rollback

Inverse statement, same guard, no restart:

```bash
kubectl exec -n robson "$PARADEDB_POD" -- psql -U robson -d robson -c \
  "UPDATE monthly_state SET monthly_budget_model = 'hwm_v1' WHERE year = EXTRACT(YEAR FROM now() AT TIME ZONE 'UTC')::smallint AND month = EXTRACT(MONTH FROM now() AT TIME ZONE 'UTC')::smallint AND monthly_budget_model = 'net_from_start_non_expanding_v1' RETURNING year, month, monthly_budget_model;"
```

`month_peak_net` keeps being refreshed under both models (the peak upsert is
model-agnostic), so a rollback resumes the HWM accounting from the true peak,
not from zero. If rows for later months were created in between, run the
same UPDATE for each of them.

## Related documentation

- [ADR-0051](../adr/ADR-0051-net-from-start-monthly-budget.md), §5 rollout
- [ADR-0046](../adr/ADR-0046-monthly-high-water-mark-budget.md), superseded
- [ADR-0043](../adr/ADR-0043-budget-metered-entry-admission.md), slots and
  admission formula
- [ADR-0045](../adr/ADR-0045-income-ledger-reconciliation.md), income ack
