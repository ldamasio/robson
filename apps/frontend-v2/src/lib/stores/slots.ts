import { writable } from 'svelte/store';
import type { StatusResponse } from '$api/robson';

export const status = writable<StatusResponse | null>(null);
