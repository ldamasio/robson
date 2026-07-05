import { writable } from 'svelte/store';

export type SseFreshness = {
  fresh: boolean;
  staleSeconds: number;
};

const AGE_FRESH_SECS = 45;

function createSseFreshnessStore() {
  const { subscribe, set } = writable<SseFreshness>({
    fresh: false,
    staleSeconds: 0,
  });

  let lastEventAt: number | null = null;
  let stale = false;
  let tickTimer: ReturnType<typeof setInterval> | null = null;

  function tick() {
    const staleSeconds =
      lastEventAt == null ? 0 : Math.floor((Date.now() - lastEventAt) / 1_000);
    const fresh = !stale && lastEventAt != null && staleSeconds < AGE_FRESH_SECS;
    set({ fresh, staleSeconds });
  }

  function start() {
    stop();
    tickTimer = setInterval(tick, 1_000);
    tick();
  }

  function stop() {
    if (tickTimer) {
      clearInterval(tickTimer);
      tickTimer = null;
    }
  }

  function markEvent() {
    stale = false;
    lastEventAt = Date.now();
    tick();
  }

  function markConnected() {
    stale = false;
    if (lastEventAt == null) lastEventAt = Date.now();
    tick();
  }

  function markStale() {
    stale = true;
    tick();
  }

  return {
    subscribe,
    start,
    stop,
    markEvent,
    markConnected,
    markStale,
  };
}

export const sseFreshness = createSseFreshnessStore();
export const markSseEvent = sseFreshness.markEvent;
export const markSseConnected = sseFreshness.markConnected;
export const markSseStale = sseFreshness.markStale;
