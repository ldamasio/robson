# ADR-0056 8g2a: private exact arithmetic

Repository-verified scope: private `balance_sizing::arithmetic`, tests and this
note. No sizing/capacity DTO, geometry resolver, qualified evidence, admission
result, runtime caller, HTTP, SQL or new dependency exists here. 8g2b/c and8i
remain pending. Shared USD-M arithmetic is unchanged. The approved decomposition
is the external `8g2-sizing-plan.md` P2; this slice does not decide trading policy.

The rational contains nonnegative integer numerator and positive denominator.
Decimal input has coefficient<=96 bits and scale<=28 (denominator<=94 bits).
Negative values and zero divisors return typed errors. Negative zero is zero.
There is no string/f64 ingress or rounded Decimal operation. Denominators need
not be normalized: e.g. an intermediate exceeding native96bits can be divided
back into range before exact export. Native export refuses nonterminating,
scale>28 or coefficient>96bit results; it never rounds them.

## Primitive proof table

Let n(x),d(x) be numerator/denominator bit lengths; zero has length0. Each
Rational field is<=4096bits. All allocations of products/sums are preceded by
the bounds below. `new` is private; its already-created inputs originate only
from these checks, bounded Decimal construction, or tests.

| Primitive | Before allocation / result bound | Frame calls | Internal divisions |
| --- | --- | --- | --- |
| Decimal import | coefficient96,10^scale<=94; exponent<=28 |0|0|
| add/sub | cross-products n(a)+d(b),n(b)+d(a)<=4096; add max+1<=4096; sub requires left>=right; denominator d(a)+d(b)<=4096 |1|0|
| multiply/divide | respective numerator/denominator factor length sums<=4096; divisor numerator>0 |1|0|
| compare | both cross-product sums<=4096 |1|0|
| floor12/floor8 | numerator bits+40/27<=4096, fixed10^12/10^8; quotient<=scaled numerator |1|1|
| floor/ceil step | ratio cross-products<=4096, step>0; quotient q<=ratio numerator; increment preflight max(bits(q),1)+1<=4096; final q*step numerator length sum<=4096 |1|1|
| exact export | input clone<=4096bits; each next-scale multiply by10 preflights current bits+4<=4096; scale0..28 only; coefficient checked<=96 before <=3-word export |1|<=29|

A zero product allocates only zero. Product bit-sum and sum max+1 checks are
conservative near the technical ceiling: a WorkLimit error is NOT a financial
refusal or policy cap. The later fixed sizing expression graph must prove its
entire accepted Decimal domain fits these primitive guards and128 calls; it
must not map WorkLimit into an ordinary insufficient-capital denial. No such
caller exists yet. The full formula graph/table is an8g2b review gate; this
primitive table alone does not prove all future formulas fit.

One Arithmetic frame allows128 charged calls, including failed calls, and then
stays exhausted. Nested internal work is counted separately: at most128*29=
3712 quotient/remainder operations if every call exports, and at most128*28=
3584 internal export scale multiplications. Other primitives perform at most
three guarded products and one guarded sum/subtraction each. Constructors have
fixed input widths; no caller-controlled exponent or public expression evaluator
exists. This is bounded computational work, not a wall-clock deadline.

`quotient_remainder(n,d)` requires d>0 and <=4096bit operands. Integer division
returns q and reconstructs r=n−q*d. Although reconstruction uses a multiplication
without the generic product helper, q*d<=n proves its result<=4096bits BEFORE
that allocation; q.bits+d.bits<=4097 and raw product storage is also bounded.
No intermediate Rational crosses4096bits. All error paths preserve private
invariants. No assertion or unwrap is used to enforce production input validity.

## Pinned library internal work and memory

