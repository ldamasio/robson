# Data Contract: bronze-v1 (Robson · cold retention export)

**Status**: Proposed (Phase A deliverable; no export runs until Phase B is explicitly approved)
**Date**: 2026-08-07 (v1.1.1; original 2026-08-06)
**Owner of this contract**: Robson (source schema authority)
**Pipeline implementer/consumer**: `rbx-data` (ADR-0201)
**Governance boundary**: rbx-governance ADR-0203 (Robson analytical data boundary)
**Version**: 1.1.1
**Amendment basis**: pre-Phase-B adversarial studies (2026-08-06: payload
field audit, window-sealing attack) plus the adversarial review of this
amendment itself (2026-08-07), whose required changes are incorporated.

This contract defines what Robson exposes for the minimal bronze retention
pipeline, and nothing more. Additions are contract changes (v1.x additive,
v2 breaking), never implementation decisions.

## 1. Purpose and non-goals

Bronze exists for durable audit memory and forensics. The runtime keeps its
audit tables lean and discards operational telemetry (ADR-0049 audit-only
event log, short Loki retention). Today's weekly retention CronJob prunes
auxiliary tables only and touches neither table in scope; §6 makes a
retention interlock a Phase B prerequisite so that stays true. Bronze is NOT:

- a protection mechanism (alerts are native Prometheus; batch never protects);
- a backup/DR mechanism (DR has its own process);
- an analytics platform (silver/marts are not authorized by this contract);
- a writeback path (nothing derived from bronze reaches the runtime, ever).

## 2. Scope v1: exactly two tables

### 2.1 `event_log`

- **Nature**: append-only, partitioned monthly by `ingested_at` (UTC month
  boundaries, tz-hardened partition functions per migration 0016).
- **Windows**: daily, **UTC half-open** `[00:00:00Z, next day 00:00:00Z)`,
  compared as `TIMESTAMPTZ` bounds (never `::date` casts, never
  session-timezone arithmetic).
- **Cursor and canonical row order**: (`ingested_at`, `event_id`) ascending.
- **Sealing**: per the §3.2 fence-then-snapshot protocol. `ingested_at` is
  `NOW()` = transaction start, so a long-lived writer transaction can make a
  row visible inside an already elapsed window; a fixed grace period is not
  sufficient. Sealed windows are immutable; re-export follows §6.4.
- **Columns (all exported, subject to §2.3 payload redaction)**: `event_id`,
  `tenant_id`, `stream_key`, `seq`, `event_type`, `payload`,
  `payload_schema_version`, `occurred_at`, `ingested_at`, `idempotency_key`,
  `trace_id`, `causation_id`, `command_id`, `workflow_id`, `actor_type`,
  `actor_id`, `prev_hash`, `hash` (both currently never populated; exported
  as-is for fidelity).
- **Normative event registry (fail-closed)**: `event_log` contains rows
  beyond the `robson-domain::Event` enum. The registry is a
  version-controlled artifact owned by this contract
  (`docs/data-contracts/bronze-event-registry.yaml`, versioned alongside
  this contract). It enumerates every accepted `event_type`,
  including: all 31 `robson-domain::Event` variants; current auxiliary types
  such as `QUERY_STATE_CHANGED` (uppercase) and the PascalCase funding-saga
  events (`FundingQuoted`, `FundingFailed`, ...); legacy families; allowed
  historical shapes per type (`payload_schema_version` is frozen at `1`, so
  optional-field presence must be modeled explicitly); and **value
  allowlists or grammars for the machine-code exceptions** of §2.3
  (type-only validation is prohibited for `exit_triggered:$.reason`,
  `capital_base_recalibrated:$.reason`, `skipped_levels[*].reason` and
  `entry_attempt_exhausted:$.reason_code`, because a `String` field passes
  structural checks even when a producer starts writing prose into it).
  Phase B remains blocked until both a code-derived producer inventory and a
  source `SELECT DISTINCT event_type` inventory are covered by the registry.
  Any unregistered type, unregistered shape, field-type mismatch,
  discriminator mismatch, or exception-value violation aborts the run with a
  schema-drift error. Unknown data is never exported and never silently
  dropped.
- **Raw JSONB only**: the exporter reads and re-emits the stored JSONB
  value. It must NOT deserialize into the Rust `Event` type and reserialize:
  that can materialize defaulted fields that were never persisted (e.g.
  historical `PositionClosed` rows without `closure_evidence`), change
  optional-field presence, or reject auxiliary/legacy payloads entirely.

