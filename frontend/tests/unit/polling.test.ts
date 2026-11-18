import { describe, it, expect } from 'vitest';
import { OPERATION_REFRESH_MS, STATUS_POLL_MS, dashboardNeedsRefresh, dashboardStreamHandlers, operationNeedsRefresh, operationStreamHandlers, shouldPollStatus } from '#lib/config/polling.ts';

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

describe('operation stream handlers', () => {
  function handlers() {
    const calls: string[] = [];
    const h = operationStreamHandlers('op-1', {
      refreshNow: () => calls.push('refreshNow'),
      scheduleRefresh: () => calls.push('schedule'),
      appendEvent: () => calls.push('append'),
      markEvent: () => calls.push('event'),
      markConnected: () => calls.push('connected'),
      markStale: () => calls.push('stale'),
    });
    return { h, calls };
  }

  it('re-reads the snapshot when the first connection opens', () => {
    const { h, calls } = handlers();
    h.onOpen();
    expect(calls).toEqual(['connected', 'refreshNow']);
  });

  it('re-reads the snapshot after every reconnect', () => {
    const { h, calls } = handlers();
    h.onReconnect();
    expect(calls).toEqual(['connected', 'refreshNow']);
  });

  it('schedules a refresh for its own position and ignores others', () => {
    const { h, calls } = handlers();
    h.onEvent({ event_type: 'position.changed', payload: { position_id: 'op-1' } });
    h.onEvent({ event_type: 'position.changed', payload: { position_id: 'op-2' } });
    expect(calls).toEqual(['event', 'append', 'schedule', 'event']);
  });

  it('marks the stream stale on error and on the idle watchdog', () => {
    const { h, calls } = handlers();
    h.onError();
    h.onStale();
    expect(calls).toEqual(['stale', 'stale']);
  });
});

describe('dashboard stream handlers', () => {
  function handlers() {
    const calls: string[] = [];
    const h = dashboardStreamHandlers({
      anchor: () => calls.push('anchor'),
      scheduleRefresh: () => calls.push('schedule'),
      pushEvent: () => calls.push('push'),
      markEvent: () => calls.push('event'),
      markConnected: () => calls.push('connected'),
      markStale: () => calls.push('stale'),
      setConnected: (value) => calls.push(`setConnected:${value}`),
    });
    return { h, calls };
  }

  it('anchors on the snapshot when the first connection opens', () => {
    const { h, calls } = handlers();
    h.onOpen();
    expect(calls).toEqual(['setConnected:true', 'connected', 'anchor']);
  });

  it('anchors again after every reconnect', () => {
    const { h, calls } = handlers();
    h.onReconnect();
    expect(calls).toEqual(['setConnected:true', 'connected', 'anchor']);
  });

  it('refreshes on lifecycle, approval, boundary and resync events only', () => {
    const ev = (event_type: string, payload: Record<string, unknown> = {}) => ({ event_type, payload });
    expect(dashboardNeedsRefresh(ev('position.changed', { position_id: 'p' }))).toBe(true);
    expect(dashboardNeedsRefresh(ev('position.changed'))).toBe(false);
    expect(dashboardNeedsRefresh(ev('query.awaiting_approval'))).toBe(true);
    expect(dashboardNeedsRefresh(ev('month_boundary.reset'))).toBe(true);
    expect(dashboardNeedsRefresh(ev('system.resync_required'))).toBe(true);
    expect(dashboardNeedsRefresh(ev('position.mark_advanced', { position_id: 'p' }))).toBe(false);
  });

  it('shows the stream down on error and stale', () => {
    const { h, calls } = handlers();
    h.onError();
    h.onStale();
    expect(calls).toEqual(['setConnected:false', 'stale', 'setConnected:false']);
  });
});
