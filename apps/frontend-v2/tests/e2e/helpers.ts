import type { Page } from '@playwright/test';

export const TEST_POS_ID = 'test-pos-1';

export const MOCK_POSITION = {
  id: TEST_POS_ID,
  account_id: 'acc-1',
  symbol: 'BTCUSDT',
  side: 'Long',
  state: 'Armed',
  entry_price: null,
  entry_filled_at: null,
  tech_stop_distance: null,
  quantity: 0.001,
  realized_pnl: 0,
  fees_paid: 0,
  entry_order_id: null,
  exit_order_id: null,
  insurance_stop_id: null,
  binance_position_id: null,
  created_at: '2026-04-23T14:00:00.000Z',
  updated_at: '2026-04-23T14:00:00.000Z',
  closed_at: null
};

export const MOCK_POSITIONS = [
  {
    id: 'pos-1',
    account_id: 'acc-1',
    symbol: 'BTCUSDT',
    side: 'Long',
    state: 'Armed',
    entry_price: null,
    entry_filled_at: null,
    tech_stop_distance: null,
    quantity: 0.001,
    realized_pnl: 0,
    fees_paid: 0,
    entry_order_id: null,
    exit_order_id: null,
    insurance_stop_id: null,
    binance_position_id: null,
    created_at: '2026-04-23T14:00:00.000Z',
    updated_at: '2026-04-23T14:00:00.000Z',
    closed_at: null
  },
  {
    id: 'pos-2',
    account_id: 'acc-1',
    symbol: 'ETHUSDT',
    side: 'Short',
    state: {
      Active: {
        current_price: 3200,
        trailing_stop: 3350,
        favorable_extreme: 3100,
        extreme_at: '2026-04-23T15:00:00.000Z',
        insurance_stop_id: null,
        last_emitted_stop: null
      }
    },
    entry_price: 3250,
    entry_filled_at: '2026-04-23T14:30:00.000Z',
    tech_stop_distance: 3.08,
    quantity: 0.5,
    realized_pnl: 1.5,
    fees_paid: 0.1,
    entry_order_id: 'ord-1',
    exit_order_id: null,
    insurance_stop_id: null,
    binance_position_id: 'bin-1',
    created_at: '2026-04-23T14:30:00.000Z',
    updated_at: '2026-04-23T15:00:00.000Z',
    closed_at: null
  }
];

export const MOCK_HALT_ACTIVE = {
  state: 'active',
  description: 'Normal operation',
  reason: null,
  triggered_at: null,
  blocks_new_entries: false,
  blocks_signals: false
};

export const EVENT_MATCHING_1 = {
  event_id: 'evt-m1',
  event_type: 'position.armed',
  occurred_at: '2026-04-23T14:00:00.100Z',
  payload: { position_id: TEST_POS_ID, symbol: 'BTCUSDT' }
};

export const EVENT_MATCHING_2 = {
  event_id: 'evt-m2',
  event_type: 'position.updated',
  occurred_at: '2026-04-23T14:00:02.200Z',
  payload: { position_id: TEST_POS_ID, new_state: 'Active' }
};

export const EVENT_NON_MATCHING = {
  event_id: 'evt-x1',
  event_type: 'position.armed',
  occurred_at: '2026-04-23T14:00:01.500Z',
  payload: { position_id: 'other-pos', symbol: 'ETHUSDT' }
};

/**
 * Install an app-specific EventSource factory before page scripts run.
 * Tests push events via window.__ssePush(sseEventObject).
 * Must be called before page.goto().
 */
export async function installMockEventSource(page: Page): Promise<void> {
  await page.addInitScript(() => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const instances: any[] = [];

    class MockEventSource {
      url: string;
      readyState = 1;
      onmessage: ((e: { data: string }) => void) | null = null;
      onerror: ((e: Event) => void) | null = null;

      constructor(url: string) {
        this.url = url;
        instances.push(this);
      }

      close() {
        this.readyState = 2;
        const i = instances.indexOf(this);
        if (i !== -1) instances.splice(i, 1);
      }

      addEventListener() {}
      removeEventListener() {}
      dispatchEvent() {
        return true;
      }
    }

    const w = window as unknown as {
      __RBX_EVENT_SOURCE_FACTORY__?: (url: string) => MockEventSource;
      __ssePush?: (data: unknown) => void;
    };

    w.__RBX_EVENT_SOURCE_FACTORY__ = (url: string) => new MockEventSource(url);
    w.__ssePush = (data: unknown) => {
      const str = JSON.stringify(data);
      for (const inst of instances) {
        if (inst.readyState === 1 && inst.onmessage) {
          inst.onmessage({ data: str });
        }
      }
    };
  });
}

export async function authAndGoto(page: Page, path: string): Promise<void> {
  await page.route('**/health', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ status: 'ok' })
    })
  );

  await page.goto(`/login?redirect=${encodeURIComponent(path)}`);
  await page.fill('input[type="password"]', 'test-token');
  await page.click('button[type="submit"]');
  await page.waitForURL((url) => url.pathname === path, { timeout: 10_000 });
}

export async function pushSseEvent(page: Page, event: unknown): Promise<void> {
  await page.evaluate(
    (d) => (window as unknown as { __ssePush: (x: unknown) => void }).__ssePush(d),
    event
  );
}