### 2.2 `income_ledger` (ingestion facts only)

- **Nature**: idempotent ingestion keyed by `exchange_income_id` (ADR-0045).
  The table is **mutable after ingestion**: `matched_event_id`/`matched_at`
  are set by later reconciliation; `acked_at`, `ack_reason`, `acked_by` are
  set by operator acknowledgement; `alarmed_at` is set by the anomaly
  worker. None has a bounded update time, so v1 exports only the immutable
  ingestion facts.
- **Exported columns**: `id`, `exchange_income_id`, `symbol`, `income_type`,
  `amount`, `asset`, `exchange_trade_id`, `income_time`, `created_at`.
- **Explicitly excluded**: `matched_event_id`, `matched_at`, `acked_at`,
  `ack_reason` (free text), `acked_by`, `alarmed_at`. Match/ack/alarm state
  may later ship as an append-only revision stream under a separate contract
  item; it never mutates sealed bronze objects. `SELECT *` is therefore a
  contract violation by construction.
- **Windows, cursor and canonical row order**: daily UTC half-open windows
  over **`created_at`** (ingestion time); cursor and row order
  (`created_at`, `exchange_income_id COLLATE "C"`) ascending, bytewise.
  `income_time` is business time: it can land days late after poller outages
  (poller cursor is `MAX(income_time)`, first-load backfill capped at 24h),
  so no finite grace proves completeness by `income_time`. Sealing by
  ingestion time restores the same fence semantics as `event_log`.

### 2.3 Field-level redaction (exact matrix, from the payload audit)

Redaction replaces a present string with the literal `"[redacted-bronze-v1]"`
(`null` stays `null`; array elements are replaced one-by-one preserving
cardinality). For the 31 domain-enum event types the redaction set is
**exactly** the following 12 paths; auxiliary producers add their own paths
in the normative registry (`bronze-event-registry.yaml`, currently 6 more
under `QUERY_STATE_CHANGED`, total 18), which is the single artifact the
exporter loads. A generic by-field-name rule is wrong in both directions:

| `event_type` | JSONPath |
|---|---|
| `signal_strategy_evaluated` | `$.reason` |
| `technical_stop_analyzed` | `$.analysis.stop_anchor.invalidation_reason` |
| `technical_stop_analyzed` | `$.analysis.stop_quality.reasons[*]` |
| `entry_attempt_exhausted` | `$.reason` |
| `entry_order_failed` | `$.reason` |
| `entry_execution_rejected` | `$.reason` |
| `entry_fill_protection_fallback` | `$.live_resolution_error` |
| `executable_stop_risk_resolution_failed` | `$.reason` |
| `capital_base_recalibrated` | `$.evidence` |
| `position_disarmed` | `$.reason` |
| `position_error` | `$.error` |
| `insurance_stop_failed` | `$.error` |

Mandatory exceptions (machine codes, exported verbatim, guarded by the
registry value allowlists of §2.1): `exit_triggered:$.reason` (`ExitReason`
enum), `capital_base_recalibrated:$.reason` (documented machine-code
grammar, e.g. `income_ledger:transfer_confirmed:sum=<value>`),
`technical_stop_analyzed:$.analysis.skipped_levels[*].reason` (closed
vocabulary), and `entry_attempt_exhausted:$.reason_code` (canonical code
that must survive the redaction of the human detail). All other fields
(financial values, identifiers, enum codes, timestamps) are exported
verbatim; the dataset as a whole remains `sensitive/financial` (§5).
Restoring full free-text export is a future minor bump gated on value-level
redaction tooling.

Everything not listed in §2 is out of scope. In particular:
`exchange_credentials_multi_tenant` is permanently prohibited;
`balances_current` and all mutable projections are out of v1.

## 3. Storage and publish protocol (implemented by rbx-data)

### 3.1 Layout

```
s3://rbx-data-lake/robson/{env}/7years/bronze/v1/{table}/date=YYYY-MM-DD/part-<index>.ndjson.zst
s3://rbx-data-lake/robson/{env}/7years/bronze/v1/{table}/date=YYYY-MM-DD/commit.json
```

- `env` ∈ {`prod`, `testnet`}; environments never mix (separate prefixes,
  cursors, credentials; no cross-env queries).
