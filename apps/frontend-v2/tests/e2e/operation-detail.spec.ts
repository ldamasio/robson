import { test, expect } from '@playwright/test';
import {
  installMockEventSource,
  authAndGoto,
  pushSseEvent,
  TEST_POS_ID,
  MOCK_POSITION,
  EVENT_MATCHING_1,
  EVENT_MATCHING_2,
  EVENT_NON_MATCHING
} from './helpers';

async function setupPositionRoute(page: Parameters<typeof authAndGoto>[0]) {
  await page.route(`**/positions/${TEST_POS_ID}`, (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(MOCK_POSITION)
    })
  );
}

test.describe('Operation Detail', () => {
  test('redirects to login without token', async ({ page }) => {
    await page.goto(`/operation/${TEST_POS_ID}`);
    await expect(page).toHaveURL(/\/login/, { timeout: 5_000 });
  });

  test('position loads: header, summary card, limitation banner', async ({ page }) => {
    await installMockEventSource(page);
    await setupPositionRoute(page);
    await authAndGoto(page, `/operation/${TEST_POS_ID}`);

    await expect(page.locator('.op-page')).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('.header')).toBeVisible({ timeout: 5_000 });
    await expect(page.locator('.eyebrow', { hasText: `OPERATION ${TEST_POS_ID.slice(0, 8)}` })).toBeVisible();
    await expect(page.locator('.event-stream-section')).toBeVisible();
    await expect(page.locator('.limitation')).toContainText('Events from this session only');
    await expect(page.locator('.limitation')).toContainText('FE-P2');
  });

  test('page title contains operation id', async ({ page }) => {
    await installMockEventSource(page);
    await setupPositionRoute(page);
    await authAndGoto(page, `/operation/${TEST_POS_ID}`);

    await expect(page.locator('.header')).toBeVisible({ timeout: 5_000 });
    const title = await page.title();
    expect(title).toContain('BTCUSDT');
  });

  test('matching SSE events render; non-matching events do not', async ({ page }) => {
    await installMockEventSource(page);
    await setupPositionRoute(page);
    await authAndGoto(page, `/operation/${TEST_POS_ID}`);

    await expect(page.locator('.header')).toBeVisible({ timeout: 5_000 });

    // Push one matching, one non-matching, one matching
    await pushSseEvent(page, EVENT_MATCHING_1);
    await pushSseEvent(page, EVENT_NON_MATCHING);
    await pushSseEvent(page, EVENT_MATCHING_2);

    // Exactly 2 event rows rendered (both matching events)
    await expect(page.locator('.event')).toHaveCount(2, { timeout: 3_000 });

    // Non-matching position_id payload does not produce a row
    const rows = page.locator('.event');
    const texts = await rows.allTextContents();
    for (const text of texts) {
      expect(text).not.toContain('other-pos');
    }
  });

  test('event anchors use session sequence (#event-{seq}), not backend event_id', async ({ page }) => {
    await installMockEventSource(page);
    await setupPositionRoute(page);
    await authAndGoto(page, `/operation/${TEST_POS_ID}`);

    await expect(page.locator('.header')).toBeVisible({ timeout: 5_000 });

    await pushSseEvent(page, EVENT_MATCHING_1);
    await pushSseEvent(page, EVENT_MATCHING_2);

    // Session seq anchors present
    await expect(page.locator('#event-1')).toBeVisible({ timeout: 3_000 });
    await expect(page.locator('#event-2')).toBeVisible({ timeout: 3_000 });

    // Backend event_id values must NOT be used as anchors
    await expect(page.locator('#evt-m1')).toHaveCount(0);
    await expect(page.locator('#evt-m2')).toHaveCount(0);
  });

  test('/operation/{id}#event-2 deep link reaches second session event', async ({ page }) => {
    await installMockEventSource(page);
    await setupPositionRoute(page);
    await authAndGoto(page, `/operation/${TEST_POS_ID}`);

    await expect(page.locator('.header')).toBeVisible({ timeout: 5_000 });

    await pushSseEvent(page, EVENT_MATCHING_1);
    await expect(page.locator('#event-1')).toBeVisible({ timeout: 3_000 });

    await pushSseEvent(page, EVENT_MATCHING_2);
    await expect(page.locator('#event-2')).toBeVisible({ timeout: 3_000 });

    // Simulate deep-link hash navigation on same page
    await page.evaluate(() => {
      location.hash = 'event-2';
    });

    // Hash set correctly
    const hash = await page.evaluate(() => location.hash);
    expect(hash).toBe('#event-2');

    // Target element exists and is visible
    await expect(page.locator('#event-2')).toBeVisible();

    // event-1 still exists (not displaced by navigation)
    await expect(page.locator('#event-1')).toBeVisible();
  });
});
