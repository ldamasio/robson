import { writable } from 'svelte/store';
import { robsonApi, type StatusResponse } from '$api/robson';

export const status = writable<StatusResponse | null>(null);

let inFlight: Promise<StatusResponse | null> | null = null;
let again = false;

/** Re-read /status into the shared store. Reads never overlap, so results
 *  publish in order. A call made while a read is running cannot be answered
 *  by that read (it may have been sent before the caller's trigger, such as
 *  an SSE subscription opening): it queues one follow-up read, and every
 *  caller of that in-flight attempt resolves only after the follow-up. */
export function refreshStatus(): Promise<StatusResponse | null> {
  if (inFlight) {
    again = true;
    return inFlight;
  }
  inFlight = (async () => {
    try {
      let latest: StatusResponse | null = null;
      let failure: unknown = null;
      do {
        again = false;
        try {
          latest = await robsonApi.getStatus();
          status.set(latest);
          failure = null;
        } catch (error) {
          failure = error;
        }
      } while (again);
      if (failure) throw failure;
      return latest;
    } finally {
      inFlight = null;
    }
  })();
  return inFlight;
}
