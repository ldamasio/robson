import { writable } from 'svelte/store';
import type { SseEvent } from '$api/robson';

export const recentEvents = writable<SseEvent[]>([]);
