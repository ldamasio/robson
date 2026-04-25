import { writable } from 'svelte/store';
import type { SseEvent } from '$api/robson';

const MAX_EVENTS = 100;

export const recentEvents = writable<SseEvent[]>([]);

export function pushEvent(event: SseEvent) {
  recentEvents.update((prev) => [event, ...prev].slice(0, MAX_EVENTS));
}

export function clearEvents() {
  recentEvents.set([]);
}
