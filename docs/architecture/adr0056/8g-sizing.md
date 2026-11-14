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

The crate-private `balance_sizing` module now contains raw fixed-shape
observations and exact geometry verification. This is implemented pure data
validation, not a public sizing entry point, a current receipt, or a capital
certificate. Candidate sizing and the connector projection remain pending in
8g2b-2. No production caller, SQL, HTTP, task or send capability is added.

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

The staged module and bridge remain private. A narrow dead-code allowance
marks this inert boundary and must be removed by the reviewed consuming slice.
This split trades an extra private bridge for smaller independent reviews;
it changes neither policy nor the128-per-frame computational budget. Typed
TechnicalWorkLimit is a proof-invariant failure, never a financial cap.