Inspected num-bigint0.4.8 sources `biguint/division.rs` (div_rem_cow/core) and
`multiplication.rs` (mac3). On the tested64-bit GNU target, BigDigit is64bits:
4096bits means<=64 digits; division normalization adds at most one digit. The
Burnikel-Ziegler branch requires dividend>128 digits AND divisor>64, so it is
unreachable here. Knuth division uses<=65 quotient positions, at most two
quotient-estimate decrements and one add-back at each position; digit loops have
<=65 elements. Single-digit division traverses<=64 digits. q*d reconstruction
and guarded products have at most65 significant result digits; their shorter
factor has<=32 digits, selecting bounded schoolbook multiplication, not recursive
Karatsuba. Temporary buffers are finite digit arrays, not input-sized text or
unbounded precision. Vector capacity/allocator overhead is implementation-specific;
this is not a claim of an exact heap-byte or wall-time ceiling.

Other targets retain the4096bit operand guards; their library limb/algorithm
accounting must be re-audited before claiming the same limb-level cost proof.
No architecture-specific unsafe code or system call is introduced. Acceptance requires both process `ulimit -s 1024` and runtime-only
`RUST_MIN_STACK=1048576` for libtest spawned threads, including4096bit division.
The original ulimit-only record did not establish a1MiB test-thread stack and
is superseded by separately retained corrected runs. Compiler threads are not
constrained; explicit thread-stack overrides are not covered by this claim.
This is not a proof about scheduler latency or allocation failure. Allocation failure
has normal Rust process behavior, not a recoverable financial error.

There are no retained caches/tasks/listeners, production state or I/O. Temporary
BigUints drop normally; each frame retains only a u16 counter. A future caller
must bound retained intermediate count as part of its expression graph. A
service crash discards calculations with no durable effects. DB/feed/API failure
cannot be handled by arithmetic; later8i/8h2 must qualify current durable evidence
and refuse new entry when it is unavailable, preserving existing leg protection.

## Evidence and independent oracle

`arithmetic_tests.rs` uses independent bounded i128 equations for generated
add/sub/mul/div/compare and floor/ceiling, without calling production helpers for
the expected values. Coefficients<10^6, denominators<10^4, fixed scaling<=10^12
bound every oracle intermediate below10^24, far below i128. Test-only final
cross-products compare rational identity using BigUint; they do not derive the
expected operations. Generated native coefficients cover96bits and all29scales.

`testdata/arithmetic.json` contains17 extreme cases independently generated by
Python stdlib Fraction: max96bits, recoverable oversized intermediates,
nonterminating/scale29 export, exact grid and one-low-order-unit ceiling, distinct
12/8 floors, negative subtraction and zero division. The reproducible `testdata/generate.py` uses only Python stdlib Fraction; its
independent generation evidence/hashes are retained in `.scratch-8g2a`. There
are no fixed-percentage chart fixtures because this slice has no stop geometry.

Required clean-copy mutants: wrong12-place floor, wrong8-place floor, lost
ceiling remainder, inexact export and frame-budget bypass. Each needs actual
crate compilation and a named assertion failure, not a compile/setup failure.
All shared sizing, ingress and cost qualification remain separately regression
tested; no tests here establish future full sizing/capacity or durable admission.

Tradeoff: private bounded BigUint primitives add some transient allocation and
local code but make exact boundaries auditable. They do not repair the shared
legacy arithmetic or expand its supported domain. Dependency pins/lock remain
unchanged. Format, Clippy and security/secret/dependency CI gates still apply;
no frontend, stream, infrastructure or production change is part of this slice.

## Private geometry staging (8g2b-1)

At the8g2b-1 checkpoint, the crate-private `balance_sizing` module introduced
raw fixed-shape observations and exact geometry verification. That staging
checkpoint had no public sizing entry point. The next section describes the
8g2b-2 raw calculation API; neither checkpoint creates a current receipt or
capital certificate. No production caller, SQL, HTTP, task or send capability
is added.

