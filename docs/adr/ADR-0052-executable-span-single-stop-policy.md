# ADR-0052 — Executable Span: One Stop Policy, Buffer-Inclusive Unit of Risk

**Date**: 2026-08-04
**Status**: DECIDED — FOLLOW-UP REQUIRED (operator decision, 2026-08-04;
implementation pending)
**Deciders**: RBX Systems (operator + architecture)
**Partially supersedes**: [ADR-0050](ADR-0050-technical-stop-valid-level-selection.md)
(stop-policy runtime selector and the `span_capped_v1` variant)

---

## Context

ADR-0050 §3/§4 introduced versioned stop derivation with a runtime selector:
the `ROBSON_STOP_POLICY` environment variable chose between
`legacy_uncapped` (the historical derivation, uncapped executable-stop
composition) and `span_capped_v1` (buffer capped at 0.25 × span,
tick-quantized trigger, fail-closed). The selector shipped 2026-08-03 as a
rollout gate, defaulting to `legacy_uncapped`, and was never activated.

Three operator findings on 2026-08-04, watching a live SHORT armed under
`legacy_uncapped`, motivated this decision:

1. **Geometry**: the displayed executable stop sat ~3 technical spans from
   entry while the trailing ladder stepped in raw-span units (~159 points),
   so the stop distance visibly disagreed with the target/trailing ruler.
2. **Buffer philosophy** (operator, verbatim intent): the buffer exists
   only to avoid the "stop-hunt" pattern of resting the stop exactly on the
   technical level; it must be a small nudge past the level and must not
   alter stop-distance policy. The unit of risk the operator reasons about
   is the full distance to where execution actually triggers.
3. **One Robson**: risk behavior selected by an environment variable means
   two products in one binary. Risk parameters are product definition, not
   configuration (ADR-0024 fixed 1%/4% for exactly this reason), and
   behavior changes belong to versioned decisions plus persisted state, not
   deploy-time config (the same reasoning ADR-0051 applied to the monthly
   budget model).

Verified precondition (2026-08-04, production and testnet databases):
`span_capped_v1` has never been persisted. Production `positions_current`
holds 57 rows, all `legacy_uncapped`; the testnet schema predates the
`stop_policy` column entirely. A never-persisted variant can be deleted
outright with no compatibility burden.

## Decision

### 1. One arming policy: `executable_span`

All new positions are armed under a single stop policy, `executable_span`,
written directly in code as product definition. There is no runtime
selector of any kind.

`executable_span` is the ADR-0050 §3/§4 mechanics with one redefinition:

- **Executable stop** = technical level composed with the anti-stop-hunt
  buffer, where the **effective buffer is capped at 0.25 × the technical
  level distance** (`SPAN_CAP_RATIO` unchanged), the trigger is
  **tick-quantized adversely** from `SymbolTradingRules`, and a degenerate
  span (missing, zero, or negative) is a **hard error: fail closed, never
  fall back to an uncapped composition**. Guard interplay (ADR-0042) keeps
  the ADR-0050 normative basis: while the entry-time guard binds, the
  normative distance is measured against the guard-clamped basis; after
  release, against the technical level.
- **The span is buffer-inclusive**: `span := |entry_reference −
  executable_stop|`. This single number is the position's unit of risk,
  movement, and decision. The discrete-step trailing ladder, the
  stop-advance targets, latent-risk reservation, and planned-risk sizing
  all use it. The previous split (money-risk priced on the executable
  distance while the ladder stepped in raw technical-span units) is
  abolished: one ruler.

Consequence accepted knowingly: ladder steps grow by the buffer (bounded at
25% of the level distance), so the stop advances slightly less often, in
slightly larger steps, and the breakeven lock requires marginally more
favorable movement. This is the price of a single coherent unit of risk.

### 2. `ROBSON_STOP_POLICY` is removed

The environment variable, its parsing, its documentation, and the
`StopPolicy` selection path for new armings are deleted. Arming code
constructs `executable_span` unconditionally. A deploy cannot change stop
derivation; only a new ADR can.

### 3. `span_capped_v1` is deleted

Because it was never persisted (verified precondition above), the
`StopPolicy::SpanCappedV1` variant, its string, its selection arm, and its
documentation are removed rather than deprecated. No compatibility code is
written for a value with no history.

### 4. `legacy_uncapped` becomes provenance-only

The per-position `stop_policy` stamp recorded at arm time is **history, not
configuration**, and is retained: event-log replay and startup recovery
must reproduce each position's derivation as armed. Concretely:

- Positions already stamped `legacy_uncapped` keep the legacy derivation
  until they close. The legacy derivation code remains only for them and is
  not selectable for new arms.
- New positions are stamped `executable_span`. The database `CHECK`
  constraint is migrated to `('legacy_uncapped', 'executable_span')`.
- When no open position stamped `legacy_uncapped` remains, the legacy
  derivation code becomes a cleanup candidate (closed historical positions
  do not re-derive stops; the stamp on their events suffices for audit).

