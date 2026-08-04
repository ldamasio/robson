# Preflight evidence: `span_capped_v1` durable-history check (ADR-0052)

**Date/time (UTC)**: 2026-08-04, 16:40 to 16:57
**Operator context**: precondition for ADR-0052 Decision 3 (outright deletion
of the never-persisted `span_capped_v1` stop-policy variant).
**Method**: ephemeral `postgres:16` pods (`kubectl run --rm`) inside each
namespace, `DATABASE_URL` injected from the environment's robsond secret via
`secretKeyRef` (credential value never displayed). Queries and raw results
below verbatim; counts are the tool output.

## Production (`robson` namespace, external ParadeDB)

| Check | Query | Result |
| --- | --- | --- |
| Projection stamps | `SELECT stop_policy, count(*) FROM positions_current GROUP BY 1;` | `legacy_uncapped\|57` (no other value) |
| Event-log payloads | `SELECT count(*) FROM event_log WHERE payload::text LIKE '%span_capped_v1%';` | `0` |
| Snapshot tables (enumerated via `pg_tables LIKE '%snap%'`: `snapshots`, `snapshots_2026_05`, `snapshots_2026_07`, `snapshots_2026_08`, `snapshots_2026_09`) | `SELECT count(*) FROM <t> WHERE <t>::text LIKE '%span_capped_v1%';` per table | `0` for every table |

## Testnet (`robson-testnet` namespace)

| Check | Query | Result |
| --- | --- | --- |
| Projection stamps | `SELECT stop_policy, count(*) FROM positions_current GROUP BY 1;` | ERROR: column does not exist (schema predates migration 000024) |
| Event-log payloads | `SELECT count(*) FROM event_log WHERE payload::text LIKE '%span_capped_v1%';` | `0` |

## Backups

Database backups are point-in-time copies of the schemas above; since no
durable row has ever contained the value, no backup can contain it either.
The archived pre-cleanup database copy
(`s3://rbx-backups/robson-db-archive/`, taken 2026-08-01) predates the
2026-08-03 deploy that introduced the `span_capped_v1` string and therefore
cannot contain it.

## Conclusion

Zero occurrences of `span_capped_v1` in any durable store: production
projection, production event log, all production snapshot partitions,
testnet event log. The testnet projection lacks the column entirely (image
predates ADR-0050 slice 5), which is consistent with its event log
containing zero stamps. Deletion precondition SATISFIED as of
2026-08-04T16:55:38Z. Any re-run before implementation merge must reproduce
zero results.
