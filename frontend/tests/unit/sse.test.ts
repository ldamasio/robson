import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { FetchEventSource } from '$api/robson';
import { markSseEvent, sseFreshness } from '$stores/sseFreshness';

/** Drain microtask queue: needed because vitest fake timers don't flush async chains. */
async function flush(rounds = 20): Promise<void> {
  for (let i = 0; i < rounds; i++) await Promise.resolve();
}

// Minimal stand-in for the EventSourceLike interface
type SseSource = {
  onmessage: ((ev: { data: string }) => void) | null;
  onerror: ((ev: Event) => void) | null;
  close: () => void;
};

// Helper: build a FetchEventSource wired to a fake fetch
function buildSource(
  fetchImpl: typeof fetch,
  onReconnect?: () => void,
  onStale?: (staleSecs: number) => void,
  onActivity?: () => void,
): SseSource {
  vi.stubGlobal('fetch', fetchImpl);
  return new FetchEventSource(
    'http://localhost/events',
    null,
    onReconnect,
    onStale,
    onActivity,
  ) as unknown as SseSource;
}

describe('FetchEventSource reconnect backoff', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('schedules reconnect after a non-ok response', async () => {
    let callCount = 0;
    const fakeFetch = vi.fn().mockImplementation(async () => {
      callCount++;
      return { ok: false, body: null } as unknown as Response;
    });

    buildSource(fakeFetch);

    // First call fires immediately in constructor
    await flush();
    expect(callCount).toBe(1);

    // Advance past first backoff window (1s * 2^0 = 1000ms)
    await vi.advanceTimersByTimeAsync(1_100);
    await flush();
    expect(callCount).toBe(2);

    // Second backoff is 2s
    await vi.advanceTimersByTimeAsync(2_100);
    await flush();
    expect(callCount).toBe(3);
  });

  it('resets retry counter after a successful stream', async () => {
    let callCount = 0;

    // First call succeeds (stream body that closes immediately)
    // Second call fails, then reconnects at 1s (not 4s, because retries were reset)
    const fakeFetch = vi.fn().mockImplementation(async () => {
      callCount++;
      if (callCount === 1) {
        // Simulate a readable stream that closes right away
        const encoder = new TextEncoder();
        const chunk = encoder.encode('data: {"event_id":"x"}\n\n');
        const stream = new ReadableStream({
          start(controller) {
            controller.enqueue(chunk);
            controller.close();
          },
        });
        return { ok: true, body: stream } as unknown as Response;
      }
      return { ok: false, body: null } as unknown as Response;
    });

    buildSource(fakeFetch);

    await flush();
    expect(callCount).toBe(1); // first call (succeeds)

    // Stream closed → schedules reconnect at 1s (retries reset to 0)
    await vi.advanceTimersByTimeAsync(1_100);
    await flush();
    expect(callCount).toBe(2); // reconnect (fails)

    // Next reconnect is again 1s * 2^1 = 2s (retries=1 after the failure)
    await vi.advanceTimersByTimeAsync(2_100);
    await flush();
    expect(callCount).toBe(3);
  });

  it('does not reconnect after close()', async () => {
    let callCount = 0;
    const fakeFetch = vi.fn().mockImplementation(async () => {
      callCount++;
      return { ok: false, body: null } as unknown as Response;
    });

    const src = buildSource(fakeFetch);

    await flush();
    expect(callCount).toBe(1);

    src.close();

    // Advancing time should NOT trigger another fetch
    await vi.advanceTimersByTimeAsync(60_000);
    await flush();
    expect(callCount).toBe(1);
  });

  it('fires onerror callback on connection failure', async () => {
    const fakeFetch = vi.fn().mockImplementation(async () => ({
      ok: false,
      body: null,
    } as unknown as Response));

    const errors: Event[] = [];
    const src = buildSource(fakeFetch);
    src.onerror = (e) => errors.push(e);

    await flush();
    expect(errors).toHaveLength(1);

    src.close();
  });


  it('notifies onReconnect after a recovered stream', async () => {
    let callCount = 0;
    const onReconnect = vi.fn();
    const fakeFetch = vi.fn().mockImplementation(async () => {
      callCount++;
      if (callCount === 1) {
        return { ok: false, body: null } as unknown as Response;
      }
      const encoder = new TextEncoder();
      const chunk = encoder.encode('data: {"event_id":"x"}\n\n');
      const stream = new ReadableStream({
        start(controller) {
          controller.enqueue(chunk);
          controller.close();
        },
      });
      return { ok: true, body: stream } as unknown as Response;
    });

    buildSource(fakeFetch, onReconnect);

    await flush();
    expect(callCount).toBe(1);
    expect(onReconnect).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(1_100);
    await flush();
    expect(callCount).toBe(2);
    expect(onReconnect).toHaveBeenCalledTimes(1);
  });

  it('caps backoff at 30s', async () => {
    let callCount = 0;
    const fakeFetch = vi.fn().mockImplementation(async () => {
      callCount++;
      return { ok: false, body: null } as unknown as Response;
    });

    const src = buildSource(fakeFetch);
    await flush();

    // Drive through 6 retries: 1s,2s,4s,8s,16s,32s→capped at 30s
    for (let i = 0; i < 6; i++) {
      await vi.advanceTimersByTimeAsync(32_000);
      await flush();
    }

    // 7th retry should fire after at most 30s, not 64s
    const before = callCount;
    await vi.advanceTimersByTimeAsync(30_100);
    await flush();
    expect(callCount).toBeGreaterThan(before);

    src.close();
  });
});