### 5. Versioning discipline

Future changes to stop derivation follow the same rite as this one: a new
ADR defines a new stamped policy name; new arms use it; open positions keep
the stamp they were born with. The version lives in the decision record and
the event log, never in a runtime knob.

## Consequences

- Positive: one Robson. Stop geometry is deterministic from the code
  version alone; the dashboard's stop, targets, and trailing steps agree
  with the priced risk by construction; the operator's original anomaly
  (stop distance ≈ 3 × first-target distance) cannot recur for new arms.
- Positive: configuration surface shrinks; one env var and one enum variant
  are deleted; no dual-behavior testing matrix.
- Trade-off: no fast rollback knob for the new derivation. The next real
  arming is the first production exercise of `executable_span`; reverting
  would be a deploy rollback, not an env flip. Accepted by the operator
  (armings are manual and individually supervised).
- Trade-off: slightly coarser trailing (bounded by the 25% buffer cap), as
  described in Decision 1.

## Alternatives

- **Keep the selector, activate `span_capped_v1`** — rejected: preserves
  the two-Robson configuration surface the operator explicitly refused,
  and keeps the ladder on the raw span, leaving the geometry split.
- **Rename only (cosmetic v1 removal), keep env var** — rejected: the
  objection is the runtime bifurcation, not the suffix.
- **Soak `span_capped_v1` behind the env var before removal** — rejected
  by the operator in favor of the direct cut: armings are manual, low
  frequency, and individually watched; a staged soak would keep the
  selector alive precisely to exercise a variant scheduled for deletion.
- **Fold the buffer into the technical span at analysis time** (make the
  analyzer emit a padded level) — rejected: hides the buffer inside chart
  analysis, contaminating the technical level's meaning (ADR-0021
  separates analysis from execution concerns); composition stays explicit
  in the executable-stop resolver.

### Decision matrix

| Option | Runtime selector? | Ladder ruler | Buffer bound | Principal trade-off | Decision |
| --- | --- | --- | --- | --- | --- |
| `executable_span` single policy | None | Buffer-inclusive executable span | 0.25 × level distance, tick-quantized, fail-closed | No fast rollback knob; next real arm is first prod exercise | Accepted |
| Activate `span_capped_v1` via env var | `ROBSON_STOP_POLICY` | Raw technical span | 0.25 × span | Keeps two-product config surface and split geometry | Rejected |
| Cosmetic rename, keep selector | `ROBSON_STOP_POLICY` | Raw technical span | 0.25 × span | Objection was bifurcation, not the name | Rejected |
| Soak behind env var, then remove | Temporary | Raw span during soak | 0.25 × span | Keeps selector alive to exercise a variant scheduled for deletion | Rejected |
| Pad the level at analysis time | None | Padded "technical" span | Hidden in analysis | Contaminates chart-level semantics (ADR-0021) | Rejected |

## Implementation Notes

- Code paths: `robson-domain/src/stop_policy.rs` (variant swap),
  `robson-domain/src/executable_stop.rs` (rename `SpanCappedV1` arm to
  `ExecutableSpan`; span redefinition), `robson-engine/src/trailing_stop.rs`
  (ladder consumes the buffer-inclusive span),
  `robson-domain/src/value_objects.rs` (`TechnicalStopDistance::span()`
  semantics vs the executable span; decide whether the ladder input moves
  to the `ExecutableStopPlan`), `robsond/src/config.rs` (delete
  `ROBSON_STOP_POLICY` parsing), `robsond/src/position_manager.rs` (arm
  path stamps `executable_span`), `robsond/src/api.rs` (policy strings),
  migration updating `chk_positions_stop_policy`.
- Migration note: the existing `CHECK` lists `span_capped_v1`; replace the
  constraint in the same migration that introduces `executable_span`.
- Tests to pin: new arm stamps `executable_span` and derives ladder steps
  equal to `|entry − executable_stop|`; a `legacy_uncapped` position's
  derivation is byte-identical before and after the change (replay
  invariance); degenerate span fails closed; buffer cap at exactly
  0.25 × level distance; tick quantization adverse on both sides; env var
  absence is not an error and presence is ignored or rejected loudly
  (decide in implementation review; rejection preferred: fail fast on
  stale manifests).
- Deploy note: remove `ROBSON_STOP_POLICY` from any rbx-infra manifests in
  the same rollout window (it was never set in production; the removal is
  hygiene).
- Related: ADR-0021 (technical stop from chart analysis), ADR-0024 (risk
  parameters are product definition), ADR-0041 (executable-stop buffer),
  ADR-0042 (invalidation guard), ADR-0049 (event log is audit), ADR-0050
  (partially superseded: §3/§4 mechanics absorbed into `executable_span`;
  runtime selector and `span_capped_v1` retired), ADR-0051 (persisted-state
  activation precedent).
