import { describe, it, expect } from 'vitest';
import { deriveSlots, occupiedCount, SLOT_COUNT } from '$lib/config/slots';
import type { PositionState } from '$api/robson';

const activeState: PositionState = {
  Active: {
    current_price: 100,
    trailing_stop: 95,
    favorable_extreme: 110,
    extreme_at: '',
    insurance_stop_id: null,
    last_emitted_stop: null
  }
};

describe('deriveSlots', () => {
  it('returns 6 empty slots when no positions', () => {
    const result = deriveSlots([]);
    expect(result).toHaveLength(SLOT_COUNT);
    expect(result.every((s) => !s.occupied)).toBe(true);
  });

  it('marks first N slots occupied for N active positions', () => {
    const positions = [
      { id: 'a', state: 'Armed' as PositionState },
      { id: 'b', state: activeState }
    ];
    const result = deriveSlots(positions);
    expect(result.filter((s) => s.occupied)).toHaveLength(2);
    expect(result[0].positionId).toBe('a');
    expect(result[1].positionId).toBe('b');
    expect(result[2].occupied).toBe(false);
  });

  it('excludes Closed/Error positions', () => {
    const positions = [
      { id: 'x', state: { Closed: { exit_price: 100, realized_pnl: 0, exit_reason: 'stop_hit' } } as PositionState },
      { id: 'y', state: { Error: { error: 'test', recoverable: false } } as PositionState }
    ];
    const result = deriveSlots(positions);
    expect(result.every((s) => !s.occupied)).toBe(true);
  });

  it('caps at SLOT_COUNT even with more than 6 active positions', () => {
    const positions = Array.from({ length: 8 }, (_, i) => ({
      id: `p-${i}`,
      state: activeState
    }));
    const result = deriveSlots(positions);
    expect(result).toHaveLength(SLOT_COUNT);
    expect(result.every((s) => s.occupied)).toBe(true);
  });
});

describe('occupiedCount', () => {
  it('counts Armed + Entering + Active only', () => {
    const positions = [
      { state: 'Armed' as PositionState },
      { state: { Entering: { entry_order_id: '1', expected_entry: 100, signal_id: 's' } } as PositionState },
      { state: activeState },
      { state: { Closed: { exit_price: 100, realized_pnl: 0, exit_reason: 'stop_hit' } } as PositionState },
      { state: { Exiting: { exit_order_id: '2', exit_reason: 'stop_hit' } } as PositionState }
    ];
    expect(occupiedCount(positions)).toBe(3);
  });

  it('returns count > SLOT_COUNT when more than 6 active', () => {
    const positions = Array.from({ length: 8 }, () => ({ state: activeState }));
    expect(occupiedCount(positions)).toBe(8);
  });
});
