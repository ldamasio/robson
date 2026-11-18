import { test, expect } from '@playwright/test';
import { installMockGoogleIdentity } from './helpers';

// ADR-0054: mock the GIS script locally so the shell test also verifies the
// real component's asynchronous script.onload/initialize/renderButton lifecycle.
// Authenticated route suites exercise its credential callback via authAndGoto.
test.describe('Auth', () => {
  test.beforeEach(async ({ page }) => {
    await installMockGoogleIdentity(page);
  });

  test('login page renders the Google sign-in container', async ({ page }) => {
    await page.goto('/login');
    await expect(page.locator('h1')).toHaveText('Robson');
    await expect(page.locator('.google-button')).toBeAttached();
    await expect(page.getByRole('button', { name: 'Sign in with Google (test)' }))
      .toBeVisible();
    // The old paste-a-token form must be gone.
    await expect(page.locator('input[type="password"]')).toHaveCount(0);
  });

  test('protected route redirects to login without a token', async ({ page }) => {
    await page.goto('/dashboard');
    await expect(page).toHaveURL(/\/login/);
  });
});
