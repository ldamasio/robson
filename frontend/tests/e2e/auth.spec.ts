import { test, expect } from '@playwright/test';

test.describe('Auth', () => {
  test('login page renders with token input', async ({ page }) => {
    await page.goto('/login');
    await expect(page.locator('input[type="password"]')).toBeVisible();
    await expect(page.locator('button[type="submit"]')).toBeVisible();
    await expect(page.locator('h1')).toHaveText('Robson');
  });

  test('protected route redirects to login without token', async ({ page }) => {
    await page.goto('/dashboard');
    await expect(page).toHaveURL(/\/login/);
  });

  test('login page shows error on invalid token', async ({ page }) => {
    await page.goto('/login');
    await page.fill('input[type="password"]', 'invalid-token');
    await page.click('button[type="submit"]');
    await expect(page.locator('.error')).toBeVisible({ timeout: 5000 });
  });
});
