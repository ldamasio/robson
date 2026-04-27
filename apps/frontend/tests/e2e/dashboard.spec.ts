import { test, expect } from '@playwright/test';
import {
  installMockEventSource,
  authAndGoto,
  MOCK_POSITIONS,
  MOCK_HALT_ACTIVE
} from './helpers';

const STATUS_OK = {
  active_positions: 2,
  positions: MOCK_POSITIONS,
  pending_approvals: []
};

test.describe('Dashboard', () => {
  test('data state: 4 slots, 2 occupied, correct status strip', async ({ page }) => {
    await installMockEventSource(page);
    await page.route('**/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(STATUS_OK)
      })
    );
    await page.route('**/monthly-halt', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(MOCK_HALT_ACTIVE)
      })
    );

    await authAndGoto(page, '/dashboard');

    await expect(page.locator('.dashboard')).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('.slot')).toHaveCount(4);
    await expect(page.locator('.slot.occupied')).toHaveCount(2);
    await expect(page.locator('.status-strip')).toContainText('SLOT 2/4');
    await expect(page.locator('.op-card-link')).toHaveCount(2);
    await expect(page.locator('.eyebrow', { hasText: "TODAY'S EVENTS" })).toBeVisible();
    await expect(page.locator('.tick-ruler')).toBeVisible();
  });

  test('502 error state: error card and retry button visible', async ({ page }) => {
    await installMockEventSource(page);
    await page.route('**/status', (route) =>
      route.fulfill({ status: 502, body: 'Bad Gateway' })
    );
    await page.route('**/monthly-halt', (route) =>
      route.fulfill({ status: 502, body: 'Bad Gateway' })
    );

    await authAndGoto(page, '/dashboard');

    await expect(page.locator('.dashboard')).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('.err-text')).toBeVisible({ timeout: 5_000 });
    await expect(page.locator('.btn-retry')).toBeVisible();
    await expect(page.locator('.eyebrow', { hasText: 'CONNECTION ERROR' })).toBeVisible();
  });

  test('redirects to login without token', async ({ page }) => {
    await page.goto('/dashboard');
    await expect(page).toHaveURL(/\/login/, { timeout: 5_000 });
  });

  test('occupied slot links to operation detail', async ({ page }) => {
    await installMockEventSource(page);
    await page.route('**/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(STATUS_OK)
      })
    );
    await page.route('**/monthly-halt', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(MOCK_HALT_ACTIVE)
      })
    );

    await authAndGoto(page, '/dashboard');

    await expect(page.locator('.slot.occupied').first()).toBeVisible({ timeout: 10_000 });
    const href = await page.locator('.slot.occupied').first().getAttribute('href');
    expect(href).toMatch(/\/operation\/pos-1/);
  });
});
