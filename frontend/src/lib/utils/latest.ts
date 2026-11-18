/** Track overlapping requests for a user selection so only the newest may
 *  publish its result or error: `begin()` returns `isCurrent`, which stays
 *  true only until a newer request begins. Meant for selection-driven reads
 *  (for example the month view), where only the latest choice matters. */
export function latestOnly(): { begin: () => () => boolean } {
  let generation = 0;
  return {
    begin() {
      const mine = ++generation;
      return () => mine === generation;
    },
  };
}
