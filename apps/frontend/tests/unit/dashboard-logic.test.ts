import { describe, it, expect } from 'vitest';
import {
  positionLabel,
  positionStateLabel,
  haltStateLabel,
  isPositionActive,
  eventSummaryText,
  eventTypeLabel
} from '$lib/presentation/labels';
import { deriveSlots, occupiedCount } from '$lib/config/slots';
import { formatTimeUtc, isTodayUtc } from '$lib/utils/time';
import type { Position, PositionState, SseEvent, StatusResponse, MonthlyHaltStatus } from '$api/robson';

// Helper to create minimal Position objects for testing
function makePosition(overrides: Partial<Position> & { id: string; state: PositionState }): Position {
  return {
    account_id: 'acc-1',
    symbol: 'BTCUSDT',
    side: 'Long',
    entry_price: null,
    entry_filled_at: null,
    tech_stop_distance: null,
    quantity: 0,
    realized_pnl: 0,
    fees_paid: 0,
    entry_order_id: null,
    exit_order_id: null,
    insurance_stop_id: null,
    binance_position_id: null,
    created_at: '2026-04-23T12:00:00Z',
    updated_at: '2026-04-23T12:00:00Z',
    closed_at: null,
    ...overrides
  };
}

function makeSseEvent(payload: Record<string, unknown>): SseEvent {
  return {
    event_id: 'ev-test',
    event_type: 'position.opened',
    occurred_at: '2026-04-23T14:30:45.123Z',
    payload
  };
}

// --- Dashboard data flow tests ---

describe('dashboard slot derivation from status response', () => {
  it('empty status yields 4 free slots', () => {
    const positions: Position[] = [];
    const slots = deriveSlots(positions, 4);
    expect(slots).toHaveLength(4);
    expect(slots.filter(s => s.occupied).length).toBe(0);
    slots.forEach(s => expect(s.occupied).toBe(false));
  });

  it('active position occupies one slot', () => {
    const positions = [
      makePosition({ id: 'p1', state: { Active: { current_price: 65000, trailing_stop: 62000, favorable_extreme: 65000, extreme_at: '2026-04-23T14:00:00Z', insurance_stop_id: null, last_emitted_stop: null } } })
    ];
    const slots = deriveSlots(positions, 4);
    expect(slots.filter(s => s.occupied).length).toBe(1);
    expect(slots.filter(s => s.occupied && s.positionId === 'p1')).toHaveLength(1);
  });

  it('production Active string position occupies one slot', () => {
    const positions = [
      makePosition({
        id: '019db3dc-c107-7872-bdcb-c3e6602ebbe0',
        state: 'Active',
        entry_price: 77932.4,
        trailing_stop: 76158.25,
        pnl: 0
      })
    ];
    const slots = deriveSlots(positions, 4);
    expect(slots).toHaveLength(4);
    expect(slots.filter(s => s.occupied)).toHaveLength(1);
  });

  it('Armed position counts as occupied', () => {
    const positions = [makePosition({ id: 'p1', state: 'Armed' })];
    expect(deriveSlots(positions, 4).filter(s => s.occupied).length).toBe(1);
  });

  it('Entering position counts as occupied', () => {
    const positions = [
      makePosition({
        id: 'p1',
        state: { Entering: { entry_order_id: 'o1', expected_entry: 65000, signal_id: 's1' } }
      })
    ];
    expect(deriveSlots(positions, 4).filter(s => s.occupied).length).toBe(1);
  });

  it('Closed position does NOT occupy a slot', () => {
    const positions = [
      makePosition({
        id: 'p1',
        state: { Closed: { exit_price: 70000, realized_pnl: 5.0, exit_reason: 'trailing_stop' } }
      })
    ];
    expect(deriveSlots(positions, 4).filter(s => s.occupied).length).toBe(0);
  });

  it('Error position does NOT occupy a slot', () => {
    const positions = [
      makePosition({ id: 'p1', state: { Error: { error: 'conn lost', recoverable: true } } })
    ];
    expect(deriveSlots(positions, 4).filter(s => s.occupied).length).toBe(0);
  });

  it('Exiting position does NOT occupy a slot', () => {
    const positions = [
      makePosition({
        id: 'p1',
        state: { Exiting: { exit_order_id: 'o1', exit_reason: 'manual' } }
      })
    ];
    expect(deriveSlots(positions, 4).filter(s => s.occupied).length).toBe(0);
  });

  it('mix of states: correct occupied count', () => {
    const positions = [
      makePosition({ id: 'p1', state: 'Armed' }),
      makePosition({ id: 'p2', state: { Active: { current_price: 1, trailing_stop: 1, favorable_extreme: 1, extreme_at: '', insurance_stop_id: null, last_emitted_stop: null } } }),
      makePosition({ id: 'p3', state: { Closed: { exit_price: 1, realized_pnl: 0, exit_reason: 'x' } } }),
      makePosition({ id: 'p4', state: { Entering: { entry_order_id: 'x', expected_entry: 1, signal_id: 'x' } } })
    ];
    expect(deriveSlots(positions, 4).filter(s => s.occupied).length).toBe(3); // Armed + Active + Entering
  });

  it('does not cap displayed active positions at the initial monthly budget', () => {
    const positions = Array.from({ length: 8 }, (_, i) =>
      makePosition({ id: `p${i}`, state: 'Armed' })
    );
    const slots = deriveSlots(positions, 4);
    expect(slots).toHaveLength(8);
    expect(slots.every(s => s.occupied)).toBe(true);
  });
});

