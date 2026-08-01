ADR-0049: The event log is an audit trail, not a telemetry sink

Status: Accepted
Date: 2026-08-01

Context

By 2026-08-01 the production database had grown to 17 GB. Measurement showed
that the genuine trading audit trail (arm/disarm, entries, fills, stop moves,
closes, monthly resets, recalibrations) amounted to less than 1 MB. The
volume was concentrated in four places:

- `event_log_2026_04`: 10 GB — 6.5M `QUERY_STATE_CHANGED` events appended in
  five days (2026-04-22 to 2026-04-27), all on the single `robson:daemon`
  stream.
- `event_log_2026_07`: 2.8 GB — 1.7M more `QUERY_STATE_CHANGED` events over
  a month of live position monitoring.
- `event_idempotency`: 2.6 GB — 8M dedupe keys, never pruned.
- `queries_current`: 2 GB — 2M projected query rows, 95% `Completed`.

The source is structural. `ProcessMarketTick` queries are created for every
active position on every market tick, and `EventLogQueryRecorder` persisted
every lifecycle transition (`accepted`, `processing`, `risk_checked`,
`acting`, `completed`) of every one of them as a `QUERY_STATE_CHANGED`
event. Three or more events per position per tick, indefinitely. The August
partition received exactly one event (`month_boundary_reset`) while no
position was open, confirming that the growth tracks monitoring activity,
not trading.

Two facts make this telemetry, not audit:

1. The governed actions a tick query can produce (trailing stop moves,
   exits) already emit their own domain events on position streams. The
   trade audit is complete without the tick query's happy-path lifecycle.
2. The hash-chain columns (`prev_hash`, `hash`) have never been populated;
   nothing verifies event-log continuity across these rows.

Decision

1. **Routine market-tick query transitions are telemetry.**
   `EventLogQueryRecorder::record_transition` persists a
   `ProcessMarketTick` query transition only when its state is a governed
   or abnormal outcome: `Denied`, `Failed`, or `Expired`. Happy-path
   transitions stay on structured tracing (`trace_query_transition`), which
   remains emitted for every transition of every query.

2. **Every other query kind remains fully event-sourced.** Signals,
   arm/disarm, closes, panic closes, funding — unchanged. Governance
   outcomes are always audited for all kinds, including market ticks.

3. **Auxiliary tables get retention.** `event_idempotency` rows older than
   14 days and `queries_current` rows in `Completed`/`Failed` older than 30
   days are prunable. `Denied` rows are kept indefinitely (governance
   evidence). Retention runs out-of-band (rbx-infra CronJob), not in
   robsond.

4. **Deleting telemetry from the event log is permitted when archived
   first.** The 2026-08-01 cleanup archived full `pg_dump -Fc` copies of
   `event_log_2026_04`, `event_log_2026_07` and `queries_current` to
   `s3://rbx-backups/robson-db-archive/` (Contabo Object Storage, SHA-256
   manifest alongside) before deleting `QUERY_STATE_CHANGED` and
   `position_monitor_tick` rows from those partitions.

Consequences

- Steady-state event-log growth drops from roughly 3 GB/month (one active
  position) to megabytes/month. Partitions stay small enough that monthly
  `create_event_log_partitions` remains the only maintenance.
- The event log becomes what recovery and audit assume it is: a record of
  decisions and effects, replayable without wading through telemetry.
- `queries_current` no longer accumulates a row per market tick, so the
  operator-facing query views show intent-level activity.
- Observability of tick queries moves entirely to tracing/logs. If
  per-tick persistence is ever needed again (e.g. for offline analysis), it
  must go to a dedicated telemetry store, not the event log.
- `position_monitor_tick` events (~70k rows, ~20 MB per active month) are
  accepted for now as heartbeat evidence on position streams; revisit if
  they ever dominate a partition.
