import { writable } from 'svelte/store';

/** True while a mutating request is in flight anywhere in the app. The
 *  authed layout freezes the whole interactive surface (navigation included)
 *  while it is set: RBX frontend guardrail 4, no double submit. */
export const mutationPending = writable(false);

// Module state, not component state: leaving and re-entering a page while a
// request is pending must not make a fresh guard that allows a resubmission.
let active = false;

/** Wrap a mutating action so at most one runs at a time, app-wide. The flag
 *  is set synchronously before the first await, so a second trigger that
 *  lands before the UI re-renders is still refused. */
export function exclusiveMutation<A extends unknown[]>(
  action: (...args: A) => Promise<void>,
): (...args: A) => Promise<void> {
  return async (...args: A) => {
    if (active) return;
    active = true;
    mutationPending.set(true);
    try {
      await action(...args);
    } finally {
      active = false;
      mutationPending.set(false);
    }
  };
}
