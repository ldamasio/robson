# Request-budget facts (ADR-0056 step8h1b-1)

`robson_exec::request_budget` calculates facts from unqualified supplied data.
It is unused by runtime callers. It creates no reservation, grant, durable
receipt, readiness state, financial clock evidence or cost-age authority.

Local usage counts every supplied Pending charge forever until the caller
changes the supplied set; it does not infer cancellation from age. Sent weight
counts while elapsed from supplied handoff is <60s; short orders while <10s.
Full boot/namespace domain equality and checked elapsed arithmetic come from
`venue_clock::TickCapture`; different domains and reversed time refuse.
The module never initializes or calls `NativeClock`.

The input slice is limited to1200 entries before evaluation. Every cost has
weight1..=1200 and0/1 order units. This validates numerical range, not a real
endpoint. All supplied entries count, including duplicates. Omitted entries,
empty input or a scalar daily count establish no source completeness.

Prospective facts compare total local usage with400/600 normal steady/protective
weight,10 short orders and10000 daily orders. Protective expanded arithmetic
uses1200/20 while keeping the same10000 daily ceiling. An expanded fit supplies
no emergency permission. Startup caps restrict both normal and expanded fits;
headers, pauses, identity and strict journal authorization remain additional
requirements for any future caller.

During the first60/10s, supplied epoch counts give floor(200/2^(k60-1)) and
floor(2/2^(k10-1)); saturation is9/3 epochs. Unknown history gives zero in the
corresponding initial window. At its boundary that additional cap ends; this
does not end the unclean120s barrier, daily quarantine or restored pauses.

Attempt-age candidates use <=60s. Daily-retention and quarantine candidates
use >=24h+60s. They never retire rows or confirm an original operation. A
quarantine anchor must match supplied non-nil cohort/recovery identities;
those matching values still prove neither provenance nor continuity. No
function replaces a missing anchor with now. N5 strict age upper<24h and its
confirmed-handoff/claim spacing remain separate later consumers.

Successful result fields are private; external compile-fail controls cover
unchecked usage and temporal-result construction. Inputs remain untrusted,
and cloning a result does not confer authority. The initial public temporal
enum draft was replaced before acceptance to honor this output boundary.

All paths perform0 SQL/internal/RI/HTTP/native/file calls, awaits, locks,
timers/tasks and writes. The usage fold visits at most1200 borrowed records,
with at most2400 checked additions and1200 elapsed comparisons. Other calls
have fixed arithmetic/comparison work. There is no collection allocation,
retained reference or mutable cache/model. This is not a wall-time guarantee.

Tests use an independent integer oracle, boundary/progress/error cases and a
heap-allocated maximum input under1MiB runtime stack. Runtime mutation failures
and API-visibility mutants that unexpectedly compile are reported separately.
No migration, dependency, USD-M or factory behavior changes.

Tradeoff: these facts can be tested independently, but cannot operate a limiter.
Daily union/projection completeness, positive durable receipts, reservation
ownership, emergency release, pause/first-poll races and runtime Drop remain
later work. The private state-model plan is not implemented here. Full35/25/200
composition, all15 activation prerequisites and SD15/SD16/T6/8o remain separate.
