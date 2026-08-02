import { test, expect, type Page } from "@playwright/test";
import {
  installMockEventSource,
  authAndGoto,
  MOCK_POSITIONS,
  MOCK_HALT_ACTIVE,
} from "./helpers";

const STATUS_OK = {
  active_positions: 2,
  positions: MOCK_POSITIONS,
  pending_approvals: [],
  stale_active_count: 0,
  reconciliation_blockers: [],
  occupied_slots: 2,
  new_slots_available: 2,
  slot_cells_total: 4,
  monthly_realized_loss: 80,
  monthly_realized_loss_pct: 0.8,
  capital_base: 10000,
  wallet_balance: 10000,
};

async function routeHealthyDashboard(page: Page): Promise<void> {
  await page.route("**/status", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(STATUS_OK),
    }),
  );
  await page.route("**/monthly-halt", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(MOCK_HALT_ACTIVE),
    }),
  );
  await page.route("**/positions?month=*", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        month: new Date().toISOString().slice(0, 7),
        positions: MOCK_POSITIONS,
        occupied_slots: 2,
        slot_cells_total: 4,
      }),
    }),
  );
}

test.describe("Dashboard", () => {
  test("data state: 4 slots, 2 occupied, correct status strip", async ({
    page,
  }) => {
    await installMockEventSource(page);
    await routeHealthyDashboard(page);
    await page.route("**/events/history?date=*", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          date: new Date().toISOString().slice(0, 10),
          events: [],
        }),
      }),
    );

    await authAndGoto(page, "/dashboard");

    await expect(page.locator(".dashboard")).toBeVisible({ timeout: 10_000 });
    await expect(page.locator(".slot")).toHaveCount(4);
    await expect(page.locator(".slot.occupied")).toHaveCount(2);
    await expect(page.locator(".status-strip")).toContainText("SLOT 2/4");
    await expect(page.locator(".op-card-link")).toHaveCount(2);
    await expect(
      page.locator(".eyebrow", { hasText: "TODAY'S EVENTS" }),
    ).toBeVisible();
    await expect(page.locator(".tick-ruler")).toBeVisible();
  });

  test("502 error state: error card and retry button visible", async ({
    page,
  }) => {
    await installMockEventSource(page);
    await page.route("**/status", (route) =>
      route.fulfill({ status: 502, body: "Bad Gateway" }),
    );
    await page.route("**/monthly-halt", (route) =>
      route.fulfill({ status: 502, body: "Bad Gateway" }),
    );

    await authAndGoto(page, "/dashboard");

    await expect(page.locator(".dashboard")).toBeVisible({ timeout: 10_000 });
    await expect(page.locator(".err-text")).toBeVisible({ timeout: 5_000 });
    await expect(page.locator(".btn-retry")).toBeVisible();
    await expect(
      page.locator(".eyebrow", { hasText: "CONNECTION ERROR" }),
    ).toBeVisible();
  });

  test("today's persisted events render with an authenticated history request", async ({
    page,
  }) => {
    await installMockEventSource(page);
    await routeHealthyDashboard(page);
    const date = new Date().toISOString().slice(0, 10);
    const now = Date.now();
    await page.route("**/events/history?date=*", (route) => {
      expect(route.request().headers().authorization).toBe("Bearer test-token");
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          date,
          events: [
            {
              event_id: "event-entry-filled",
              event_type: "entry_filled",
              occurred_at: new Date(now - 3_000).toISOString(),
              payload: { position_id: "pos-1" },
            },
            {
              event_id: "event-stop-replaced",
              event_type: "insurance_stop_replaced",
              occurred_at: new Date(now - 2_000).toISOString(),
              payload: { position_id: "pos-1" },
            },
            {
              event_id: "event-trailing-updated",
              event_type: "trailing_stop_updated",
              occurred_at: new Date(now - 1_000).toISOString(),
              payload: { position_id: "pos-1" },
            },
          ],
        }),
      });
    });

    await authAndGoto(page, "/dashboard");

    await expect(page.locator(".event-line")).toHaveCount(3);
    await expect(page.locator(".event-stream")).toContainText("ENTRY_FILLED");
    await expect(page.locator(".event-stream")).toContainText(
      "INSURANCE_STOP_REPLACED",
    );
    await expect(page.locator(".event-stream")).toContainText(
      "TRAILING_STOP_UPDATED",
    );
    await expect(page.getByText("No events today.")).toHaveCount(0);
  });

  test("an empty successful history response shows no events today", async ({
    page,
  }) => {
    await installMockEventSource(page);
    await routeHealthyDashboard(page);
    await page.route("**/events/history?date=*", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          date: new Date().toISOString().slice(0, 10),
          events: [],
        }),
      }),
    );

    await authAndGoto(page, "/dashboard");

    await expect(page.getByText("No events today.")).toBeVisible();
    await expect(page.locator(".event-line")).toHaveCount(0);
  });

  test("an event history fetch error is not rendered as an empty day", async ({
    page,
  }) => {
    await installMockEventSource(page);
    await routeHealthyDashboard(page);
    await page.route("**/events/history?date=*", (route) =>
      route.fulfill({
        status: 401,
        contentType: "application/json",
        body: JSON.stringify({
          error: "Missing or invalid Authorization header",
        }),
      }),
    );

    await authAndGoto(page, "/dashboard");

    await expect(page.getByRole("alert")).toContainText(
      "Today's events could not be loaded.",
    );
    await expect(page.getByRole("alert")).toContainText("401 Unauthorized");
    await expect(page.getByText("No events today.")).toHaveCount(0);
  });

  test("redirects to login without token", async ({ page }) => {
    await page.goto("/dashboard");
    await expect(page).toHaveURL(/\/login/, { timeout: 5_000 });
  });

  test("occupied slot links to operation detail", async ({ page }) => {
    await installMockEventSource(page);
    await page.route("**/status", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(STATUS_OK),
      }),
    );
    await page.route("**/monthly-halt", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(MOCK_HALT_ACTIVE),
      }),
    );

    await authAndGoto(page, "/dashboard");

    await expect(page.locator(".slot.occupied").first()).toBeVisible({
      timeout: 10_000,
    });
    const href = await page
      .locator(".slot.occupied")
      .first()
      .getAttribute("href");
    expect(href).toMatch(/\/operation\/pos-1/);
  });

  test("month boundary preserves occupied slots and shows new monthly slots", async ({
    page,
  }) => {
    const carriedPositions = [
      ...MOCK_POSITIONS,
      {
        ...MOCK_POSITIONS[0],
        id: "pos-3",
        symbol: "ADAUSDT",
      },
    ];
    await installMockEventSource(page);
    await page.route("**/status", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          active_positions: 3,
          positions: carriedPositions,
          pending_approvals: [],
          stale_active_count: 0,
          reconciliation_blockers: [],
          occupied_slots: 3,
          new_slots_available: 4,
          slot_cells_total: 7,
          monthly_realized_loss: 0,
          monthly_realized_loss_pct: 0,
          capital_base: 10000,
          wallet_balance: 10000,
        }),
      }),
    );
    await page.route("**/monthly-halt", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(MOCK_HALT_ACTIVE),
      }),
    );

    await authAndGoto(page, "/dashboard");

    await expect(page.locator(".dashboard")).toBeVisible({ timeout: 10_000 });
    await expect(page.locator(".slot")).toHaveCount(7);
    await expect(page.locator(".slot.occupied")).toHaveCount(3);
    await expect(page.locator(".status-strip")).toContainText("SLOT 3/7");
    await expect(page.locator(".eyebrow", { hasText: "4 FREE" })).toBeVisible();
  });
});
