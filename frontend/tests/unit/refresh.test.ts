import { describe, expect, it } from 'vitest';
import { selectionRefresh, serialRefresh } from '#lib/utils/refresh.ts';

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

describe('selection-bound history refresh', () => {
  function harness() {
    let selected = '2026-10';
    const reads: Array<{ month: string; resolve: (v: string) => void; reject: (e: Error) => void }> = [];
    const published: string[] = [];
    const failed: string[] = [];
    const refresh = selectionRefresh<string, string>({
      selection: () => selected,
      read: (month) => new Promise<string>((resolve, reject) => { reads.push({ month, resolve, reject }); }),
      publish: (month, result) => published.push(`${month}:${result}`),
      fail: (month) => failed.push(month),
    });
    return { refresh, reads, published, failed, select: (m: string) => { selected = m; } };
  }

  it('keeps publishing the current selection under sustained refreshes', async () => {
    const h = harness();
    h.select('2026-09');
    h.refresh();
    for (let round = 0; round < 4; round += 1) {
      h.refresh();
      h.reads[round].resolve(`r${round}`);
      await flush();
    }
    expect(h.published).toEqual(['2026-09:r0', '2026-09:r1', '2026-09:r2', '2026-09:r3']);
  });

  it('drops a response for a selection changed in flight and reads the new one', async () => {
    const h = harness();
    h.refresh();
    h.select('2026-09');
    h.refresh();
    h.reads[0].resolve('october-data');
    await flush();
    expect(h.published).toEqual([]);
    expect(h.reads[1].month).toBe('2026-09');
    h.reads[1].resolve('september-data');
    await flush();
    expect(h.published).toEqual(['2026-09:september-data']);
  });

  it('reports an error only for the current selection', async () => {
    const h = harness();
    h.refresh();
    h.select('2026-09');
    h.refresh();
    h.reads[0].reject(new Error('fixture october failure'));
    await flush();
    expect(h.failed).toEqual([]);
    h.reads[1].reject(new Error('fixture september failure'));
    await flush();
    expect(h.failed).toEqual(['2026-09']);
  });
});
