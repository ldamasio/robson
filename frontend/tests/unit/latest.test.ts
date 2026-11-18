import { describe, expect, it } from 'vitest';
import { latestOnly } from '#lib/utils/latest.ts';

describe('latest-only response ordering', () => {
  it('drops an older response that arrives after a newer request began', () => {
    const latest = latestOnly();
    const older = latest.begin();
    const newer = latest.begin();
    expect(newer()).toBe(true);
    expect(older()).toBe(false);
  });

  it('keeps a lone request current until another begins', () => {
    const latest = latestOnly();
    const only = latest.begin();
    expect(only()).toBe(true);
    expect(only()).toBe(true);
    latest.begin();
    expect(only()).toBe(false);
  });
});
