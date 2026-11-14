# Venue capability gates — ADR-0056 step 8f, part 1

Status: repository implementation; operational rollout pending. This document
covers direction gating only. The legacy balance-batch barrier and account-specific
worker/funding gates are separate pending parts of 8f; balance execution, accounting
and factory activation still require their later owners. USD-M execution/validation
and the existing position-account stub keep their previous paths.

`ExchangePort: AccountPort` exposes the existing canonical contract. No capability
is inferred from a venue id, symbol, market kind or account kind. The three manual
legacy test mocks explicitly forward cash, valuation, costs and capabilities to
configured canonical stub delegates while retaining their deliberate legacy faults.

`supports_short` decides whether Short is allowed. API arm and direct arm/preview
refuse before basis reads, engine changes or detector construction. The API returns
the existing error body with HTTP400 and a typed `VenueUnsupported` cause.

A signal consumer stops its spent detector, checks incoming and authoritative sides,
and returns before query creation or generic rearm. Approval and autonomous expiry
check cached, proposed and authoritative sides before ordinary expiry/rearm. A
consumed denied approval records at most one Failed snapshot; an audit error or
cancellation cannot recreate a detector. Restore and the common rearm boundary
independently refuse before creating a child token, detector or OHLCV work.

Existing manual-approval gates retain their precedence: MonthlyHalt, unresolved
residual and the admission fence are checked before pending removal. A closed gate
leaves the pending record intact, with no venue call or audit write. The residual
check reads the existing in-memory position list; when those gates pass, capability
validation makes one authoritative in-memory lookup. USD-M ordering is unchanged.

Refusal preserves every leg, inventory, financial and admission fact. Empty Armed
fields do not establish historical no-send, and unresolved journal evidence is not
finalized. No provenance cache or capability-induced EntryAttemptExhausted event is
added. Typed errors, existing public EntryRejected (`terminal=false`), restore logs
and `robsond_venue_capability_refusals_total` expose the refusal. The metric has no
identity labels. Other entry rejection policies remain unchanged.

## Direct work and failures

| Path | SQL / venue requests attributable to the direction refusal |
|---|---|
| Arm, preview, signal, restore, common rearm | 0 / 0; signal may stop its spent detector. |
| Closed legacy approval gate | 0 / 0; pending approval remains queued. |
| Consumed approval or autonomous expiry | One Failed append: 6 SQL normally, at most 7 including rollback after failed COMMIT; 0 venue requests. |
| Repeated consumed approval | 0 / 0; QueryNotFound. |

The Failed append is BEGIN, next_seq, idempotency INSERT, event INSERT, stream UPDATE,
COMMIT. A failure at the sequence/idempotency/event/stream stages sends at most
3/4/5/6 statements including rollback. A deferred COMMIT rejection costs7; lost
COMMIT acknowledgement may leave the audit durable and returns an error without
rearming. Tests count the wire through completed SQLx rollback using a single-slot
pool, not merely the journal counter. No domain projection or stream-suspension
retry is introduced by this query-only failure audit. Existing unrelated projection
workers remain separate.

Database failure remains visible through the audit error; no durable success is
claimed. Process cancellation leaves the local pending work consumed, preserves the
leg and cannot fall through to rearm. On restart unsupported restoration refuses
again. No HTTP dependency, retry loop, new task, migration, configuration or
production balance venue variant is added. Existing database deadlines are not
changed. This local refusal does not certify startup recovery, earlier balance-Long
preparation, tick healing or the later balance owner.

Acceptance includes chart-derived test signals, mismatched sides, failed/cancelled
audits, autonomous expiry, generated approval/expiry/disarm schedules, restored
empty Armed work with unresolved J, direct/API guards and canonical mock forwarding.
SQL tests use isolated sqlx test databases and a bounded wire trace. Preserve the
existing USD-M/stub suites and factory refusal; deliberate mutants must fail runtime
assertions. Full operational profiling and activation remain separate evidence.
