/** Track overlapping requests so only the newest one may publish its result.
 *  `begin()` starts a request and returns `isCurrent`, which stays true only
 *  until a newer request begins: a slow older response that arrives after a
 *  newer one is dropped instead of restoring stale state. */
export function latestOnly(): { begin: () => () => boolean } {
  let generation = 0;
  return {
    begin() {
      const mine = ++generation;
      return () => mine === generation;
    },
  };
}