`validate` checks full binding and scalar domains, reconstructing RiskConfig
with the supplied basis and selected per-leg costs while retaining operator
headroom/buffer/distance bounds. It does not establish currentness. Explicit
cash.free is distinct from locked cash; reference P is distinct from the
required applicable filter valuation. A matching raw V_filter descriptor is
not an exchange observation certificate. Its actual producer, sealed8h2 age
authority and certified8l basis remain later caller blockers.

`lot` and `geometry` share a caller-owned Arithmetic frame. Their worst-case
prefix is32 charged calls: nesting2, minPrice-grid2, derivation12, final-grid2,
three distance checks/thresholds14. Maximum preflight is492bits including exact
export scaling; four native exports try at most29 scales each. Legacy omits
unused Dcap/span exports because its option fields must be None. Only required
exact outputs are narrowed; no intermediate rounding or unchecked Decimal
product/division is used. The final sizing consumer must continue this SAME
preparation frame for the remaining25 calls (57 total), then at most four
37-call candidate frames. This private slice does not claim to implement that
later financial graph or its872bit/774division full bound.

Geometry preserves the analyzer's second support on the15m chart, the long
adverse guard, policy buffer cap and adverse tick rounding. All supplied plan
fields must match, including exact cap_basis_distance even when nonbinding,
and policy-specific Some/None. Three distance stages use exact cross products.
Because long guard/buffer/tick order chart >= basis >= trigger, the middle
stage's acceptance inequality is logically implied by the two endpoints;
its explicit required evaluation is additionally covered by the32-call trace.
Tests must not claim a distinct financial counterexample for deleting only
that redundant middle check. A trace mutant enforces the declared stage/work
contract; independent endpoint mutants enforce financial refusal.

Controls use an independent bounded-i128 geometry oracle, safe-domain
comparison with the existing canonical builder, and Python Fraction extreme
goldens (max96bit/scale28 and policy-specific unrepresentability). Arithmetic
tracing is cfg(test) only. Test-only terms still pass through the existing
sealed fixture authority; there is no public proof constructor. Zero I/O and
no durable effects mean cancellation/crash discards only transient data.
Missing/stale external evidence must still be refused by later admission.

The first checkpoint kept the module and bridge private with a narrow staging
dead-code allowance. The consuming8g2b-2 slice removes that allowance and
exports only raw inputs and unqualified calculation data; the bridge stays private.
This split trades an extra private bridge for smaller independent reviews;
it changes neither policy nor the128-per-frame computational budget. Typed
TechnicalWorkLimit is a proof-invariant failure, never a financial cap.


## Unqualified candidate calculation (8g2b-2)

`calculate_balance_candidate` evaluates fixed raw observations; matching IDs
are not proof of currentness, certified capital or admission. Result fields
are private and immutable, retaining copied original terms/hash, observations
and exact S/Q/AF/L/Rhat/a/gross. The synchronous spot `sizing_inputs` projection
copies full relevant price/lot/notional metadata from an already-read record,
checking spot venue, configured symbol/assets, status and bounded order types.
It cannot authorize account permissions or STP. No production caller exists.

Preparation uses the SAME Arithmetic frame for the32-call geometry prefix and
25 remaining calls. Candidate0..3 each get one independently bounded frame,
maximum37 calls:57+4*37=205, within the unchanged128-per-frame/640 ceiling.
The independent symbolic table is approved in8g2bP1; implementation traces also
check maximal operands872bits, <=26 exact exports and <=774 integer divisions.
Exact export may try29 scales internally; these divisions are counted separately
from charged formula operations. TechnicalWorkLimit is an invariant failure,
never an exposure cap or an alternate sizing policy.

