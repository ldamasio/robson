import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('$app/env', () => ({ browser: true }));
type Response = { credential?: string };
type Config = { client_id: string; callback: (response: Response) => void };
type Moment = { isNotDisplayed?: () => boolean };
let callbacks: Array<(response: Response) => void>;
let initialize: ReturnType<typeof vi.fn>;
let prompt: ReturnType<typeof vi.fn>;
let cancel: ReturnType<typeof vi.fn>;
let storage: Map<string, string>;
let auth: typeof import('$stores/auth');

beforeEach(async () => {
  vi.resetModules();
  vi.useFakeTimers();
  callbacks = [];
  storage = new Map();
  initialize = vi.fn((config: Config) => { callbacks.push(config.callback); });
  prompt = vi.fn((_listener: (moment: Moment) => void) => {});
  cancel = vi.fn();
  vi.stubGlobal('sessionStorage', {
    getItem: (key: string) => storage.get(key) ?? null,
    setItem: (key: string, value: string) => { storage.set(key, value); },
    removeItem: (key: string) => { storage.delete(key); }
  });
  vi.stubGlobal('window', { google: { accounts: { id: { initialize, prompt, cancel } } } });
  auth = await import('$stores/auth');
});

afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe('silent refresh settlement ownership', () => {
  it('does not restore auth when a credential arrives after timeout', async () => {
    const result = auth.silentRefreshGoogleToken('fixture-client');
    await vi.advanceTimersByTimeAsync(5000);
    await expect(result).resolves.toBeNull();
    expect(cancel).toHaveBeenCalledTimes(1);
    callbacks[0]({ credential: 'late-fixture-token' });
    expect(get(auth.authToken)).toBeNull();
    expect(storage.get('robson_google_id_token')).toBeUndefined();
    expect(vi.getTimerCount()).toBe(0);
  });

  it('owns null before cancellation can synchronously deliver a credential', async () => {
    cancel.mockImplementation(() => callbacks[0]({ credential: 'cancel-reentrant-token' }));
    const result = auth.silentRefreshGoogleToken('fixture-client');
    await vi.advanceTimersByTimeAsync(5000);
    await expect(result).resolves.toBeNull();
    expect(get(auth.authToken)).toBeNull();
    expect(storage.has('robson_google_id_token')).toBe(false);
    expect(cancel).toHaveBeenCalledTimes(1);
  });

  it('clears its timer when the moment listener settles synchronously', async () => {
    prompt.mockImplementation((listener: (moment: Moment) => void) => listener({ isNotDisplayed: () => true }));
    await expect(auth.silentRefreshGoogleToken('fixture-client')).resolves.toBeNull();
    expect(vi.getTimerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(5000);
    expect(cancel).not.toHaveBeenCalled();
  });

  it('shares one SDK request and publishes only the winning credential', async () => {
    const results = [auth.silentRefreshGoogleToken('fixture-client'), auth.silentRefreshGoogleToken('fixture-client'), auth.silentRefreshGoogleToken('fixture-client')];
    expect(initialize).toHaveBeenCalledTimes(1);
    expect(prompt).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(1);
    callbacks[0]({ credential: 'winning-fixture-token' });
    expect(await Promise.all(results)).toEqual(Array(3).fill('winning-fixture-token'));
    callbacks[0]({ credential: 'duplicate-fixture-token' });
    expect(get(auth.authToken)).toBe('winning-fixture-token');
    expect(storage.get('robson_google_id_token')).toBe('winning-fixture-token');
    expect(vi.getTimerCount()).toBe(0);
  });

  it('preserves initialization rejection without leaving a timer', async () => {
    initialize.mockImplementation(() => { throw new Error('fixture initialize failure'); });
    await expect(auth.silentRefreshGoogleToken('fixture-client')).rejects.toThrow('fixture initialize failure');
    expect(vi.getTimerCount()).toBe(0);
    expect(prompt).not.toHaveBeenCalled();
    expect(get(auth.authToken)).toBeNull();
  });

  it('settles every waiter when publishing the credential fails', async () => {
    vi.stubGlobal('sessionStorage', {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: () => { throw new Error('fixture storage failure'); },
      removeItem: (key: string) => { storage.delete(key); }
    });
    const results = [auth.silentRefreshGoogleToken('fixture-client'), auth.silentRefreshGoogleToken('fixture-client')];
    callbacks[0]({ credential: 'unstorable-fixture-token' });
    for (const result of results) await expect(result).rejects.toThrow('fixture storage failure');
    expect(get(auth.authToken)).toBeNull();
    expect(vi.getTimerCount()).toBe(0);
    const next = auth.silentRefreshGoogleToken('fixture-client');
    expect(initialize).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(5000);
    await expect(next).resolves.toBeNull();
  });

  it('preserves prompt rejection and permits a later independent refresh', async () => {
    prompt.mockImplementationOnce(() => { throw new Error('fixture prompt failure'); });
    await expect(auth.silentRefreshGoogleToken('fixture-client')).rejects.toThrow('fixture prompt failure');
    expect(vi.getTimerCount()).toBe(0);
    const next = auth.silentRefreshGoogleToken('fixture-client');
    callbacks[1]({ credential: 'next-fixture-token' });
    await expect(next).resolves.toBe('next-fixture-token');
    expect(vi.getTimerCount()).toBe(0);
  });
});
