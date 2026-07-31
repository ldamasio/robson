import { describe, it, expect } from "vitest";
import {
  deriveBudgetView,
  MONTHLY_BUDGET_LIMIT_PCT,
} from "$lib/presentation/budget";
import type { StatusResponse } from "$api/robson";

function makeStatus(overrides: Partial<StatusResponse> = {}): StatusResponse {
  return {
    active_positions: 0,
    positions: [],
    pending_approvals: [],
    stale_active_count: 0,
    reconciliation_blockers: [],
    occupied_slots: 0,
    new_slots_available: 0,
    slot_cells_total: 0,
    monthly_realized_loss: 0,
    monthly_realized_loss_pct: 0,
    capital_base: 10000,
    wallet_balance: 10000,
    month_equity_net: null,
    month_peak_net: null,
    monthly_giveback_pct: null,
    monthly_budget_remaining: null,
    ...overrides,
  };
}

/**
 * Numbers taken verbatim from the 2026-07-31 production status payload, the
 * one that raised "Monthly risk budget exhausted" while the daemon still had
 * budget left and `blocks_new_entries: false`.
 */
const PROD_2026_07_31 = makeStatus({
  monthly_realized_loss: 51.3695871,
  monthly_realized_loss_pct: 3.1818316623275953,
  capital_base: 1614.46589737,
  wallet_balance: 1558.44685972,
  month_equity_net: -46.89377085,
  month_peak_net: 14.16462915,
  monthly_giveback_pct: 94.54891568082277,
  monthly_budget_remaining: 3.5202358948,
});

describe("deriveBudgetView", () => {
  it("treats monthly_giveback_pct as a share of the budget, not of capital", () => {
    const view = deriveBudgetView(PROD_2026_07_31, true);

    // 94.55% of the budget consumed, NOT 100%.
    expect(view.usedPct).toBeCloseTo(94.5489, 3);
    // Which is 3.78% of capital, against the 4% limit.
    expect(view.drawdownPct).toBeCloseTo(3.782, 3);
    expect(view.drawdownPct).toBeLessThan(MONTHLY_BUDGET_LIMIT_PCT);
  });

  it("does not report the budget as exhausted while the daemon still admits entries", () => {
    const view = deriveBudgetView(PROD_2026_07_31, true);

    expect(view.exhausted).toBe(false);
    // It is nearly gone, which is a different, honest statement.
    expect(view.low).toBe(true);
  });

  it("reports exhausted exactly when nothing is left, matching MonthlyHalt", () => {
    expect(
      deriveBudgetView(makeStatus({ monthly_budget_remaining: 0 }), true)
        .exhausted,
    ).toBe(true);
    expect(
      deriveBudgetView(makeStatus({ monthly_budget_remaining: -1.5 }), true)
        .exhausted,
    ).toBe(true);
    expect(
      deriveBudgetView(makeStatus({ monthly_budget_remaining: 0.01 }), true)
        .exhausted,
    ).toBe(false);
  });

  it("never reports both exhausted and low at once", () => {
    const view = deriveBudgetView(
      makeStatus({ monthly_budget_remaining: 0, monthly_giveback_pct: 100 }),
      true,
    );

    expect(view.exhausted).toBe(true);
    expect(view.low).toBe(false);
  });

  it("keeps the legacy metric in percent of capital when the backend omits HWM fields", () => {
    const view = deriveBudgetView(
      makeStatus({ monthly_realized_loss_pct: 2 }),
      true,
    );

    // 2% of capital is half of the 4% budget.
    expect(view.usedPct).toBeCloseTo(50, 6);
    expect(view.drawdownPct).toBeCloseTo(2, 6);
  });

  it("clamps the gauge to 0-100", () => {
    expect(
      deriveBudgetView(makeStatus({ monthly_giveback_pct: 140 }), false).usedPct,
    ).toBe(100);
    expect(
      deriveBudgetView(makeStatus({ monthly_giveback_pct: -10 }), false).usedPct,
    ).toBe(0);
  });

  it("stays quiet when capital is sufficient", () => {
    const view = deriveBudgetView(PROD_2026_07_31, false);

    expect(view.low).toBe(false);
    expect(view.exhausted).toBe(false);
  });

  it("handles a null status without throwing", () => {
    const view = deriveBudgetView(null, false);

    expect(view.usedPct).toBe(0);
    expect(view.drawdownPct).toBe(0);
    expect(view.exhausted).toBe(false);
    expect(view.low).toBe(false);
  });
});
