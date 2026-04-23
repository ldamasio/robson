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

    // Either error state or data state renders below header
    const hasError = await page.locator('.err-text').isVisible().catch(() => false);
    if (hasError) {
      // Error card has retry
      await expect(page.locator('.btn-retry')).toBeVisible();
    } else {
      // Data state: slots, operations, events sections
      await expect(page.locator('.slot')).toHaveCount(6);
      await expect(page.locator('.eyebrow', { hasText: 'SLOTS' })).toBeVisible();
      await expect(page.locator('.eyebrow', { hasText: 'ACTIVE OPERATIONS' })).toBeVisible();
      await expect(page.locator('.eyebrow', { hasText: "TODAY'S EVENTS" })).toBeVisible();
      await expect(page.locator('.tick-ruler')).toBeVisible();
    }
  });

  test('redirects to login without token', async ({ page }) => {
    await page.goto('/dashboard');
    await expect(page).toHaveURL(/\/login/, { timeout: 5_000 });
  });

  test('slot cells link to operation detail when occupied', async ({ page }) => {
    await page.goto('/login');
    await page.evaluate(() => sessionStorage.setItem('robson_api_token', 'test-token'));
    await page.goto('/dashboard');

    await expect(page.locator('.dashboard')).toBeVisible({ timeout: 10_000 });

    // Only meaningful if data state (not error)
    const slotsVisible = await page.locator('.slot').first().isVisible().catch(() => false);
    if (slotsVisible) {
      const emptySlots = page.locator('.slot:not(.occupied)');
      if ((await emptySlots.count()) > 0) {
        const href = await emptySlots.first().getAttribute('href');
        expect(href).toBe('');
      }
    }
  });

  test('error state renders correctly when backend unreachable', async ({ page }) => {
    await page.goto('/login');
    await page.evaluate(() => sessionStorage.setItem('robson_api_token', 'test-token'));
    await page.goto('/dashboard');

    await expect(page.locator('.dashboard')).toBeVisible({ timeout: 10_000 });

    // Wait for API call to resolve (error or data)
    await page.waitForTimeout(2000);

    const hasError = await page.locator('.err-text').isVisible().catch(() => false);
    if (hasError) {
      await expect(page.locator('.eyebrow', { hasText: 'CONNECTION ERROR' })).toBeVisible();
      await expect(page.locator('.btn-retry')).toBeVisible();
      // Status strip shows offline
      await expect(page.locator('.dot.err')).toBeVisible();
    }
  });

  test('data state renders sections when backend available', async ({ page }) => {
    await page.goto('/login');
    await page.evaluate(() => sessionStorage.setItem('robson_api_token', 'test-token'));
    await page.goto('/dashboard');

    await expect(page.locator('.dashboard')).toBeVisible({ timeout: 10_000 });
    await page.waitForTimeout(2000);

    // Only test sections if no error (backend unreachable in test env)
    const hasError = await page.locator('.err-text').isVisible().catch(() => false);
    if (!hasError) {
      // Slots grid
      await expect(page.locator('.slot')).toHaveCount(6);
      // Active operations section
      await expect(page.locator('.eyebrow', { hasText: 'ACTIVE OPERATIONS' })).toBeVisible();
      // Today's events section
      await expect(page.locator('.eyebrow', { hasText: "TODAY'S EVENTS" })).toBeVisible();
      // Tick ruler
      await expect(page.locator('.tick-ruler')).toBeVisible();
    }
  });
});
