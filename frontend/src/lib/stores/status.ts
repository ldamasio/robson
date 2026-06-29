import { writable } from 'svelte/store';
import { robsonApi, type StatusResponse } from '$api/robson';

export const status = writable<StatusResponse | null>(null);

let refreshInFlight: Promise<StatusResponse | null> | null = null;

export async function refreshStatus(): Promise<StatusResponse | null> {
  if (refreshInFlight) return refreshInFlight;

  refreshInFlight = robsonApi
    .getStatus()
    .then((next) => {
      status.set(next);
      return next;
    })
    .finally(() => {
      refreshInFlight = null;
    });

  return refreshInFlight;
}
