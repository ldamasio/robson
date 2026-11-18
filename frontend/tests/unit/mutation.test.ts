import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';

let mutation: typeof import('$stores/mutation');

function deferred() {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

beforeEach(async () => {
  vi.resetModules();
  mutation = await import('$stores/mutation');
});

describe('app-wide exclusive mutation', () => {
  it('refuses a second submission while the first is pending', async () => {
    const pending = deferred();
    const action = vi.fn((_id: string) => pending.promise);
    const run = mutation.exclusiveMutation(action);
    const first = run('fixture-a');
    const second = run('fixture-b');
    expect(action).toHaveBeenCalledTimes(1);
    expect(action).toHaveBeenCalledWith('fixture-a');
    expect(get(mutation.mutationPending)).toBe(true);
    pending.resolve();
    await Promise.all([first, second]);
    expect(get(mutation.mutationPending)).toBe(false);
    await run('fixture-c');
    expect(action).toHaveBeenCalledTimes(2);
  });

  it('refuses a resubmission from a recreated page while the first is pending', async () => {
    const pending = deferred();
    const original = mutation.exclusiveMutation((_id: string) => pending.promise);
    const recreated = vi.fn(async (_id: string) => {});
    const first = original('approval-1');
    await mutation.exclusiveMutation(recreated)('approval-1');
    expect(recreated).not.toHaveBeenCalled();
    pending.resolve();
    await first;
  });

  it('releases the freeze when the action fails', async () => {
    const pending = deferred();
    const run = mutation.exclusiveMutation(() => pending.promise);
    const first = run();
    pending.reject(new Error('fixture failure'));
    await expect(first).rejects.toThrow('fixture failure');
    expect(get(mutation.mutationPending)).toBe(false);
    const next = vi.fn(async () => {});
    await mutation.exclusiveMutation(next)();
    expect(next).toHaveBeenCalledTimes(1);
  });
});
