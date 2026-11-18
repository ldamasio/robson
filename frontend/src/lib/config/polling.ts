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

// Operation details: the SSE stream is a delta feed, so the page re-reads the
// position snapshot when an event concerns this position, when the server
// asks for a resync, and after every reconnect. Refreshes are coalesced to at
// most one request per window, so an event burst cannot become a request storm.

export const OPERATION_REFRESH_MS = 1_500;

/** Whether an SSE event should trigger a snapshot re-read of `operationId`. */
export function operationNeedsRefresh(
  event: { event_type: string; payload: Record<string, unknown> },
  operationId: string,
): boolean {
  if (event.event_type === 'system.resync_required') return true;
  return operationId !== '' && event.payload?.position_id === operationId;
}
