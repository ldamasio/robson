import { test, expect } from '@playwright/test';

test.describe('Dashboard', () => {
  test('renders slot cells from API data', async ({ page }) => {
    await page.route('**/health', (route) =>
      route.fulfill({ status: 200, body: JSON.stringify({ status: 'ok' }) })
    );
    await page.route('**/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          active_positions: 2,
          positions: [
            {
              id: 'pos-1',
              account_id: 'acc-1',
              symbol: 'BTCUSDT',
              side: 'Long',
              state: 'Armed',
              entry_price: null,
              entry_filled_at: null,
              tech_stop_distance: null,
              quantity: 0.001,
              realized_pnl: 0,
              fees_paid: 0,
              entry_order_id: null,
              exit_order_id: null,
              insurance_stop_id: null,
              binance_position_id: null,
              created_at: '2026-04-23T14:00:00Z',
              updated_at: '2026-04-23T14:00:00Z',
              closed_at: null
            },
            {
              id: 'pos-2',
              account_id: 'acc-1',
              symbol: 'ETHUSDT',
              side: 'Short',
              state: {
                Active: {
                  current_price: 3200,
                  trailing_stop: 3350,
                  favorable_extreme: 3100,
                  extreme_at: '2026-04-23T15:00:00Z',
                  insurance_stop_id: null,
                  last_emitted_stop: null
                }
              },
              entry_price: 3250,
              entry_filled_at: '2026-04-23T14:30:00Z',
              tech_stop_distance: 3.08,
              quantity: 0.5,
              realized_pnl: 1.5,
              fees_paid: 0.1,
              entry_order_id: 'ord-1',
              exit_order_id: null,
              insurance_stop_id: null,
              binance_position_id: 'bin-1',
              created_at: '2026-04-23T14:30:00Z',
              updated_at: '2026-04-23T15:00:00Z',
              closed_at: null
            }
          ],
          pending_approvals: []
        })
      })
    );
    await page.route('**/monthly-halt', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          state: 'active',
          description: 'Normal operation',
          reason: null,
          triggered_at: null,
          blocks_new_entries: false,
          blocks_signals: false
        })
      })
    );

    // Set a fake token in sessionStorage so auth guard passes
    await page.goto('/login');
    await page.evaluate(() => sessionStorage.setItem('robson_api_token', 'test-token'));
    await page.goto('/dashboard');

    // Slot cells: 2 occupied, 4 free
    const slots = page.locator('.slot');
    await expect(slots).toHaveCount(6);
    const occupied = page.locator('.slot.occupied');
    await expect(occupied).toHaveCount(2);

    // Status strip shows slot count
    await expect(page.locator('.status-strip')).toContainText('SLOT 2/6');

    // Active operations panel has cards
    await expect(page.locator('.op-card-link')).toHaveCount(2);

    // Today events section exists (empty is ok, no SSE in test)
    await expect(page.locator('.eyebrow', { hasText: "TODAY'S EVENTS" })).toBeVisible();
  });

  test('shows error when API unreachable', async ({ page }) => {
    await page.route('**/status', (route) =>
      route.fulfill({ status: 502, body: 'Bad Gateway' })
    );
    await page.route('**/monthly-halt', (route) =>
      route.fulfill({ status: 502, body: 'Bad Gateway' })
    );

    await page.goto('/login');
    await page.evaluate(() => sessionStorage.setItem('robson_api_token', 'test-token'));
    await page.goto('/dashboard');

    await expect(page.locator('.err-text')).toBeVisible({ timeout: 5000 });
    await expect(page.locator('.btn-retry')).toBeVisible();
  });
});
