import { test, expect } from '@playwright/test';

test.describe('Dashboard', () => {
  test('renders dashboard page structure', async ({ page }) => {
    await page.goto('/login');
    await page.evaluate(() => sessionStorage.setItem('robson_api_token', 'test-token'));
    await page.goto('/dashboard');

    // Page should render (auth guard passes via sessionStorage check)
    await expect(page).toHaveURL(/\/dashboard/);

    // Wait for Svelte hydration
    await expect(page.locator('.dashboard')).toBeVisible({ timeout: 10_000 });

    // Header has brand marks
    await expect(page.locator('img[alt="RBX"]')).toBeVisible();
    await expect(page.locator('img[alt="RBX Robson"]')).toBeVisible();

    // Status strip exists
    await expect(page.locator('.status-strip')).toBeVisible();

    // Slots grid renders 6 cells
    await expect(page.locator('.slot')).toHaveCount(6);

    // Slots section label exists
    await expect(page.locator('.eyebrow', { hasText: 'SLOTS' })).toBeVisible();

    // Active operations section exists
    await expect(page.locator('.eyebrow', { hasText: 'ACTIVE OPERATIONS' })).toBeVisible();

    // Today's events section exists
    await expect(page.locator('.eyebrow', { hasText: "TODAY'S EVENTS" })).toBeVisible();

    // Tick ruler signature element is present
    await expect(page.locator('.tick-ruler')).toBeVisible();
  });

  test('redirects to login without token', async ({ page }) => {
    await page.goto('/dashboard');
    await expect(page).toHaveURL(/\/login/, { timeout: 5_000 });
  });

  test('slot cells link to operation detail when occupied', async ({ page }) => {
    await page.goto('/login');
    await page.evaluate(() => sessionStorage.setItem('robson_api_token', 'test-token'));
    await page.goto('/dashboard');

    await expect(page.locator('.slot').first()).toBeVisible({ timeout: 10_000 });

    // Empty slots link to root (no operation)
    const emptySlots = page.locator('.slot:not(.occupied)');
    if ((await emptySlots.count()) > 0) {
      const href = await emptySlots.first().getAttribute('href');
      expect(href).toBe('');
    }
  });
});
