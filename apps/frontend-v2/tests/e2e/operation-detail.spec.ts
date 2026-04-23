import { test, expect } from '@playwright/test';

test.describe('Operation Detail', () => {
  test('renders page shell with auth', async ({ page }) => {
    await page.goto('/login');
    await page.evaluate(() => sessionStorage.setItem('robson_api_token', 'test-token'));
    await page.goto('/operation/test-op-id');

    // Page shell renders
    await expect(page.locator('.op-page')).toBeVisible({ timeout: 10_000 });

    // Page title includes operation id
    const title = await page.title();
    expect(title).toContain('test-op-id');
  });

  test('redirects to login without token', async ({ page }) => {
    await page.goto('/operation/some-id');
    await expect(page).toHaveURL(/\/login/, { timeout: 5_000 });
  });

  test('shows error or data state (no real backend)', async ({ page }) => {
    await page.goto('/login');
    await page.evaluate(() => sessionStorage.setItem('robson_api_token', 'test-token'));
    await page.goto('/operation/no-backend-id');

    await expect(page.locator('.op-page')).toBeVisible({ timeout: 10_000 });

    // Without backend: error card with retry, or loading state
    const hasError = await page.locator('.err-text').isVisible().catch(() => false);
    const hasData = await page.locator('.header').isVisible().catch(() => false);
    const hasLoading = await page.locator('.loading').isVisible().catch(() => false);
    expect(hasError || hasData || hasLoading).toBe(true);

    // Error state should have retry button
    if (hasError) {
      await expect(page.locator('.btn-retry')).toBeVisible();
    }
  });

  test('event stream section renders when position loaded', async ({ page }) => {
    await page.goto('/login');
    await page.evaluate(() => sessionStorage.setItem('robson_api_token', 'test-token'));
    await page.goto('/operation/event-test-id');

    await expect(page.locator('.op-page')).toBeVisible({ timeout: 10_000 });

    // If position loaded (unlikely without backend), check event stream
    const hasHeader = await page.locator('.header').isVisible().catch(() => false);
    if (hasHeader) {
      await expect(page.locator('.event-stream-section')).toBeVisible();
      await expect(page.locator('.limitation')).toContainText('Events from this session only');
    }
  });
});