// --- Read-idle watchdog ---
describe('FetchEventSource read-idle watchdog', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('calls onStale and reconnects when no bytes arrive for 45s', async () => {
    let callCount = 0;
    const onStale = vi.fn();
    const fakeFetch = vi.fn().mockImplementation(async () => {
      callCount++;
      const stream = new ReadableStream({
        start() {
          // never enqueue; the watchdog should abort this idle read
        },
      });
      return { ok: true, body: stream } as unknown as Response;
    });

    buildSource(fakeFetch, undefined, onStale);

    await flush();
    expect(callCount).toBe(1);
    expect(onStale).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(30_000);
    await flush();
    expect(onStale).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(15_100);
    await flush();
    expect(onStale).toHaveBeenCalledTimes(1);
    expect(onStale).toHaveBeenCalledWith(45);

    // A reconnect should be scheduled at 1s backoff
    await vi.advanceTimersByTimeAsync(1_100);
    await flush();
    expect(callCount).toBe(2);
  });

  it('resets the idle timer on every chunk', async () => {
    let callCount = 0;
    const onStale = vi.fn();
    const encoder = new TextEncoder();
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    let streamCtrl: any = null;
    const fakeFetch = vi.fn().mockImplementation(async () => {
      callCount++;
      const stream = new ReadableStream<Uint8Array>({
        start(controller) {
          streamCtrl = controller;
        },
      });
      return { ok: true, body: stream } as unknown as Response;
    });

    buildSource(fakeFetch, undefined, onStale);

    await flush();
    expect(callCount).toBe(1);

    // 44s without data would normally trip the watchdog, but a chunk resets it
    await vi.advanceTimersByTimeAsync(44_000);
    await flush();
    streamCtrl?.enqueue(encoder.encode(':heartbeat\n\n'));
    await flush();
    expect(onStale).not.toHaveBeenCalled();

    // Watchdog fires 45s after the last received chunk
    await vi.advanceTimersByTimeAsync(45_100);
    await flush();
    expect(onStale).toHaveBeenCalledTimes(1);
  });

  it('notifies activity for comment-only heartbeat chunks', async () => {
    const onActivity = vi.fn();
    const encoder = new TextEncoder();
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    let streamCtrl: any = null;
    const fakeFetch = vi.fn().mockImplementation(async () => {
      const stream = new ReadableStream<Uint8Array>({
        start(controller) {
          streamCtrl = controller;
        },
      });
      return { ok: true, body: stream } as unknown as Response;
    });

    buildSource(fakeFetch, undefined, undefined, onActivity);

    await flush();
    expect(onActivity).not.toHaveBeenCalled();

    streamCtrl?.enqueue(encoder.encode(':heartbeat\n\n'));
    await flush();

    expect(onActivity).toHaveBeenCalledTimes(1);
  });
});

describe('SSE freshness store', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-07-06T00:00:00.000Z'));
  });

  afterEach(() => {
    sseFreshness.stop();
    vi.useRealTimers();
  });

  it('stays fresh when byte activity is marked within 45s without messages', async () => {
    let latest: { fresh: boolean; staleSeconds: number } | null = null;
    const unsubscribe = sseFreshness.subscribe((value) => {
      latest = value;
    });

    sseFreshness.start();
    markSseEvent();

    await vi.advanceTimersByTimeAsync(44_000);
    markSseEvent();
    await vi.advanceTimersByTimeAsync(44_000);

    expect(latest).toEqual({ fresh: true, staleSeconds: 44 });
    unsubscribe();
  });
});

// --- Operation page event cap logic ---
describe('operation page event cap', () => {
  it('caps event list at MAX_OP_EVENTS entries', () => {
    const MAX_OP_EVENTS = 500;
    let events: { _seq: number }[] = [];

    function pushEvent(seq: number) {
      const tagged = { _seq: seq };
      events = events.length >= MAX_OP_EVENTS
        ? [...events.slice(-(MAX_OP_EVENTS - 1)), tagged]
        : [...events, tagged];
    }

    for (let i = 1; i <= 600; i++) pushEvent(i);

    expect(events).toHaveLength(500);
    // Most recent event is at end
    expect(events[events.length - 1]._seq).toBe(600);
    // Oldest kept is seq 101 (600 - 499)
    expect(events[0]._seq).toBe(101);
  });

  it('does not cap when under the limit', () => {
    const MAX_OP_EVENTS = 500;
    let events: { _seq: number }[] = [];

    function pushEvent(seq: number) {
      const tagged = { _seq: seq };
      events = events.length >= MAX_OP_EVENTS
        ? [...events.slice(-(MAX_OP_EVENTS - 1)), tagged]
        : [...events, tagged];
    }

    for (let i = 1; i <= 10; i++) pushEvent(i);
    expect(events).toHaveLength(10);
    expect(events[9]._seq).toBe(10);
  });

  it('retains exactly MAX_OP_EVENTS on boundary push', () => {
    const MAX_OP_EVENTS = 500;
    let events: { _seq: number }[] = [];

    function pushEvent(seq: number) {
      const tagged = { _seq: seq };
      events = events.length >= MAX_OP_EVENTS
        ? [...events.slice(-(MAX_OP_EVENTS - 1)), tagged]
        : [...events, tagged];
    }

    for (let i = 1; i <= 501; i++) pushEvent(i);
    expect(events).toHaveLength(500);
    expect(events[0]._seq).toBe(2);
    expect(events[499]._seq).toBe(501);
  });
});
