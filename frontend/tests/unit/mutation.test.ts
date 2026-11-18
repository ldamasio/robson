import { describe, expect, it, vi } from 'vitest';
import { singleFlight } from '#lib/utils/mutation.ts';

function deferred() {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

describe('single-flight mutation guard', () => {
  it('refuses a second submission while the first is pending', async () => {
    const pending = deferred();
    const action = vi.fn((_id: string) => pending.promise);
    const busy: boolean[] = [];
    const run = singleFlight(action, (value) => busy.push(value));
    const first = run('fixture-a');
    const second = run('fixture-b');
    expect(action).toHaveBeenCalledTimes(1);
    expect(action).toHaveBeenCalledWith('fixture-a');
    pending.resolve();
    await Promise.all([first, second]);
    expect(busy).toEqual([true, false]);
    await run('fixture-c');
    expect(action).toHaveBeenCalledTimes(2);
  });

  it('releases the guard when the action fails', async () => {
    const pending = deferred();
    const action = vi.fn(() => pending.promise);
    const busy: boolean[] = [];
    const run = singleFlight(action, (value) => busy.push(value));
    const first = run();
    pending.reject(new Error('fixture failure'));
    await expect(first).rejects.toThrow('fixture failure');
    expect(busy).toEqual([true, false]);
    action.mockImplementationOnce(async () => {});
    await run();
    expect(action).toHaveBeenCalledTimes(2);
  });
});
