import { writable } from 'svelte/store';
import type { MonthlyHaltStatus } from '$api/robson';

export const haltStatus = writable<MonthlyHaltStatus | null>(null);
