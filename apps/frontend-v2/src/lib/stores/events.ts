import { writable } from 'svelte/store';
import type { SseEvent } from '$api/robson';

export const recentEvents = writable<SseEvent[]>([]);

export function pushEvent(event: SseEvent) {
  recentEvents.update((prev) => [event, ...prev].slice(0, 50));
}

export function clearEvents() {
  recentEvents.set([]);
}
