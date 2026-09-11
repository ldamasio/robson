import { test, expect } from '@playwright/test';

// ADR-0054: the pasted-token form was replaced by Google Identity Services
// (GIS). GIS renders its button asynchronously into `.google-button` after
// loading `https://accounts.google.com/gsi/client`, which these tests don't
// mock — so they assert the page shell and the redirect guard rather than a
// completed Google sign-in, which would need a mocked GIS response.
test.describe('Auth', () => {
  test('login page renders the Google sign-in container', async ({ page }) => {
    await page.goto('/login');
    await expect(page.locator('h1')).toHaveText('Robson');
    await expect(page.locator('.google-button')).toBeAttached();
    // The old paste-a-token form must be gone.
    await expect(page.locator('input[type="password"]')).toHaveCount(0);
  });

  test('protected route redirects to login without a token', async ({ page }) => {
    await page.goto('/dashboard');
    await expect(page).toHaveURL(/\/login/);
  });
});