- **Deterministic commit marker**: exactly one `commit.json` per table/window
  is the unit of publication. The marker **contains no per-run value**;
  `run_id` exists only in exporter logs. `commit.json` is canonical and
  deterministic for identical table/window content, so idempotent re-runs
  produce byte-identical markers. It is written only after every listed part
  has been uploaded and checksum-verified (fetched back or verified via a
  conformance-tested checksum mechanism). Each part entry records both the
  uncompressed NDJSON SHA-256 and the stored-object SHA-256 and size.
  `payload_schema_version_min/max` are always present for `event_log`
  markers (numeric for non-empty windows, both `null` for empty windows) and
  always absent for `income_ledger` markers. Empty windows publish a marker
  with `rows: 0` and no parts.
- **Conditional creation required**: every part and the marker are written
  with `If-None-Match: *`. On `412`, `409`, or an ambiguous timeout, the
  exporter retrieves and hashes the existing object (a HEAD checksum may be
  used only if the conformance test proves it is a full-object SHA-256):
  equal content = success; different content = fatal, no checkpoint advance.
  A missing object (`404` after an ambiguous outcome) is retried with
  conditional PUT. Checksum semantics are two distinct cases: a **missing
  native checksum** (HEAD returns none, or an ETag that is not a full-object
  SHA-256) is acceptable and handled by GET plus local hashing; an
  **inability to retrieve and verify the stored bytes at all** fails the
  conformance gate. **Phase B is blocked until a live conformance test on
  Contabo Object Storage** (Ceph-based, partially S3-compatible) **proves
  conditional-PUT semantics, byte-retrieval verification, and the bucket
  policy** (deny delete/overwrite, versioning).
- Checkpoint (exporter-side, never in the Robson DB) advances only after the
  commit marker is verified readable.

### 3.2 Sealing: fence, then snapshot (normative order)

- The eligibility fence MUST run as one autocommit statement on the writable
  primary (`pg_is_in_recovery() = false`), **before** any export snapshot is
  acquired. It obtains `clock_timestamp()` and fails closed if any
  non-exporter transaction in the source database has
  `xact_start < window_end`. Only after the fence returns clear may the
  exporter execute `BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY`; the
  count and every keyset page MUST use that single snapshot. Running the
  fence after that snapshot has been acquired is prohibited.
