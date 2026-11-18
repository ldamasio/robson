import { describe, expect, it } from 'vitest';
import { latestOnly } from '#lib/utils/latest.ts';

describe('latest-only selection reads', () => {
  it('drops an older response that arrives after a newer request began', () => {
    const latest = latestOnly();
    const october = latest.begin();
    const september = latest.begin();
    expect(september()).toBe(true);
    expect(october()).toBe(false);
  });

  it('keeps a lone request current until another begins', () => {
    const latest = latestOnly();
    const only = latest.begin();
    expect(only()).toBe(true);
    latest.begin();
    expect(only()).toBe(false);
  });
});
