import { describe, expect, it } from 'vitest';
import { serialRefresh } from '#lib/utils/refresh.ts';

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((res) => { resolve = res; });
  return { promise, resolve };
}

const flush = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

describe('serialized snapshot refresh', () => {
  it('never overlaps runs and coalesces triggers into one follow-up', async () => {
    const gates: Array<ReturnType<typeof deferred>> = [];
    let active = 0;
    let maxActive = 0;
    const refresh = serialRefresh(async () => {
      active += 1;
      maxActive = Math.max(maxActive, active);
      const gate = deferred();
      gates.push(gate);
      await gate.promise;
      active -= 1;
    });
    const done = refresh();
    refresh();
    refresh();
    refresh();
    expect(gates).toHaveLength(1);
    gates[0].resolve();
    await flush();
    expect(gates).toHaveLength(2);
    gates[1].resolve();
    await done;
    expect(gates).toHaveLength(2);
    expect(maxActive).toBe(1);
  });

  it('keeps publishing under a sustained trigger stream', async () => {
    const published: number[] = [];
    let gate = deferred();
    let n = 0;
    const refresh = serialRefresh(async () => {
      const mine = ++n;
      await gate.promise;
      published.push(mine);
    });
    refresh();
    for (let round = 0; round < 5; round += 1) {
      refresh();
      const current = gate;
      gate = deferred();
      current.resolve();
      await flush();
    }
    gate.resolve();
    await flush();
    expect(published.length).toBeGreaterThanOrEqual(5);
    expect(published).toEqual([...published].sort((a, b) => a - b));
  });

  it('starts a fresh run after the previous one settled', async () => {
    let runs = 0;
    const refresh = serialRefresh(async () => { runs += 1; });
    await refresh();
    await refresh();
    expect(runs).toBe(2);
  });
});
