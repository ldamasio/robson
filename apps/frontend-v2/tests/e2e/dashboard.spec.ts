import { test, expect } from '@playwright/test';

test.describe('Dashboard', () => {
  test('placeholder renders', async ({ page }) => {
    // EP-004 will replace with real e2e dashboard checks
    await page.goto('/');
    await expect(page).toHaveTitle(/RBX Robson/);
  });
});
