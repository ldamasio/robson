import { describe, it, expect } from 'vitest';
import { OPERATION_REFRESH_MS, STATUS_POLL_MS, operationNeedsRefresh, shouldPollStatus } from '#lib/config/polling.ts';

describe('status polling policy', () => {
  it('polls only with an open position in a visible tab', () => {
    expect(shouldPollStatus(1, 'visible')).toBe(true);
    expect(shouldPollStatus(2, 'visible')).toBe(true);
  });

  it('does not poll with an empty book', () => {
    expect(shouldPollStatus(0, 'visible')).toBe(false);
  });

  it('does not poll while the tab is hidden', () => {
    expect(shouldPollStatus(1, 'hidden')).toBe(false);
  });

  it('keeps the cadence slow enough to stay off the hot path', () => {
    expect(STATUS_POLL_MS).toBeGreaterThanOrEqual(15_000);
  });
});

describe('operation details refresh policy', () => {
  const event = (event_type: string, payload: Record<string, unknown>) => ({ event_type, payload });

  it('refreshes on any event for this position', () => {
    expect(operationNeedsRefresh(event('position.changed', { position_id: 'op-1' }), 'op-1')).toBe(true);
    expect(operationNeedsRefresh(event('position.stop_advanced', { position_id: 'op-1' }), 'op-1')).toBe(true);
  });

  it('ignores events for other positions', () => {
    expect(operationNeedsRefresh(event('position.changed', { position_id: 'op-2' }), 'op-1')).toBe(false);
    expect(operationNeedsRefresh(event('query.created', {}), 'op-1')).toBe(false);
  });

  it('refreshes when the server asks for a resync', () => {
    expect(operationNeedsRefresh(event('system.resync_required', {}), 'op-1')).toBe(true);
  });

  it('never matches an empty operation id', () => {
    expect(operationNeedsRefresh(event('position.changed', { position_id: '' }), '')).toBe(false);
  });

  it('coalesces refreshes into a bounded window', () => {
    expect(OPERATION_REFRESH_MS).toBeGreaterThanOrEqual(1_000);
  });
});
