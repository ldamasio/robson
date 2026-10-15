// Status polling policy for the dashboard.
//
// The SSE stream carries lifecycle transitions and, since the mark-advance
// events, stop and watermark moves. It does not carry prices: open P&L, the
// "market move" badge and the distance to the next rung only change when
// /status is re-read. Poll it on a slow cadence, and only while there is
// something to watch and someone watching.

export const STATUS_POLL_MS = 20_000;

/** Whether the dashboard should re-read /status on this poll tick. */
export function shouldPollStatus(
  activeCount: number,
  visibilityState: DocumentVisibilityState | string,
): boolean {
  return activeCount > 0 && visibilityState === 'visible';
}
