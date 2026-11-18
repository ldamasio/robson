/** Serialize a refresh: at most one run in flight, and triggers that arrive
 *  while it runs coalesce into exactly one follow-up run after it. Runs never
 *  overlap, so results publish in order, and every run that starts also
 *  completes and publishes, so a steady stream of triggers cannot starve it. */
export function serialRefresh(run: () => Promise<void>): () => Promise<void> {
  let inFlight: Promise<void> | null = null;
  let again = false;
  return () => {
    if (inFlight) {
      again = true;
      return inFlight;
    }
    inFlight = (async () => {
      try {
        do {
          again = false;
          await run();
        } while (again);
      } finally {
        inFlight = null;
      }
    })();
    return inFlight;
  };
}