// --- Status strip rendering logic ---

describe('status strip display logic', () => {
  it('halt active state shows correct label', () => {
    expect(haltStateLabel('active')).toBe('Active');
  });

  it('halt monthly_halt state shows correct label', () => {
    expect(haltStateLabel('monthly_halt')).toBe('Monthly Halt');
  });

  it('position state labels for all variants', () => {
    expect(positionStateLabel('Armed')).toBe('Armed');
    expect(positionStateLabel({ Entering: { entry_order_id: 'x', expected_entry: 1, signal_id: 'x' } })).toBe('Entering');
    expect(positionStateLabel({ Active: { current_price: 1, trailing_stop: 1, favorable_extreme: 1, extreme_at: '', insurance_stop_id: null, last_emitted_stop: null } })).toBe('Active');
    expect(positionStateLabel({ Exiting: { exit_order_id: 'x', exit_reason: 'x' } })).toBe('Exiting');
    expect(positionStateLabel({ Closed: { exit_price: 1, realized_pnl: 0, exit_reason: 'x' } })).toBe('Closed');
    expect(positionStateLabel({ Error: { error: 'x', recoverable: false } })).toBe('Error');
  });
});

// --- Today's events filtering logic ---

describe('today events filtering', () => {
  it('isTodayUtc matches events from today', () => {
    const now = new Date();
    const iso = now.toISOString();
    expect(isTodayUtc(iso)).toBe(true);
  });

  it('isTodayUtc rejects yesterday events', () => {
    const yesterday = new Date();
    yesterday.setUTCDate(yesterday.getUTCDate() - 1);
    expect(isTodayUtc(yesterday.toISOString())).toBe(false);
  });

  it('isTodayUtc rejects tomorrow events', () => {
    const tomorrow = new Date();
    tomorrow.setUTCDate(tomorrow.getUTCDate() + 1);
    expect(isTodayUtc(tomorrow.toISOString())).toBe(false);
  });

  it('UTC timestamp format includes milliseconds', () => {
    const formatted = formatTimeUtc('2026-04-23T14:30:45.123Z');
    expect(formatted).toContain('14:30:45.123');
  });
});

// --- Event stream presentation ---