- The exporter invokes a `SECURITY DEFINER` fence function; direct
  `pg_read_all_stats` membership for the export role is not required. The
  function is **created by a Robson migration** (gated PR, Phase B
  prerequisite; the database schema is Robson's, `rbx-data` only invokes)
  and MUST be: schema-qualified; owned by a role with the needed stats
  visibility; defined with `SET search_path = pg_catalog, pg_temp`;
  `REVOKE EXECUTE ... FROM PUBLIC`; `GRANT EXECUTE` only to the export role.
  Masked statistics, permission errors, prepared transactions, a replica
  connection, or a database clock below the last sealed `window_end` leave
  the window open and alert (fail closed, never fail open).
- **Post-export verification (second fence)**: after the snapshot
  transaction ends and before the marker PUT, the exporter re-runs the fence
  in autocommit and re-counts the window in a fresh statement. Publication
  requires: fence clear again; `clock_timestamp()` monotonically greater
  than the first fence reading; and the re-count equal to the exported row
  count. Any mismatch aborts without publishing.
- **Clock discipline and residual risk**: the database host clock MUST be
  slew-only (no backward steps) as a deployment invariant, and the exporter
  persists the highest database clock it has observed, halting all sealing
  on any regression. The residual window (a backward-stepping clock plus a
  backdated writer committing entirely between the second fence and the
  marker PUT) is accepted as detectable-not-preventable: while source rows
  are retained, the coverage audit re-compares sealed windows against the
  source, and any late-appearing in-window row raises a fatal integrity
  alarm for operator decision. Full prevention requires LSN/CDC-based
  sealing, explicitly out of v1 scope.
- Two-phase commit MUST be disabled on the source, or `pg_prepared_xacts`
  MUST be included in the fence (prepared transactions are invisible in
  `pg_stat_activity`).
- Writer roles MUST NOT supply or update `event_log.ingested_at` or
  `income_ledger.created_at`; both stay database-default (`NOW()`). This is
  a contract invariant, not an observation.
- Window bounds and eligibility come from the database clock only; exporter
  host clock skew must not affect sealing.

### 3.3 Canonical byte profile `c1` (normative spec required)

"Byte-identical" is defined by profile `c1`, specified normatively in
`docs/data-contracts/bronze-c1.md` with committed golden vectors (Phase B
prerequisite; to be created before any exporter emits `profile: c1`). The
specification, not an exporter lockfile, fixes at minimum:

- JSON encodings for every SQL scalar (UUID, `TIMESTAMPTZ`, `NUMERIC`,
  `BIGINT`, `NULL`), arbitrary-precision JSONB number encoding, and string
  escaping;
- recursive bytewise key sorting inside `payload`, and the explicit
  top-level-row exception: the row object follows the §2 column order;
- canonical row order (the §2 cursors) and deterministic part boundaries,
  sizes and names;
- hash scope (uncompressed NDJSON and stored object, per §3.1);
- the canonical encoding of `commit.json` itself;
- every zstd frame parameter (level, checksum flag, single-frame, no
  dictionary, window log) and the pinned encoder version.

Redaction (§2.3) applies before hashing. Any change to the profile is a new
profile id and a new storage prefix, never an in-place change. No exporter
may emit `profile: c1` until the spec and golden vectors exist and pass
cross-run golden tests.

## 4. Source access constraints (hard requirements on the exporter)

- Dedicated read-only role (new; never `robsond` credentials, never the
  `rbx_data` warehouse role): `NOINHERIT`, `CONNECTION LIMIT 1`,
  `default_transaction_read_only=on`, SELECT only on §2 tables, plus
  EXECUTE on the §3.2 fence function (itself a Robson migration
  prerequisite).
- `statement_timeout <= 5s`, `lock_timeout <= 1s`, idle-in-transaction
  timeout, partition-pruned window queries only, no global COUNT/SUM.
- **Index prerequisite (BTREE, gated Robson migration PR)**:
  `event_log (ingested_at ASC, event_id ASC)` and
  `income_ledger (created_at ASC, exchange_income_id COLLATE "C" ASC)`.
  These are the cursor indexes for keyset pagination; BRIN may be added as a
  separate range accelerator but MUST NOT replace either cursor index.
- Role provisioning must not restart PostgreSQL (pg_hba via reload, in an
  approved window).
- Runs outside the jaguar node (no toleration to the analytics taint) unless
  the provisioning runbook's HBA-origin analysis concludes otherwise; the
  observed client address is verified empirically during the window.
- Fail-closed: any error aborts the run; no partial markers, no in-memory
  fallback.

## 5. Data classification

`sensitive/financial`. Access via a dedicated S3 credential restricted to the
`robson/` prefix (never the backup credential), deny delete/overwrite,
versioning enabled. No public artifacts. Consumers are registered in the
rbx-data catalog with owner and retention class (`7years`).

## 6. Quality invariants (checked per run; property-tested in the exporter)

1. Row-count parity: snapshot window count = bronze line count.
2. Uniqueness: `exchange_income_id` unique across bronze; `event_id`
   duplication is **detected and fatal** (the source PK is
   `(event_id, ingested_at)`, which does not guarantee global uniqueness).
3. Coverage: every daily window from the first cursor **through the latest
   completed window whose sealing preconditions passed** (never through the
   current database time) has exactly one commit marker (possibly
   `rows: 0`); holes alert; `prod` and `testnet` never mix.
4. Idempotency: source re-export of a sealed window reproduces the same
   hashes **while source rows remain retained**; after source pruning,
   verification uses the committed bronze objects and marker.

The exporter implementation must ship property tests covering at minimum:
UTC window partitioning (DST/leap/month boundaries), database-clock-only
sealing, the fence-then-snapshot ordering, the late-writer fence,
late-income coverage, cursor totality under ties and crashes, bytewise
collation independence, single-snapshot consistency, structural schema drift
and registry fail-closed (including exception-value allowlists), exclusion
of post-ingestion mutations, canonical JSON stability under key permutation,
redaction exactness, zstd golden frames, same/conflicting-content S3 races,
ambiguous PUT convergence, publish state machine (checkpoint implies
verified marker implies verified parts), crash-point resumption, coverage
completeness, and global `event_id` uniqueness detection.

**Retention interlock (Phase B prerequisite, not current implementation)**:
before any covered source row of `event_log` or `income_ledger` may ever be
pruned, the retention job MUST fetch and validate the commit marker and
every referenced part. Today's weekly CronJob touches neither table.
ADR-0049 carries the matching amendment (2026-08-07): this rule supersedes
its generic "archived first" allowance for these two tables.

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
- **(e) income match/ack/alarm revision stream**: append-only export of the
  state changes excluded from v1 by §2.2.

## 8. Versioning and change process

- Additive changes (new exported column, new registry entry, restored free
  text after value-level redaction): bump minor (1.x), announce in this
  file's changelog.
- Breaking changes (column removal/rename, cursor semantics, canonical
  profile): new storage prefix; existing prefixes frozen, never rewritten.
- `payload_schema_version` changes inside events do not break this contract;
  bronze carries them verbatim and commit markers record the observed range.

## 9. Failure modes (external dependencies; exporter behavior is normative)

| Failure | Exporter behavior |
|---|---|
| Source PostgreSQL unavailable | Run aborts; no marker, no checkpoint advance; retry next schedule |
| Connected to a replica (`pg_is_in_recovery() = true`) | Fence fails closed; window stays open; alert |
| Fence statistics masked / permission error | Window stays open; alert (never assume clear) |
| Prepared transaction present (2PC not disabled) | Window stays open; alert |
| Database clock below last sealed `window_end` | Sealing halts for all windows; alert (clock regression) |
| Exporter dies between parts and marker | No marker exists; next run re-derives parts idempotently (§3.1 conditional PUT converges) |
| Exporter dies between marker and checkpoint | Next run finds the verified marker and advances the checkpoint without re-export |
| Checkpoint store lost or corrupted | Rebuild from the last verified marker in S3 (markers are the source of truth for progress) |
| Ambiguous PUT (timeout after possible accept), `412`, `409` | Converge by fetching and hashing the existing object; mismatch is fatal |
| No native HEAD checksum on stored objects | Acceptable: verify via GET plus local SHA-256 |
| Stored bytes cannot be retrieved and verified at all, or conditional PUT / bucket policy unsupported | Phase B conformance gate fails; canary does not start |
| Clock regression detected at any fence or via the persisted high-water clock | All sealing halts; alert |
| Late in-window row found by the coverage audit on a sealed window | Fatal integrity alarm; operator decision (marker is never rewritten) |
| Object administratively deleted from the bucket | Coverage check fails (marker references missing part); alert; no silent re-publish over a sealed window |

## Changelog

- 1.1.1 (2026-08-07): part objects named part-<index> (five-digit ascending
  index per bronze-c1 §5); redaction matrix scoped to the domain enum (12 paths)
  with auxiliary-producer paths carried by the normative event registry
  (total 18 including QUERY_STATE_CHANGED); registry file named as the
  single redaction artifact the exporter loads.
- 1.1.0 (2026-08-07): incorporate pre-Phase-B adversarial findings and the
  adversarial review of this amendment. Exact 12-path redaction matrix with
  machine-code exceptions guarded by registry value allowlists; normative
  fail-closed event registry artifact (covering uppercase and PascalCase
  auxiliary producers); raw-JSONB rule; normative fence-then-snapshot
  sealing order (autocommit fence on the primary, SECURITY DEFINER fence
  function, 2PC handling, writer-default invariant, clock-regression halt,
  REPEATABLE READ READ ONLY single snapshot); `income_ledger` reduced to
  immutable ingestion facts on a `created_at` cursor; deterministic
  run-id-free commit marker with dual hashes and mandatory conditional
  creation; Contabo conformance gate extended to checksums and bucket
  policy; canonical profile `c1` delegated to a normative spec with golden
  vectors; BTREE cursor indexes required (BRIN only as accelerator);
  coverage bounded by sealable windows; re-export idempotency scoped to
  retained source rows; retention interlock as Phase B prerequisite with an
  ADR-0049 amendment requirement; failure-mode table added. Second
  verification pass fixes: empty-window marker schema-version rule made
  representable; post-export second fence with count re-verification,
  slew-only clock invariant, persisted clock high-water mark and a declared
  detectable-not-preventable residual (LSN/CDC named as the v2 path);
  SECURITY DEFINER fence given a concrete security and ownership contract
  (Robson migration, search_path pin, PUBLIC revoke); missing-native-
  checksum vs unverifiable-bytes disambiguated.
- 1.0.0 (2026-08-06): initial contract. Scope: `event_log`, `income_ledger`.
