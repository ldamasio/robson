/** Guard a mutating action so it cannot run twice at once (RBX frontend
 *  guardrail 4: no double submit). The guard is a plain closure flag, set
 *  synchronously before the first await, so a second click that lands
 *  before the UI re-renders its disabled state is still refused.
 *  `onBusy` mirrors the flag into component state to disable the controls. */
export function singleFlight<A extends unknown[]>(
  action: (...args: A) => Promise<void>,
  onBusy: (busy: boolean) => void,
): (...args: A) => Promise<void> {
  let busy = false;
  return async (...args: A) => {
    if (busy) return;
    busy = true;
    onBusy(true);
    try {
      await action(...args);
    } finally {
      busy = false;
      onBusy(false);
    }
  };
}
