import { describe, it, expect } from 'vitest';
import { STATUS_POLL_MS, shouldPollStatus } from '$lib/config/polling';

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