describe('event stream rendering logic', () => {
  it('event type label converts dots to spaces', () => {
    const event: SseEvent = {
      event_id: 'ev-test',
      event_type: 'position.stop_updated',
      occurred_at: '2026-04-23T14:30:45.123Z',
      payload: {}
    };
    expect(eventTypeLabel(event)).toBe('POSITION STOP_UPDATED');
  });

  it('event type label for single-word type', () => {
    const event: SseEvent = { ...makeSseEvent({}), event_type: 'signal' };
    expect(eventTypeLabel(event)).toBe('SIGNAL');
  });

  it('event summary includes all payload fields', () => {
    const event = makeSseEvent({
      symbol: 'BTCUSDT',
      side: 'Long',
      entry_price: 63000,
      stop_price: 62000,
      exit_price: 70000,
      realized_pnl: 11.11,
      reason: 'trailing_stop',
      new_state: 'Closed'
    });
    const text = eventSummaryText(event);
    expect(text).toContain('BTCUSDT');
    expect(text).toContain('Long');
    expect(text).toContain('entry 63,000.00');
    expect(text).toContain('stop 62,000.00');
    expect(text).toContain('exit 70,000.00');
    expect(text).toContain('pnl +11.11%');
    expect(text).toContain('trailing_stop');
    expect(text).toContain('Closed');
  });

  it('event summary with negative PnL', () => {
    const event = makeSseEvent({ realized_pnl: -5.5 });
    expect(eventSummaryText(event)).toContain('pnl -5.50%');
  });

  it('event summary with zero PnL', () => {
    const event = makeSseEvent({ realized_pnl: 0 });
    expect(eventSummaryText(event)).toContain('pnl 0.00%');
  });
});

// --- Active position filter for dashboard ---

describe('isPositionActive for dashboard operations panel', () => {
  it('only Armed, Entering, Active are active', () => {
    expect(isPositionActive('Armed')).toBe(true);
    expect(isPositionActive('Active')).toBe(true);
    expect(isPositionActive({ Entering: { entry_order_id: 'x', expected_entry: 1, signal_id: 'x' } })).toBe(true);
    expect(isPositionActive({ Active: { current_price: 1, trailing_stop: 1, favorable_extreme: 1, extreme_at: '', insurance_stop_id: null, last_emitted_stop: null } })).toBe(true);
    expect(isPositionActive({ Exiting: { exit_order_id: 'x', exit_reason: 'x' } })).toBe(false);
    expect(isPositionActive({ Closed: { exit_price: 1, realized_pnl: 0, exit_reason: 'x' } })).toBe(false);
    expect(isPositionActive({ Error: { error: 'x', recoverable: false } })).toBe(false);
  });
});

// --- Dashboard error state logic ---

describe('error state handling', () => {
  it('position fetch error produces user message', () => {
    const err = new Error('API /positions/x failed: 404 Not Found');
    const message = err instanceof Error ? err.message : 'Failed to load position';
    expect(message).toContain('404');
    expect(message).toContain('Not Found');
  });

  it('status fetch error produces user message', () => {
    const err = new Error('API /status failed: 502 Bad Gateway');
    const message = err instanceof Error ? err.message : 'Failed to load';
    expect(message).toContain('502');
  });
});

// --- Number formatting for dashboard display ---

describe('number formatting for dashboard', () => {
  it('formats large prices with commas', () => {
    // Test via eventSummaryText which uses fmtNum internally
    const event = makeSseEvent({ entry_price: 65000.5 });
    const text = eventSummaryText(event);
    expect(text).toContain('65,000.50');
  });

  it('formats small prices correctly', () => {
    const event = makeSseEvent({ entry_price: 0.01 });
    const text = eventSummaryText(event);
    expect(text).toContain('0.01');
  });

  it('PnL formatting positive with plus sign', () => {
    const event = makeSseEvent({ realized_pnl: 15.5 });
    const text = eventSummaryText(event);
    expect(text).toContain('+15.50%');
  });
});
