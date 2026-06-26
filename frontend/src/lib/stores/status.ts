import { writable } from 'svelte/store';
import { robsonApi, type StatusResponse } from '$api/robson';

export const status = writable<StatusResponse | null>(null);

let refreshInFlight: Promise<StatusResponse | null> | null = null;
let pollTimer: ReturnType<typeof setInterval> | null = null;

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

export function startStatusPolling(intervalMs = 10_000): void {
  if (pollTimer) return;

  void refreshStatus().catch(() => {});
  pollTimer = setInterval(() => {
    void refreshStatus().catch(() => {});
  }, intervalMs);
}

export function stopStatusPolling(): void {
  if (!pollTimer) return;
  clearInterval(pollTimer);
  pollTimer = null;
}
