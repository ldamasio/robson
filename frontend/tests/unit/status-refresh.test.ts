import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';

type Reply = { resolve: (value: unknown) => void; reject: (error: Error) => void };
const replies: Reply[] = [];
let active = 0;
let maxActive = 0;

vi.mock('$api/robson', () => ({
  robsonApi: {
    getStatus: () => {
      active += 1;
      maxActive = Math.max(maxActive, active);
      return new Promise((resolve, reject) => {
        replies.push({
          resolve: (value) => { active -= 1; resolve(value); },
          reject: (error) => { active -= 1; reject(error); },
        });
      });
    },
  },
}));

let store: typeof import('$stores/status');
const flush = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

beforeEach(async () => {
  vi.resetModules();
  replies.length = 0;
  active = 0;
  maxActive = 0;
  store = await import('$stores/status');
});

describe('shared status refresh', () => {
  it('answers a trigger made during a read with a later read, not the earlier one', async () => {
    const bootstrap = store.refreshStatus();
    const anchor = store.refreshStatus();
    expect(replies).toHaveLength(1);
    replies[0].resolve({ positions: [], tag: 'pre-subscription' });
    await flush();
    expect(replies).toHaveLength(2);
    replies[1].resolve({ positions: [{ id: 'p' }], tag: 'post-subscription' });
    await expect(anchor).resolves.toMatchObject({ tag: 'post-subscription' });
    await expect(bootstrap).resolves.toMatchObject({ tag: 'post-subscription' });
    expect(get(store.status)).toMatchObject({ tag: 'post-subscription' });
    expect(maxActive).toBe(1);
  });

  it('coalesces several triggers into one follow-up read', async () => {
    const first = store.refreshStatus();
    store.refreshStatus();
    store.refreshStatus();
    replies[0].resolve({ tag: 'a' });
    await flush();
    expect(replies).toHaveLength(2);
    replies[1].resolve({ tag: 'b' });
    await first;
    expect(replies).toHaveLength(2);
  });

  it('rejects when the last read fails and allows a fresh read afterwards', async () => {
    const failed = store.refreshStatus();
    replies[0].reject(new Error('fixture status failure'));
    await expect(failed).rejects.toThrow('fixture status failure');
    const next = store.refreshStatus();
    replies[1].resolve({ tag: 'recovered' });
    await expect(next).resolves.toMatchObject({ tag: 'recovered' });
  });
});
