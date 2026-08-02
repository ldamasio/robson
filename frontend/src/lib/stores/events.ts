import { writable } from "svelte/store";
import type { SseEvent } from "$api/robson";

const MAX_EVENTS = 100;

export const recentEvents = writable<SseEvent[]>([]);

export function pushEvent(event: SseEvent) {
  recentEvents.update((prev) => mergeRecentEvents([event], prev));
}

export function mergeEvents(events: SseEvent[]) {
  recentEvents.update((prev) => mergeRecentEvents(prev, events));
}

export function mergeRecentEvents(
  existing: SseEvent[],
  incoming: SseEvent[],
): SseEvent[] {
  const byId = new Map<string, SseEvent>();
  for (const event of [...existing, ...incoming]) {
    if (!byId.has(event.event_id)) byId.set(event.event_id, event);
  }

  return [...byId.values()]
    .sort((a, b) => {
      const delta = Date.parse(b.occurred_at) - Date.parse(a.occurred_at);
      return Number.isFinite(delta) ? delta : 0;
    })
    .slice(0, MAX_EVENTS);
}

export function clearEvents() {
  recentEvents.set([]);
}
