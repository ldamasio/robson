# Venue capability gates — ADR-0056 step 8f, parts 1–2

Status: repository implementation; operational rollout pending. This document
covers direction gating and the legacy balance-batch barrier. Account-specific
worker/funding gates remain pending part 3; balance execution, accounting and
factory activation still require their later owners. USD-M execution/validation
and the existing position-account stub keep their previous paths.

`ExchangePort: AccountPort` exposes the existing canonical contract. No side support
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


## Legacy balance batch boundary

`Executor::validate_legacy_batch` is a synchronous O(n) scan with O(1) retained
space. Only canonical `account_kind` selects this boundary. On BalanceAccount,
a batch containing PlaceEntryOrder, PlaceExitOrder, PlaceInsuranceStop,
ReplaceInsuranceStop or CancelInsuranceStop returns typed `BalanceOwnerRequired`.
UpdateTrailingStop, TriggerExit and EmitEvent remain allowed when the entire batch
contains no trading action. New EngineAction variants require explicit classification.

The manager checks the original slice before entry fragmentation or persistence;
governed dispatch checks before Acting, its audit and run_admission; direct
Executor checks before opening a journal operation, including ambient calls.
Earlier event/state actions in a refused mixed batch never run. No settings read,
venue request, journal statement, booking or domain event occurs inside that batch.
PositionAccount entry/exit continue through the existing futures-settings checks.

Full signal and approval wrappers preserve their truthful AdmissionReach and
exclude this typed error from failure rearm, including Long. They emit a bounded
`balance_owner_required` public rejection without exhaustion, retry, backoff or
detector creation. The error is neither SendRefused nor a reduce-only/rules rejection.
Earlier preparation remains separate: the legacy Long fixture makes two reads
(balance and rules) and three snapshots (Accepted, Processing, RiskChecked) for a
signal, or one rules read and one Authorized snapshot on approval resume. The
boundary adds no Acting/Failed append. Those fixtures do not certify every earlier
balance preparation path or authorize its use in production.

Close, ordinary tick failure and each Active panic leg retain their existing one
Failed query snapshot. Close/tick return the owner error after a successful audit,
or the audit error if that append fails. Panic retains log-and-continue behavior,
never lists the refused id as closed, and never changes its financial facts.

| Full caller reaching the refused batch | SQL including transaction controls/cleanup |
|---|---|
| Close / one Active panic leg | Accepted + Processing + Acting + Failed: 24 normal, at most 25 if the final COMMIT fails. |
| Ordinary tick refused trading batch | Routine snapshots are nondurable; Failed: 6 normal, at most 7. |
| N Active panic legs | 24N normal, at most 25N on final-audit failure per leg. |
| Earlier failed Accepted / Processing / Acting append in close or panic | Existing failure paths: at most 7 / 20 / 26 respectively. These do not reach the batch refusal and are not covered by the 25 bound. |

The 28-case PostgreSQL fixture observes whole callers through single-slot pool
reacquisition, including queued rollback, and configures both query and domain
writers. Final sequence/idempotency/event/stream failures cost 3/4/5/6 statements
on top of earlier snapshots; deferred COMMIT costs 7. Lost final acknowledgement
can leave the Failed audit durable. Error/cancellation retains the leg, and no
fallback reads the venue. Generated original-batch permutations and concurrent
approval schedules exercise final financial/rearm invariants. USD-M settings
refusal remains covered for both entry and exit.

No database or venue availability fallback, deadline, migration, financial policy,
factory selection or configuration is changed by this barrier. Account-specific
worker gates and the dedicated balance owner remain pending. Activation requires
the coordinator's current 15 prerequisites, including event-bus remediation and
N5 restart evidence; this implementation grants no SD12 exception or rollout.
