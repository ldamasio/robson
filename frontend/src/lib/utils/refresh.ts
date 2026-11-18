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

/** A serialized refresh bound to a user selection (for example the month on
 *  view). Each run reads for the selection current when it starts and
 *  publishes its result or error only if that selection is still current
 *  when it ends. Runs never overlap and triggers coalesce into one follow-up,
 *  so repeated refreshes of the same selection keep publishing, and a
 *  selection change (which must trigger a refresh) is read by the follow-up. */
export function selectionRefresh<S, R>(options: {
  selection: () => S;
  read: (selection: S) => Promise<R>;
  publish: (selection: S, result: R) => void;
  fail: (selection: S, error: unknown) => void;
}): () => Promise<void> {
  return serialRefresh(async () => {
    const selection = options.selection();
    try {
      const result = await options.read(selection);
      if (options.selection() === selection) options.publish(selection, result);
    } catch (error) {
      if (options.selection() === selection) options.fail(selection, error);
    }
  });
}
