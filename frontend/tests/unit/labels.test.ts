import { describe, it, expect } from 'vitest';
import {
  positionSummaryLines,
  positionMetaLine,
  eventSummaryText,
  eventTypeLabel,
  isPositionActive,
  positionLabel,
  positionStateLabel,
  trailingStopMoveTarget,
  trailingLadderRung
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

describe('trailingStopMoveTarget', () => {
  it('uses the executable span and entry anchor before the first long advance', () => {
    const target = trailingStopMoveTarget(
      basePosition({
        state: 'Active',
        entry_price: 100,
        entry_reference: 100,
        executable_span: 12,
        trailing_stop: 90,
        tech_stop_distance: 10
      })
    );

    expect(target).toEqual({ trigger_price: 112, next_stop: 100 });
  });

  it('uses completed executable spans after a long advance', () => {
    const target = trailingStopMoveTarget(
      basePosition({
        state: 'Active',
        entry_price: 100,
        entry_reference: 100,
        executable_span: 12,
        trailing_stop: 100,
        tech_stop_distance: 10
      })
    );

    expect(target).toEqual({ trigger_price: 124, next_stop: 112 });
  });

  it('uses favorable extreme as the completed-span source when present', () => {
    const state: PositionState = {
      Active: {
        current_price: 128,
        trailing_stop: 100,
        favorable_extreme: 130,
        extreme_at: '2026-08-04T12:00:00Z',
        insurance_stop_id: 'ins-1',
        last_emitted_stop: 100
      }
    };
    const target = trailingStopMoveTarget(
      basePosition({
        state,
        entry_price: 101,
        entry_reference: 100,
        executable_span: 12,
        tech_stop_distance: 10
      })
    );

    expect(target).toEqual({ trigger_price: 136, next_stop: 124 });
  });

  it('does not require fill entry price for the executable-span branch', () => {
    const target = trailingStopMoveTarget(
      basePosition({
        state: 'Active',
        entry_price: null,
        entry_reference: 100,
        executable_span: 12,
        trailing_stop: 90,
        tech_stop_distance: 10
      })
    );

    expect(target).toEqual({ trigger_price: 112, next_stop: 100 });
  });

  it('mirrors the entry-anchored formulas for shorts', () => {
    const target = trailingStopMoveTarget(
      basePosition({
        state: 'Active',
        side: 'Short',
        entry_price: 100,
        entry_reference: 100,
        executable_span: 12,
        trailing_stop: 110,
        tech_stop_distance: 10
      })
    );

    expect(target).toEqual({ trigger_price: 88, next_stop: 100 });
  });

  it('preserves the legacy raw-span calculation when S is absent', () => {
    const target = trailingStopMoveTarget(
      basePosition({
        state: 'Active',
        entry_price: 100,
        executable_span: null,
        trailing_stop: 90,
        tech_stop_distance: 10
      })
    );

    expect(target).toEqual({ trigger_price: 110, next_stop: 100 });
  });

  it('does not fall back to legacy geometry when executable-span evidence is incomplete', () => {
    const target = trailingStopMoveTarget(
      basePosition({
        state: 'Active',
        entry_price: 100,
        entry_reference: null,
        executable_span: 12,
        trailing_stop: 90,
        tech_stop_distance: 10
      })
    );

    expect(target).toBeNull();
  });
});

// --- positionSummaryLines ---

describe('trailingLadderRung', () => {
  it('reports the rung, next trigger and distance for a short on the second rung', () => {
    // Live geometry from the 2026-10-05 BTCUSDT short: E 85,879.90, S 1,183.20,
    // technical stop at E - S after the price touched 83,500.
    const rung = trailingLadderRung(
      basePosition({
        state: 'Active',
        side: 'Short',
        entry_price: 85879.9,
        entry_reference: 85879.9,
        executable_span: 1183.2,
        trailing_stop: 84696.7,
        current_price: 83120.8,
        tech_stop_distance: 1096.2
      })
    );

    expect(rung).not.toBeNull();
    expect(rung?.completed_spans).toBe(2);
    expect(rung?.favorable_extreme).toBeNull();
    expect(rung?.next_trigger).toBeCloseTo(82330.3, 6);
    // (83,120.80 - 82,330.30) / 83,120.80
    expect(rung?.distance_to_next_pct).toBeCloseTo(0.951, 2);
  });

  it('is rung 0 before the first advance and uses the favorable extreme when present', () => {
    const state: PositionState = {
      Active: {
        current_price: 108,
        trailing_stop: 90,
        favorable_extreme: 110,
        extreme_at: '2026-08-04T12:00:00Z',
        insurance_stop_id: 'ins-1',
        last_emitted_stop: null
      }
    };
    const rung = trailingLadderRung(
      basePosition({ state, entry_price: 100, entry_reference: 100, executable_span: 12 })
    );

    expect(rung).toEqual({
      completed_spans: 0,
      favorable_extreme: 110,
      next_trigger: 112,
      distance_to_next_pct: expect.closeTo((112 - 108) / 108 * 100, 6)
    });
  });

  it('clamps the distance at zero once the price is past the next trigger', () => {
    const rung = trailingLadderRung(
      basePosition({
        state: 'Active',
        entry_price: 100,
        entry_reference: 100,
        executable_span: 12,
        trailing_stop: 100,
        current_price: 125
      })
    );

    expect(rung?.completed_spans).toBe(1);
    expect(rung?.next_trigger).toBe(124);
    expect(rung?.distance_to_next_pct).toBe(0);
  });

  it('leaves the distance null when the current price is unknown', () => {
    const rung = trailingLadderRung(
      basePosition({
        state: 'Active',
        entry_price: 100,
        entry_reference: 100,
        executable_span: 12,
        trailing_stop: 90
      })
    );

    expect(rung?.distance_to_next_pct).toBeNull();
  });

  it('has no ruler on the legacy path or outside Active', () => {
    expect(
      trailingLadderRung(
        basePosition({ state: 'Active', entry_price: 100, trailing_stop: 90, tech_stop_distance: 10 })
      )
    ).toBeNull();
    expect(
      trailingLadderRung(
        basePosition({ state: 'Armed', entry_reference: 100, executable_span: 12, trailing_stop: 90 })
      )
    ).toBeNull();
  });
});

describe('positionSummaryLines', () => {
  it('renders the RUNG line after TARGET on the executable-span path', () => {
    const lines = positionSummaryLines(
      basePosition({
        state: 'Active',
        side: 'Short',
        entry_price: 85879.9,
        entry_reference: 85879.9,
        executable_span: 1183.2,
        trailing_stop: 84696.7,
        effective_stop: 84781.4,
        current_price: 83120.8,
        quantity: 0.012
      })
    );

    expect(lines[0]).toContain('ACTIVE');
    expect(lines[1]).toContain('TARGET');
    expect(lines[1]).toContain('82,330.30 -> stop 83,513.50');
    expect(lines[2]).toMatch(/^RUNG\s+2 · next 82,330.30 \(0\.95% away\)$/);
    expect(lines[3]).toContain('ENTRY');
    expect(lines[4]).toContain('SIZE');
  });

  it('folds the favorable extreme into the RUNG line instead of a separate EXTREME line', () => {
    const state: PositionState = {
      Active: {
        current_price: 128,
        trailing_stop: 100,
        favorable_extreme: 130,
        extreme_at: '2026-08-04T12:00:00Z',
        insurance_stop_id: 'ins-1',
        last_emitted_stop: 100
      }
    };
    const lines = positionSummaryLines(
      basePosition({ state, entry_price: 101, entry_reference: 100, executable_span: 12 })
    );

    const rungLine = lines.find((l) => l.startsWith('RUNG'));
    expect(rungLine).toBeDefined();
    expect(rungLine).toContain('2 · extreme 130.00 · next 136.00');
    expect(lines.some((l) => l.startsWith('EXTREME'))).toBe(false);
  });


  it('Armed state', () => {
    const lines = positionSummaryLines(basePosition({ state: 'Armed' }));
    expect(lines).toHaveLength(2);
    expect(lines[0]).toContain('ARMED');
    expect(lines[0]).toContain('awaiting entry signal');
    expect(lines[1]).toContain('LEVERAGE');
    expect(lines[1]).toContain('1x (fixed)');
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

  it('Active state shows the executable stop and guard level when the invalidation guard binds', () => {
    const state: PositionState = {
      Active: {
        current_price: 62900,
        trailing_stop: 62984.17,
        favorable_extreme: 62800,
        extreme_at: '2026-07-04T17:10:00Z',
        insurance_stop_id: 'ins-1',
        last_emitted_stop: null
      }
    };
    const lines = positionSummaryLines(
      basePosition({
        state,
        side: 'Short',
        entry_price: 62885.6,
        quantity: 0.025,
        effective_stop: 63132.67,
        raw_technical_stop: 62984.17,
        invalidation_guard_level: 63069.6,
        effective_stop_basis: 'invalidation_guard'
      })
    );
    expect(lines[0]).toContain('ACTIVE');
    expect(lines[0]).toContain('stop 63,132.67');
    expect(lines[0]).toContain('(guard 63,069.60)');
    expect(lines[0]).not.toContain('62,984.17');
  });

  it('Active string state prefers effective_stop over trailing_stop without guard suffix', () => {
    const lines = positionSummaryLines(
      basePosition({
        state: 'Active',
        entry_price: 63000,
        trailing_stop: 62000,
        effective_stop: 61938
      })
    );
    expect(lines[0]).toContain('ACTIVE');
    expect(lines[0]).toContain('stop 61,938.00');
    expect(lines[0]).not.toContain('guard');
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
