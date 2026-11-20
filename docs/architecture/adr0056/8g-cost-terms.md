# ADR-0056 step 8g1b: pure cost terms

This slice implements exact commission interpretation, injected configuration and
immutable cost selection. It adds no cache, worker, receipt clock implementation,
SQL, migration, environment reader, production caller or send authorization.
The spot factory and `AccountPort::execution_costs` remain unchanged. Sizing is
8g2; durable receipts and their actual age authority are 8h prerequisites.

## Inputs and units

The preceding ingress slice validates original spot decimal strings. Commission
rates and discount now retain their opaque `ExactDecimal` wrappers in the typed
record, so the pure handoff does not accept a parse-then-reformat approximation.
`SpotCommission::cost_candidate` verifies the response symbol against the supplied
scoped identity and performs no I/O. It returns validated candidate data, never a
qualified storage receipt.

The pure constructor validates all12 nonnegative commission components. For both
maker and taker, it sums the standard, tax and special rate plus their respective
buyer/seller contributions, and takes the larger six-term total. No component is
omitted and no discount is applied. Both discount flags true refuse Base policy,
including zero discount or another asset. The other flag combinations permit the
undiscounted rates. Taker fraction must remain strictly below0.01; gap remains in
0..100bps. Funding is absent.

Arithmetic aligns six native96-bit coefficients to scale28, sums and multiplies
by10,000 using pinned `num-bigint=0.4.8`. This fixed expression needs fewer than208
bits, with at most32 zero-normalization iterations (including bps-to-fraction conversion). Outputs must fit native96-bit
Decimal exactly. Each selected rate's fraction is converted exactly once and
stored; an override bps value whose division by10,000 needs nonzero scale>28 is
refused. No f64, rounded Decimal division, clamp or arbitrary precision input is
accepted. `sha2=0.10.9` supplies canonical hashing. Both dependencies already
existed in Cargo.lock; only their robson-exec edges are added, without a package
version, source or checksum change.

## Injected configuration

`CostPolicy::parse` takes gap text, optional notional-margin-bps text and optional
max-entry-notional text. All use the same128-byte exact grammar. Missing margin
uses approved200bps; margin is nonnegative and below10,000bps. A configured cap
must be positive. This parses units only; it performs no sizing or policy fallback.
The existing daemon USD-M configuration and `ROBSON_TAKER_FEE_RATE` are untouched.

`parse_dated_override(body, &configured_identity)` accepts at most4096 bytes, denies duplicate/unknown fields,
and requires this explicit version1 JSON shape (example identity and values only):

```json
{
  "version": 1,
  "tenant_id": "00000000-0000-0000-0000-000000000001",
  "venue": "binance:spot",
  "scope": "configured-scope",
  "uid": 7,
  "symbol": "BTCUSDT",
  "maker_bps": "8",
  "taker_bps": "10",
  "as_of": "2026-11-14T12:00:00Z",
  "note": "original dated operator terms"
}
```

The explicit tenant/venue/scope/UID/symbol must equal the already configured
identity. The parser clones its canonical base/quote symbol; it does not guess
quote suffixes or use the legacy fixed quote list. Configured USDC and other
quote assets therefore retain the same fallback. Venue metadata/tradability and
unknown-symbol handling remain their existing owners, including later8k; this
parser adds no discovery or fallback for an unknown symbol. The version1 JSON
shape is unchanged.

Rates must be strings, not JSON numbers. Note is at most256 bytes, scope128,
venue64 and each symbol asset32; blank/control-character identities/provenance
refuse. The original date and note are retained. Parsing does not claim freshness,
register an override, reset its timestamp, or read process environment. The sample
chooses no production rate, symbol, cap or activation decision.

## Receipt authority and selection

`CostAgeAuthority` has a private supertrait and **no production implementation**.
`AgeInterval` has no public constructor or deserializer; compile-fail tests protect
both boundaries. The future reviewed8h producer must verify positive persistence,
tenant/venue/scope/UID/symbol, full evidence hash, validation version and original
as_of before returning the original nonnegative age interval at selection time.
A caller supplies neither a new wall-clock age nor a reusable proof object.
An inverted interval refuses as unknown. The upper bound must be strictly below
24h; exactly24h is stale. Unknown/future/unresolved age never permits fallback.

The authority also positively confirms transport/storage absence before an
unavailable venue result can use an override. An unresolved storage result and a
known malformed or incompatible response refuse instead. A valid fresh venue
candidate wins even if the unused override configuration is invalid; a stale or confirmed-absent candidate may use a fresh qualified
scoped dated override. No external-date initial age is manufactured here. Same
BOOTTIME-domain recovery, original receipts and unknown-domain fallback remain
8h's approved joint contract, not code implemented by this interface.

Canonical evidence hashing is versioned and length-delimited, with normalized
exact amounts, units, identity, original date, source, Base policy, discount flags
and all source components. Equivalent1.0 and1.00 hash equally; source, identity,
date or note changes do not. Selected terms add the separately identified policy
gap to their own hash. The envelope source preserves the override note for the
existing AdmissionRow source field. `CostTerms` fields are private with no unchecked constructor
or deserializer. Cloning/replacing candidate/config state cannot edit selected
terms. The envelope is data, not a journal ticket or final admission authorization;
8i must still revalidate the prepared snapshot before committing or sending.

## Bounds and failures

All functions are synchronous and perform **0 HTTP,0 SQL**, with bounded strings,
fixed12-component validation, four six-term sums, and bounded hash/arithmetic work.
There is no cache, queue, retained listener, background task or await/lock race.
Property tests compare generated fee calculations to an independent fixed-scale
integer oracle. The existing full spot wire suite verifies unchanged request counts.

| Condition | Outcome |
|---|---|
| Invalid original amount, policy flags, configuration or exact conversion | Typed refusal; no rounded or discounted terms |
| Missing/unresolved persistence or clock authority | Typed refusal; no fabricated freshness or override concealment |
| Stale venue, qualified fresh override | Original override identity/date/note retained |
| Stale both, future date, mismatched receipt identity | Closed selection for new entries |
| Database/API/process fails | No I/O/recovery owned here; later8h producer must report unresolved/absence precisely |
| New fee/config selection after a leg is admitted | Existing immutable/persisted per-leg terms remain authoritative |

The tradeoff is explicit: strict exactness may refuse very small bps values that
cannot also be represented as fractions; sealed authority intentionally makes this
slice unusable for production selection until8h is independently implemented and
accepted. No availability, quota, chart-stop or trading-policy exception is added.