Native imports are also bounded. Geometry consumes at most24 conversions;
remaining preparation20; each candidate reuses converted price/AF/step/S/Q/
minimum/maximum/filter values and needs at most12. Total <=44+4*12=92, below
approved128. The initial draft repeated conversions and failed the bound at154
in a four-candidate late-Unrepresentable case with all nonbinding filters/cap
active. Local reuse fixes that computational issue without changing charged
operations, bits, results or counter resets. Its regression/mutant is a WORK
budget check, not a financial-semantic kill. Native scalar comparisons remain
fixed (conservative<=256); no caller-controlled list/exponent enters the graph.

B is the supplied capital basis, A the guard-aware instrument stop. Risk quantity
uses floor12 of(B/100)/L; physical quantity uses floor8 ofmin(B,free)*h/P. Both
then use the lot step. Every allowed candidate prices grossQ=ceil(S/(1-f)),
Rhat=min(S,ceil(minNotional/(AF*(1-m)))), and
`a=S*L+(Q-S)*P+Rhat*AF*(1-f)`. No clamp, fourth reduction, percentage chart stop
or shared USD-M helper is introduced. Stop early only when S is nonpositive or
below mandatory quantity minimum; an unrepresentable S/Q/charge may still
permit a later candidate and must not terminate all attempts.

S0 stays rational: the max96bit/scale28 Fraction fixture has an unrepresentable
initial S0 and an accepted second candidate. R0 also stays rational until min;
its huge-R0 fixture correctly reaches an ordinary minimum refusal instead of
an artificial preparation Unrepresentable error. It is NOT an example of an
accepted trade below its protection minimum. Exact final outputs never round;
a separate late-unrepresentable fixture passes all financial bounds before
all four final-export refusals. Risk-floor12, physical-floor8, margin reserve,
gross-cash and tiny-quote cancellation goldens cover the actual public function.

Applicable venue min/max compare bothQ*V_filter andS*V_filter; P and AF are not
substitutes. Financially, Q upper and S lower dominate their counterparts when
Q>=S; mutation claims must not count redundant counterpart deletion as distinct
financial coverage. Envelope minima, physical/operator gross caps and1% charge
remain separate. The actual V_filter producer is still absent: later8i/8h must
supply an already-budgeted qualifying observation or keep entry closed. No extra
GET or composition budget is granted.8h2 sealed receipt,8l certified basis,
8g2c handoff/capacity and8i serialized admission remain pending integration.

The code retains no cache/listener/task/queue; all BigUint work is transient and
bounded. Cancellation has no durable effect. No live production memory-profile
claim is made for an inert calculator. Whole-workspace/security/CI and future
caller memory/atomicity acceptance remain required before activation.

## Candidate journal payload (8g2c-1)

`candidate_admission` copies the existing18-field AdmissionRow from an immutable
SizingCalculation. It checks non-nil position, exact configured symbol and the
original basis month. These are data checks, not governed-action authentication.
The selected nested lot step is retained at successful sizing and copied without
rerunning geometry. Original fee/gap/margin/rules, source/note/as_of/terms hash,
S/Q/reference price/Rhat/charge/initial trigger remain exact. It never reads a
clock, later fee cache, current config or alternate rules. The terms hash is
encoded as64 lowercase hex characters; source and symbol remain bounded.

The calculation is immutable; AdmissionRow remains existing public mutable data.
Later8i must compare/rebuild the exact payload and requestedQ before strictJ.
Intent/venue/scope/tenant/time still belong to J/backend. No row conversion
creates a receipt, certified basis, capacity reservation or send authorization.
All18 copies require0 new financial arithmetic,0 SQL/RI/HTTP/await/locks/tasks;
hash encoding visits exactly32 bytes. A crash loses only transient values.

Capacity2c-2 remains deferred until a real complete producer exists. This slice
adds no count API, reconciliation, initialX, latent-risk or runtime caller.
Actual8h2/8l/V_filter qualification, complete capacity deduplication and unknown
retention, serialized reservation, persistence/restart and send equality remain
separate acceptance. Existing USD-M and SD12/factory behavior is unchanged.
Tests establish field copying and source immutability, not those later guarantees.
