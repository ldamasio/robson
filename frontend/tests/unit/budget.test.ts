import { describe, it, expect } from "vitest";
import {
  budgetModelKind,
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
    monthly_budget_model: null,
    monthly_budget_utilization_pct: null,
    month_governed_realized_net: null,
    monthly_net_loss_consumed: null,
    monthly_open_risk_reserved: null,
    monthly_budget_amount: null,
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

/**
 * Production `/status` on 2026-10-15 02:40 UTC, right after a +37.5 USDT
 * insurance-stop close left the book empty: the month was net +83.50 with
 * no open risk, yet the persisted `hwm_v1` model charged the 17.92 give-back
 * from the intra-trade equity peak and showed 2 slots. The same payload under
 * `net_from_start_non_expanding_v1` is what ADR-0051 activation produces.
 */
const PROD_2026_10_15_HWM = makeStatus({
  new_slots_available: 2,
  slot_cells_total: 2,
  monthly_realized_loss: 33.9722871,
  monthly_realized_loss_pct: 1.9857441479818966,
  capital_base: 1710.80887407,
  wallet_balance: 1796.92471682,
  month_equity_net: 83.49757967,
  month_peak_net: 101.41972607,
  monthly_giveback_pct: 26.18958007472127,
  monthly_budget_remaining: 50.5102085628,
  monthly_budget_model: "hwm_v1",
  monthly_budget_utilization_pct: 26.18958007472127,
  month_governed_realized_net: 83.49757967,
  monthly_net_loss_consumed: 0,
  monthly_open_risk_reserved: 0,
  monthly_budget_amount: 68.4323549628,
});

const PROD_2026_10_15_NFS = makeStatus({
  ...PROD_2026_10_15_HWM,
  new_slots_available: 4,
  slot_cells_total: 4,
  month_peak_net: null,
  monthly_giveback_pct: null,
  monthly_budget_remaining: 68.4323549628,
  monthly_budget_model: "net_from_start_non_expanding_v1",
  monthly_budget_utilization_pct: 0,
});

describe("budgetModelKind", () => {
  it("names the persisted model and falls back by field presence", () => {
    expect(budgetModelKind(PROD_2026_10_15_HWM)).toBe("hwm");
    expect(budgetModelKind(PROD_2026_10_15_NFS)).toBe("net_from_start");
    expect(budgetModelKind(makeStatus())).toBe("legacy");
    expect(budgetModelKind(null)).toBe("legacy");
    // A backend that reports the model but no utilization yet.
    expect(
      budgetModelKind(
        makeStatus({ monthly_budget_model: "hwm_v1", monthly_giveback_pct: 3 }),
      ),
    ).toBe("hwm");
  });
});

describe("deriveBudgetView", () => {
  it("reads the active model's utilization: HWM charges the give-back from peak", () => {
    const view = deriveBudgetView(PROD_2026_10_15_HWM, false);

    expect(view.model).toBe("hwm");
    expect(view.usedPct).toBeCloseTo(26.19, 2);
    // 1.05% of capital against the 4% limit, the "1.0% of a 4.0% limit" the
    // operator questioned on a net-positive month.
    expect(view.drawdownPct).toBeCloseTo(1.0476, 3);
  });

  it("reads the active model's utilization: net-from-start shows a clean gauge on a net-positive month", () => {
    const view = deriveBudgetView(PROD_2026_10_15_NFS, false);

    expect(view.model).toBe("net_from_start");
    expect(view.usedPct).toBe(0);
    expect(view.drawdownPct).toBe(0);
    expect(view.exhausted).toBe(false);
    expect(view.low).toBe(false);
  });

  it("never falls back to the gross-loss formula once utilization is reported", () => {
    // Without utilization the legacy fallback would read 33.97 of gross loss
    // as 49.6% of the budget on a month that is net +83.50.
    const legacyLooking = makeStatus({
      ...PROD_2026_10_15_NFS,
      monthly_budget_model: null,
      monthly_budget_utilization_pct: null,
    });
    expect(deriveBudgetView(legacyLooking, false).usedPct).toBeCloseTo(
      49.64,
      1,
    );
    expect(deriveBudgetView(PROD_2026_10_15_NFS, false).usedPct).toBe(0);
  });


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
