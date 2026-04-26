import { describe, it, expect } from 'vitest';
import {
  positionSummaryLines,
  positionMetaLine,
  eventSummaryText,
  eventTypeLabel,
  isPositionActive,
  positionLabel,
  positionStateLabel
} from '$lib/presentation/labels';
import type { Position, PositionState, SseEvent } from '$api/robson';

function basePosition(overrides: Partial<Position> = {}): Position {
  return {
    id: 'aaaa-bbbb',
    account_id: 'acc-1',
    symbol: 'BTCUSDT',
    side: 'Long',
    state: 'Armed',
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

function makeEvent(payload: Record<string, unknown>): SseEvent {
  return {
    event_id: 'ev-1',
    event_type: 'position.entered',
    occurred_at: '2026-04-23T12:00:00Z',
    payload
  };
}

// --- positionSummaryLines ---

describe('positionSummaryLines', () => {
  it('Armed state', () => {
    const lines = positionSummaryLines(basePosition({ state: 'Armed' }));
    expect(lines).toHaveLength(1);
    expect(lines[0]).toContain('ARMED');
    expect(lines[0]).toContain('awaiting entry signal');
  });

  it('Active state with trailing stop', () => {
    const state: PositionState = {
      Active: {
        current_price: 65000,
        trailing_stop: 62000,
        favorable_extreme: 66000,
        extreme_at: '2026-04-23T13:00:00Z',
        insurance_stop_id: null,
        last_emitted_stop: null
      }
    };
    const lines = positionSummaryLines(
      basePosition({ state, entry_price: 63000, quantity: 0.5 })
    );
    expect(lines[0]).toContain('ACTIVE');
    expect(lines[0]).toContain('65,000.00');
    expect(lines[0]).toContain('62,000.00');
    expect(lines[1]).toContain('EXTREME');
    expect(lines[2]).toContain('ENTRY');
    expect(lines[3]).toContain('SIZE');
  });

  it('Closed state with PnL', () => {
    const state: PositionState = {
      Closed: { exit_price: 70000, realized_pnl: 11.11, exit_reason: 'trailing_stop' }
    };
    const lines = positionSummaryLines(
      basePosition({ state, entry_price: 63000, quantity: 0.5 })
    );
    expect(lines[0]).toContain('CLOSED');
    expect(lines[0]).toContain('70,000.00');
    expect(lines[1]).toContain('PnL');
    expect(lines[1]).toContain('+11.11%');
  });

  it('Error state', () => {
    const state: PositionState = { Error: { error: 'connection lost', recoverable: true } };
    const lines = positionSummaryLines(basePosition({ state }));
    expect(lines).toHaveLength(1);
    expect(lines[0]).toContain('ERROR');
    expect(lines[0]).toContain('connection lost');
  });

  it('Entering state', () => {
    const state: PositionState = {
      Entering: { entry_order_id: 'ord-1', expected_entry: 63000.5, signal_id: 'sig-1' }
    };
    const lines = positionSummaryLines(basePosition({ state }));
    expect(lines[0]).toContain('ENTERING');
    expect(lines[0]).toContain('63,000.50');
  });

  it('Exiting state', () => {
    const state: PositionState = { Exiting: { exit_order_id: 'ord-2', exit_reason: 'manual' } };
    const lines = positionSummaryLines(basePosition({ state }));
    expect(lines[0]).toContain('EXITING');
    expect(lines[0]).toContain('manual');
  });
});

// --- positionMetaLine ---

describe('positionMetaLine', () => {
  it('includes state and created date', () => {
    const meta = positionMetaLine(basePosition({ state: 'Armed' }));
    expect(meta).toContain('State Armed');
    expect(meta).toContain('Created 2026-04-23 12:00:00 UTC');
  });

  it('includes closed date when present', () => {
    const meta = positionMetaLine(
      basePosition({ closed_at: '2026-04-23T18:30:00Z' })
    );
    expect(meta).toContain('Closed 2026-04-23 18:30:00 UTC');
  });
});

// --- eventSummaryText ---

describe('eventSummaryText', () => {
  it('extracts symbol, side, prices, pnl', () => {
    const text = eventSummaryText(
      makeEvent({
        symbol: 'ETHUSDT',
        side: 'Short',
        entry_price: 3000,
        stop_price: 3100,
        exit_price: 2900,
        realized_pnl: 3.33
      })
    );
    expect(text).toContain('ETHUSDT');
    expect(text).toContain('Short');
    expect(text).toContain('entry 3,000.00');
    expect(text).toContain('stop 3,100.00');
    expect(text).toContain('exit 2,900.00');
    expect(text).toContain('pnl +3.33%');
  });

  it('handles minimal payload', () => {
    expect(eventSummaryText(makeEvent({}))).toBe('');
  });

  it('includes reason and new_state', () => {
    const text = eventSummaryText(makeEvent({ reason: 'trailing_stop', new_state: 'Closed' }));
    expect(text).toContain('trailing_stop');
    expect(text).toContain('Closed');
  });
});

// --- eventTypeLabel ---

describe('eventTypeLabel', () => {
  it('replaces dots with spaces and uppercases', () => {
    expect(eventTypeLabel(makeEvent({}))).toBe('POSITION ENTERED');
  });
});

// --- isPositionActive ---

describe('isPositionActive', () => {
  it('Armed is active', () => expect(isPositionActive('Armed')).toBe(true));
  it('Active string state is active', () => expect(isPositionActive('Active')).toBe(true));
  it('Entering is active', () =>
    expect(
      isPositionActive({
        Entering: { entry_order_id: 'x', expected_entry: 1, signal_id: 'x' }
      })
    ).toBe(true));
  it('Active is active', () =>
    expect(
      isPositionActive({
        Active: {
          current_price: 1,
          trailing_stop: 1,
          favorable_extreme: 1,
          extreme_at: '',
          insurance_stop_id: null,
          last_emitted_stop: null
        }
      })
    ).toBe(true));
  it('Closed is not active', () =>
    expect(
      isPositionActive({ Closed: { exit_price: 1, realized_pnl: 1, exit_reason: 'x' } })
    ).toBe(false));
  it('Error is not active', () =>
    expect(isPositionActive({ Error: { error: 'x', recoverable: false } })).toBe(false));
});

// --- positionLabel ---

describe('positionLabel', () => {
  it('formats symbol and side', () => {
    expect(positionLabel(basePosition({ symbol: 'BTCUSDT', side: 'Long' }))).toBe(
      'BTCUSDT · Long'
    );
  });
});

// --- positionStateLabel ---

describe('positionStateLabel', () => {
  it('returns string state directly', () => expect(positionStateLabel('Armed')).toBe('Armed'));
  it('extracts key from object state', () =>
    expect(positionStateLabel({ Closed: { exit_price: 1, realized_pnl: 1, exit_reason: 'x' } })).toBe('Closed'));
});
