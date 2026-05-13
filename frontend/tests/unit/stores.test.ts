import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { recentEvents, pushEvent, clearEvents } from '$stores/events';
import { activePositions, upsertPosition, removePosition } from '$stores/operations';
import type { SseEvent, Position, PositionState } from '$api/robson';

function makeEvent(id: string): SseEvent {
  return { event_id: id, event_type: 'position.opened', occurred_at: '2026-04-23T12:00:00Z', payload: {} };
}

function makePosition(id: string, state: PositionState = 'Armed'): Position {
  return {
    id,
    account_id: 'acc-1',
    symbol: 'BTCUSDT',
    side: 'Long',
    state,
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
    closed_at: null
  };
}

// --- Events store ---

describe('events store', () => {
  beforeEach(() => clearEvents());

  it('starts empty', () => {
    expect(get(recentEvents)).toEqual([]);
  });

  it('pushes event to front', () => {
    pushEvent(makeEvent('a'));
    pushEvent(makeEvent('b'));
    const events = get(recentEvents);
    expect(events).toHaveLength(2);
    expect(events[0].event_id).toBe('b');
    expect(events[1].event_id).toBe('a');
  });

  it('caps at 100 events', () => {
    for (let i = 0; i < 120; i++) pushEvent(makeEvent(String(i)));
    expect(get(recentEvents)).toHaveLength(100);
    // Most recent first
    expect(get(recentEvents)[0].event_id).toBe('119');
  });

  it('clearEvents resets to empty', () => {
    pushEvent(makeEvent('x'));
    clearEvents();
    expect(get(recentEvents)).toEqual([]);
  });
});

// --- Operations store ---

describe('operations store', () => {
  beforeEach(() => activePositions.set([]));

  it('starts empty', () => {
    expect(get(activePositions)).toEqual([]);
  });

  it('upsert adds new position', () => {
    upsertPosition(makePosition('p1'));
    expect(get(activePositions)).toHaveLength(1);
    expect(get(activePositions)[0].id).toBe('p1');
  });

  it('upsert updates existing position by id', () => {
    upsertPosition(makePosition('p1', 'Armed'));
    const updated = makePosition('p1', { Entering: { entry_order_id: 'o1', expected_entry: 65000, signal_id: 's1' } });
    upsertPosition(updated);
    const positions = get(activePositions);
    expect(positions).toHaveLength(1);
    expect(typeof positions[0].state).toBe('object');
  });

  it('remove deletes position by id', () => {
    upsertPosition(makePosition('p1'));
    upsertPosition(makePosition('p2'));
    removePosition('p1');
    const positions = get(activePositions);
    expect(positions).toHaveLength(1);
    expect(positions[0].id).toBe('p2');
  });

  it('remove is no-op for missing id', () => {
    upsertPosition(makePosition('p1'));
    removePosition('nonexistent');
    expect(get(activePositions)).toHaveLength(1);
  });
});
