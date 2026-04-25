import { test, expect } from '@playwright/test';
import { authAndGoto, MOCK_HALT_ACTIVE, MOCK_POSITIONS } from './helpers';

const MOCK_STATUS = {
  active_positions: 2,
  positions: MOCK_POSITIONS,
  pending_approvals: []
};

const MOCK_HALT_HALTED = {
  state: 'monthly_halt',
  description: 'Monthly halt active',
  reason: 'Operator intervention — risk limit exceeded',
  triggered_at: '2026-04-23T16:30:00.000Z',
  blocks_new_entries: true,
  blocks_signals: true
};

async function setupActiveRoutes(page: Parameters<typeof authAndGoto>[0]) {
  await page.route('**/monthly-halt', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(MOCK_HALT_ACTIVE)
    })
  );
  await page.route('**/status', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(MOCK_STATUS)
    })
  );
}

async function setupHaltedRoutes(page: Parameters<typeof authAndGoto>[0]) {
  await page.route('**/monthly-halt', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(MOCK_HALT_HALTED)
    })
  );
  await page.route('**/status', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(MOCK_STATUS)
    })
  );
}

test.describe('Kill Switch', () => {
  test('redirects to login without token', async ({ page }) => {
    await page.goto('/kill-switch');
    await expect(page).toHaveURL(/\/login/, { timeout: 5_000 });
  });

  test('active state renders with trigger form and affected positions', async ({
    page
  }) => {
    await setupActiveRoutes(page);
    await authAndGoto(page, '/kill-switch');

    await expect(page.locator('.ks-page')).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('h1')).toContainText('Robson');
    await expect(page.locator('.dot.live')).toBeVisible();
    await expect(page.locator('#reason-input')).toBeVisible();
    await expect(page.locator('#confirm-input')).toBeVisible();
    await expect(page.locator('.btn-confirm')).toBeVisible();
    await expect(page.locator('.btn-confirm')).toBeDisabled();

    // Affected positions preview
    await expect(page.locator('.positions-preview')).toBeVisible();
    await expect(page.locator('.pos-row')).toHaveCount(2);
    await expect(page.locator('.pos-row').first()).toContainText('BTCUSDT');
    await expect(page.locator('.pos-row').first()).toContainText('Armed');
    await expect(page.locator('.pos-row').last()).toContainText('ETHUSDT');
  });

  test('confirm button disabled until keyword match and non-empty reason', async ({
    page
  }) => {
    await setupActiveRoutes(page);
    await authAndGoto(page, '/kill-switch');

    await expect(page.locator('.btn-confirm')).toBeVisible({ timeout: 10_000 });

    // Empty reason + empty keyword → disabled
    await expect(page.locator('.btn-confirm')).toBeDisabled();

    // Keyword only, no reason → disabled (use DISABLE for default en locale)
    await page.fill('#confirm-input', 'DISABLE');
    await expect(page.locator('.btn-confirm')).toBeDisabled();

    // Reason only, wrong keyword → disabled
    await page.fill('#confirm-input', 'wrong');
    await page.fill('#reason-input', 'some reason');
    await expect(page.locator('.btn-confirm')).toBeDisabled();

    // Both correct → enabled
    await page.fill('#confirm-input', 'DISABLE');
    await expect(page.locator('.btn-confirm')).toBeEnabled();
  });

  test('successful POST transitions page to halted/latched state', async ({
    page
  }) => {
    let postCalled = false;

    await page.route('**/monthly-halt', async (route) => {
      if (route.request().method() === 'POST') {
        postCalled = true;
        const body = route.request().postDataJSON();
        expect(body?.reason).toBeTruthy();
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify(MOCK_HALT_HALTED)
        });
      } else {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify(MOCK_HALT_ACTIVE)
        });
      }
    });
    await page.route('**/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(MOCK_STATUS)
      })
    );

    await authAndGoto(page, '/kill-switch');

    await expect(page.locator('#confirm-input')).toBeVisible({ timeout: 10_000 });

    await page.fill('#reason-input', 'Test halt reason');
    await page.fill('#confirm-input', 'DISABLE');

    await expect(page.locator('.btn-confirm')).toBeEnabled();
    await page.click('.btn-confirm');

    // Transitions to halted state
    await expect(page.locator('.dot.halted')).toBeVisible({ timeout: 5_000 });
    await expect(page.locator('.halted-detail')).toBeVisible();

    expect(postCalled).toBe(true);
  });

  test('halted state shows triggered_at, reason, and no enable/countdown', async ({
    page
  }) => {
    await setupHaltedRoutes(page);
    await authAndGoto(page, '/kill-switch');

    await expect(page.locator('.ks-page')).toBeVisible({ timeout: 10_000 });

    // Title shows halted state (locale-agnostic: both have "Disabled")
    await expect(page.locator('h1')).toContainText('Disabled');

    // Shows triggered_at timestamp
    await expect(page.locator('.ts')).toContainText('2026-04-23');

    // Shows reason (first .reason-text is the REASON field, second is description)
    await expect(page.locator('.reason-text').first()).toContainText('Operator intervention');

    // Shows blocks metadata
    await expect(page.locator('.meta')).toContainText('New entries: YES');

    // No enable/re-enable button
    await expect(page.locator('.btn-confirm')).toHaveCount(0);

    // No countdown element
    await expect(page.locator('.countdown')).toHaveCount(0);
    await expect(page.locator('.cooldown')).toHaveCount(0);

    // No enable keyword input
    await expect(page.locator('#confirm-input')).toHaveCount(0);
  });

  test('error state shows retry button', async ({ page }) => {
    await page.route('**/monthly-halt', (route) =>
      route.fulfill({ status: 502, body: 'Bad Gateway' })
    );
    await page.route('**/status', (route) =>
      route.fulfill({ status: 502, body: 'Bad Gateway' })
    );

    await authAndGoto(page, '/kill-switch');

    await expect(page.locator('.ks-page')).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('.err-text')).toBeVisible({ timeout: 5_000 });
    await expect(page.locator('.btn-retry')).toBeVisible();
  });

  test('active state shows position count from status', async ({ page }) => {
    await setupActiveRoutes(page);
    await authAndGoto(page, '/kill-switch');

    await expect(page.locator('.state-line')).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('.state-line')).toContainText('SLOT 2/6');
  });

  test('pt-BR locale uses DESLIGAR keyword', async ({ page }) => {
    await page.route('**/monthly-halt', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(MOCK_HALT_ACTIVE)
      })
    );
    await page.route('**/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(MOCK_STATUS)
      })
    );

    // Set pt-BR locale cookie before auth
    await page.context().addCookies([
      { name: 'locale', value: 'pt-BR', domain: 'localhost', path: '/' }
    ]);

    await authAndGoto(page, '/kill-switch');
    await expect(page.locator('.ks-page')).toBeVisible({ timeout: 10_000 });

    // pt-BR title and keyword
    await expect(page.locator('h1')).toContainText('Desligar Robson');
    await page.fill('#reason-input', 'motivo teste');
    await page.fill('#confirm-input', 'DESLIGAR');
    await expect(page.locator('.btn-confirm')).toBeEnabled();

    // DISABLE does not work in pt-BR
    await page.fill('#confirm-input', 'DISABLE');
    await expect(page.locator('.btn-confirm')).toBeDisabled();
  });

  test('en locale uses DISABLE keyword', async ({ page }) => {
    await page.route('**/monthly-halt', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(MOCK_HALT_ACTIVE)
      })
    );
    await page.route('**/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(MOCK_STATUS)
      })
    );

    // Set en locale cookie before auth
    await page.context().addCookies([
      { name: 'locale', value: 'en', domain: 'localhost', path: '/' }
    ]);

    await authAndGoto(page, '/kill-switch');
    await expect(page.locator('.ks-page')).toBeVisible({ timeout: 10_000 });

    // en title and keyword
    await expect(page.locator('h1')).toContainText('Disable Robson');
    await page.fill('#reason-input', 'test reason');
    await page.fill('#confirm-input', 'DISABLE');
    await expect(page.locator('.btn-confirm')).toBeEnabled();

    // DESLIGAR does not work in en
    await page.fill('#confirm-input', 'DESLIGAR');
    await expect(page.locator('.btn-confirm')).toBeDisabled();
  });
});
