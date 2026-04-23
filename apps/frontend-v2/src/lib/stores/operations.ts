import { writable } from 'svelte/store';
import type { Position } from '$api/robson';

export const activePositions = writable<Position[]>([]);
