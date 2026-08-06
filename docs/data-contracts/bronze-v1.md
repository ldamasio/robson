# Data Contract: bronze-v1 (Robson · cold retention export)

**Status**: Proposed (Phase A deliverable; no export runs until Phase B is explicitly approved)
**Date**: 2026-08-06
**Owner of this contract**: Robson (source schema authority)
**Pipeline implementer/consumer**: `rbx-data` (ADR-0201)
**Governance boundary**: rbx-governance ADR-0203 (Robson analytical data boundary)
**Version**: 1.0.0

This contract defines what Robson exposes for the minimal bronze retention
pipeline, and nothing more. Additions are contract changes (v1.x additive,
v2 breaking), never implementation decisions.

## 1. Purpose and non-goals

Bronze exists for durable audit memory and forensics: the runtime deliberately
prunes its own history (ADR-0049 audit-only event log, weekly retention
CronJob, short Loki retention). Bronze is NOT:

- a protection mechanism (alerts are native Prometheus; batch never protects);
- a backup/DR mechanism (DR has its own process);
- an analytics platform (silver/marts are not authorized by this contract);
- a writeback path (nothing derived from bronze reaches the runtime, ever).

## 2. Scope v1: exactly two tables

### 2.1 `event_log`

- **Nature**: append-only, partitioned monthly by `ingested_at`.
- **Cursor**: (`ingested_at`, `event_id`), daily windows, sealed after a grace
  period; a sealed window is immutable and re-exports must be byte-identical.
- **Columns (all exported verbatim, except where noted)**: `event_id`,
  `tenant_id`, `stream_key`, `seq`, `event_type`, `payload` (see 2.3),
  `payload_schema_version`, `occurred_at`, `ingested_at`, `idempotency_key`,
  `trace_id`, `causation_id`, `command_id`, `workflow_id`, `actor_type`,
  `actor_id`, `prev_hash`, `hash` (both currently never populated; exported
  as-is for fidelity).

### 2.2 `income_ledger`

- **Nature**: idempotent ingestion keyed by `exchange_income_id` (ADR-0045).
- **Cursor**: (`income_time`, `exchange_income_id`), daily windows, sealed.
- **Columns (all exported verbatim)**: `id`, `exchange_income_id`, `symbol`,
  `income_type`, `amount`, `asset`, `exchange_trade_id`, `income_time`,
  `matched_event_id`, `matched_at`, `created_at`.

### 2.3 Field-level redaction (v1 tradeoff, reversible)

Free-text `reason` fields inside `payload` (today: `EntryOrderFailed.reason`,
`EntryExecutionRejected.reason`, and any other free-form string reason) are
replaced with the literal `"[redacted-bronze-v1]"` at export time.
Machine-readable codes (`reason_code`, `recoverable`, all numeric fields)
are preserved. Rationale: the current redaction tooling inspects field names,
not string contents; until value-level redaction exists, free text is
excluded by default. Restoring full `reason` export is a v1.x change gated on
that tooling.

Everything not listed in §2 is out of scope. In particular:
`exchange_credentials_multi_tenant` is permanently prohibited;
`balances_current` and all mutable projections are out of v1.

## 3. Storage layout (implemented by rbx-data)

```
s3://rbx-data-lake/robson/{env}/7years/bronze/v1/{table}/date=YYYY-MM-DD/part-<window>.ndjson.zst
s3://rbx-data-lake/robson/{env}/7years/bronze/v1/{table}/manifest-<run_id>.json
```

- `env` ∈ {`prod`, `testnet`}; environments never mix (separate prefixes,
  cursors, credentials; no cross-env queries).
- NDJSON, zstd; one line per source row; JSONB nested verbatim (post §2.3).
- Manifest per run: table, window, row count, SHA-256 of objects, exporter
  version, `payload_schema_version` min/max observed.
- Objects are immutable; deterministic names per sealed window; checkpoint
  advances only after the manifest is committed.

## 4. Source access constraints (hard requirements on the exporter)

- Dedicated read-only role (new; never `robsond` credentials, never the
  `rbx_data` warehouse role): `NOINHERIT`, `CONNECTION LIMIT 1`,
  `default_transaction_read_only=on`, SELECT only on §2 tables.
- `statement_timeout <= 5s`, `lock_timeout <= 1s`, idle-in-transaction
  timeout, partition-pruned window queries only, no global COUNT/SUM.
- Role provisioning must not restart PostgreSQL (pg_hba via reload, in an
  approved window).
- Runs outside the jaguar node (no toleration to the analytics taint).
- Fail-closed: any error aborts the run; no partial manifests, no in-memory
  fallback.

## 5. Data classification

`sensitive/financial`. Access via a dedicated S3 credential restricted to the
`robson/` prefix (never the backup credential), deny delete/overwrite,
versioning enabled. No public artifacts. Consumers are registered in the
rbx-data catalog with owner and retention class (`7years`).

## 6. Quality invariants (checked per run, from manifests + sampled windows)

1. Row-count parity: source window count = bronze line count.
2. Uniqueness: `event_id` and `exchange_income_id` unique across bronze.
3. Coverage: every daily window between first cursor and now has a manifest
   or an explicit empty-window record; holes alert.
4. Idempotency: re-export of a sealed window is byte-identical (hash match).

## 7. Future contract items (explicitly NOT in v1; one gated PR each)

These unblock metrics that are only approximate today. They are listed here
so the need is on record; each requires its own Robson PR and review:

- **(a) `planned_worst_case_loss_quote` persisted at arm** (+ formula version
  and cost inputs): makes planned-vs-realized risk exact. Today
  `PositionArmed` does not persist it and `ExecutableStopPlan` is transient.
- **(b) `transport` (`ws` | `rest`) on `TrailingStopUpdated`**: enables
  trailing-quality comparison under REST fallback (ADR-0044).
- **(c) enumerated `reason_code` on `EntryExecutionRejected`**: replaces
  fragile free-text normalization (parity with `EntryAttemptExhausted`).
- **(d) `last_successful_poll` signal for the income poller**: poller health
  is currently unobservable (`MAX(income_time)` measures economic activity,
  not health).

## 8. Versioning and change process

- Additive changes (new exported column, restored `reason` after value-level
  redaction): bump minor (1.x), announce in this file's changelog.
- Breaking changes (column removal/rename, cursor semantics): new
  `bronze/v2/` prefix; `v1` frozen, never rewritten.
- `payload_schema_version` changes inside events do not break this contract;
  bronze carries them verbatim and manifests record the observed range.

## Changelog

- 1.0.0 (2026-08-06): initial contract. Scope: `event_log`, `income_ledger`.
