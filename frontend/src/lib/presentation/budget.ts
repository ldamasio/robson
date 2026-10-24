import type { StatusResponse } from "$api/robson";

/// Monthly drawdown cap, as a percentage of `capital_base` (ADR-0024).
export const MONTHLY_BUDGET_LIMIT_PCT = 4;

/**
 * Which monthly budget accounting the backend is enforcing (ADR-0051 §4).
 *
 * - `net_from_start`: consumed = net governed loss since month start; profit
 *   does not expand the 4% and unrealized P&L never moves the gauge.
 * - `hwm`: consumed = give-back from the month's equity peak (ADR-0046).
 * - `legacy`: an older backend that only reports gross realized loss.
 */
export type BudgetModelKind = "net_from_start" | "hwm" | "legacy";

export const NET_FROM_START_MODEL = "net_from_start_non_expanding_v1";

export function budgetModelKind(
  status: StatusResponse | null,
): BudgetModelKind {
  if (status == null) return "legacy";
  if (status.monthly_budget_model === NET_FROM_START_MODEL) {
    return "net_from_start";
  }
  if (status.monthly_giveback_pct !== null) return "hwm";
  return "legacy";
}

export type BudgetView = {
  /** Accounting model the gauge is expressed in. */
  model: BudgetModelKind;
  /** Share of the monthly budget already consumed, on a 0-100 scale. */
  usedPct: number;
  /**
   * Drawdown as a percentage OF CAPITAL, the unit the 4% limit is expressed
   * in, so "X% of a 4.0% limit" compares like with like.
   */
  drawdownPct: number;
  /** Nothing left: the daemon blocks every new entry until the rollover. */
  exhausted: boolean;
  /** No full-cap slot fits, but a smaller entry still can. */
  low: boolean;
};

/**
 * Derive the monthly budget readouts from a status payload.
 *
 * The backend reports two budget metrics in DIFFERENT units, and conflating
 * them is what produced the 2026-07-31 dashboard defect:
 *
 * - `monthly_giveback_pct` is already a percentage OF THE BUDGET (0-100).
 * - `monthly_realized_loss_pct` is a percentage OF CAPITAL, and still has to
 *   be divided by the 4% limit to become a budget share.
 *
 * Running the first through the second's formula pinned the gauge at 100% and
 * raised the "budget exhausted" banner from roughly 3% of budget usage
 * onwards, while the daemon still had budget left and was still admitting
 * entries.
 */
export function deriveBudgetView(
  status: StatusResponse | null,
  insufficientCapital: boolean,
): BudgetView {
  const model = budgetModelKind(status);

  // `monthly_budget_utilization_pct` is the active model's own consumption
  // (ADR-0051 §4) and wins whenever the backend sends it: under
  // net-from-start the give-back field is null and the gross-loss fallback
  // would overstate a month whose losses were already offset by gains.
  const utilizationPct = status?.monthly_budget_utilization_pct ?? null;
  const rawUsedPct =
    utilizationPct !== null
      ? utilizationPct
      : model === "hwm"
        ? (status?.monthly_giveback_pct ?? 0)
        : ((status?.monthly_realized_loss_pct ?? 0) /
            MONTHLY_BUDGET_LIMIT_PCT) *
          100;
  const usedPct = Math.min(100, Math.max(0, rawUsedPct));

  const drawdownPct =
    utilizationPct !== null || model === "hwm"
      ? (usedPct / 100) * MONTHLY_BUDGET_LIMIT_PCT
      : (status?.monthly_realized_loss_pct ?? 0);

  // Exhausted only when nothing is left, which is exactly when the daemon
  // fires MonthlyHalt (ADR-0043: remaining_budget <= 0). Above zero, entries
  // whose planned risk fits are still admitted, so it must not be reported as
  // a hard block until the month rollover.
  const exhausted =
    status?.monthly_budget_remaining != null
      ? status.monthly_budget_remaining <= 0
      : false;

  // `new_slots_available === 0` is the guaranteed-full-cap floor, never a
  // ceiling (ADR-0043), so it alone does not mean Robson cannot operate.
  const low = insufficientCapital && !exhausted && usedPct > 0;

  return { model, usedPct, drawdownPct, exhausted, low };
}
