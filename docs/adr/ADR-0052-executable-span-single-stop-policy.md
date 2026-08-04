# ADR-0052 — Executable Span: One Stop Policy, Buffer-Inclusive Unit of Risk

**Date**: 2026-08-04
**Status**: DECIDED — IMPLEMENTED (repository; operational rollout pending)
**Implementation status** (2026-08-04):
`feat/adr-0052-executable-span-impl` implements the single-policy domain,
admission persistence and projection, immutable-span replay/recovery,
entry-anchored engine and frontend behavior, executable-trigger latent-risk
pricing, migration, and regression tests. The manifest-validation half of
rejecting `ROBSON_STOP_POLICY` lives in `rbx-infra` and is pending;
arming-quiescence remains an explicit operator rollout step with no code
enforcement in this repository.
**Deciders**: RBX Systems (operator + architecture)
**Partially supersedes**: [ADR-0050](ADR-0050-technical-stop-valid-level-selection.md)
only for the runtime selector (`ROBSON_STOP_POLICY`) and the
`span_capped_v1` identifier. ADR-0050 §3 executable costing, §4 buffer-cap
mechanics (renamed here as the `cap_basis_distance` rule), and §5 staged
stop-distance bounds remain in force.
**Amends**: [ADR-0041](ADR-0041-executable-stop-buffer.md): the
chart-derived technical level remains a separate concept, but its raw
distance no longer drives the new-position ladder, targets, or risk
derivation; the executable span does.
**Preserves**: ADR-0039's two-layer same-trigger enforcement and market-exit
semantics; ADR-0042's entry-only invalidation guard and first-advance
release.

---

## Context

ADR-0050 §3/§4 introduced versioned stop derivation with a runtime selector:
the `ROBSON_STOP_POLICY` environment variable chose between
`legacy_uncapped` (the historical derivation, uncapped executable-stop
composition) and `span_capped_v1` (buffer capped at 0.25 × span,
tick-quantized trigger, fail-closed). The selector shipped 2026-08-03 as a
rollout gate, defaulting to `legacy_uncapped`, and was never activated.

Three operator findings on 2026-08-04, watching live SHORTs armed under
`legacy_uncapped`, motivated this decision:

1. **Geometry**: one arm's executable stop sat ~3 technical spans from
   entry while the trailing ladder stepped in raw-span units, so the stop
   distance visibly disagreed with the target/trailing ruler; a later arm
   the same day sat at ~1.18 spans. Geometry varies arm by arm under the
   legacy composition.
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

## Decision

### 1. One arming policy: `executable_span`

All new positions are stamped `executable_span`; no runtime value selects
another policy.

Let `E` be the signal entry reference, `T0` the original chart-derived
technical stop, and `A0` the adverse guard-clamped basis while the ADR-0042
guard binds.

Define the pre-buffer cap distance as:

```
cap_basis_distance =
    |E − A0|      while the guard binds
    |E − T0|      after guard release
```

After guard release, the second expression always means the original
arm-time `TechnicalStopDistance::span()`. It is never measured from `E` to
the current trailing stop and is never the executable span.

The resolver computes:

```
effective_buffer = min(configured_buffer_at_basis, 0.25 × cap_basis_distance)

X0               = adverse_tick_quantize(basis ± effective_buffer)
executable_span  S = |E − X0|
```

Both `cap_basis_distance` and `S` must be present and strictly positive.
Failure is a hard error; no uncapped fallback is permitted. Adverse tick
quantization is part of `S`, so the total widening over the basis is the
buffer plus up to one adverse tick.

`S` is computed once from the admission-time executable plan, recorded
durably (Decision 5), and remains immutable for the position lifetime.
Guard release, trailing-stop advances, configuration changes, and
exchange-metadata refreshes do not redefine it.

The entry-anchored ladder uses:

```
completed_spans = floor(favorable_distance(entry_reference, watermark) / S)

LONG  candidate technical stop = E − S + completed_spans × S
SHORT candidate technical stop = E + S − completed_spans × S
```

The first completed span therefore moves the conceptual technical stop to
the entry reference. The current executable trigger remains the result of
applying the buffer and adverse tick quantization to that candidate,
exactly once, in the single resolver. This is a technical-price breakeven
lock, not a guarantee of executable or economic breakeven after buffer,
gap, and fees.

The software exit comparison and the ADR-0039 insurance stop always consume
the same resolved executable trigger. No consumer may apply the buffer a
second time. Dashboard targets use the entry-anchored formulas above, not
`current_technical_stop ± 2 × S`.

Admission sizing remains ADR-0050 §3 costing: `S` is the trigger-distance
component, while gap allowance and both taker fees remain in
`worst_case_loss_per_unit`. Latent risk uses the current executable trigger
and the ADR-0051 residual cost model, never the raw technical stop alone.

```
chart analysis     guard (ADR-0042,     buffer cap        adverse tick
(T0, level) ──┐    while binding: A0)   0.25 × basis      quantization
              ├──> basis ────────────> +buffer ─────────> X0 (trigger)
entry ref E ──┘                                            │
                       S = |E − X0|  (immutable, persisted at admission)
                       │
        ┌──────────────┼───────────────┬─────────────────┐
     ladder steps   latent risk     sizing (§3 cost)   FE targets
```

### 2. `ROBSON_STOP_POLICY` is removed

`robsond` does not read `ROBSON_STOP_POLICY`. Deployment validation rejects
a manifest that still declares it; its presence can never select runtime
behavior. The parsing code, its documentation, and the `StopPolicy`
selection path for new armings are deleted. A deploy cannot change stop
derivation; only a new ADR can.

### 3. `span_capped_v1` is deleted

Deletion is permitted only after durable-history checks return zero in
every production, testnet, retained event-log partition, and applicable
backup:

- `positions_current.stop_policy = 'span_capped_v1'`;
- `event_log` rows whose `payload::text` contains `span_capped_v1`
  (covers `position_armed` stamps and any other payload);
- every snapshot table/partition whose row text contains the stamp.

The query text, environments, timestamps, and results are retained as
repository evidence:
[docs/analysis/2026-08-04-span-capped-v1-preflight.md](../analysis/2026-08-04-span-capped-v1-preflight.md)
— all checks returned zero on 2026-08-04 (production projection: 57 rows,
all `legacy_uncapped`; production event log and all snapshot partitions: 0;
testnet event log: 0; testnet projection predates the column). The check
must be re-run with zero results immediately before the implementation
merge. A missing projection column is not evidence that event payloads are
absent; the event log is the source of truth.

If any hit exists, deployment is blocked. Event history must not be
rewritten and the value must not be silently mapped to another policy;
compatibility handling would require a new operator decision.

Given zero durable history, the `StopPolicy::SpanCappedV1` variant, its
wire string, its selection arm, and its documentation are removed rather
than deprecated. No compatibility code is written for a value with no
history.

### 4. `legacy_uncapped` becomes provenance-only

The per-position `stop_policy` stamp recorded at arm time is **history, not
configuration**, and is retained: event-log replay and startup recovery
must reproduce each position's derivation as armed. Concretely:

- Positions already stamped `legacy_uncapped` keep the legacy derivation
  until they close. The legacy derivation code remains only for them and is
  not selectable for new arms.
- New positions are stamped `executable_span`. The database `CHECK`
  constraint is migrated to `('legacy_uncapped', 'executable_span')`
  (Rollout order below).
- The `legacy_uncapped` wire value, deserializer, projector support,
  database allowance, and historical stamps remain forever. Only its
  executable-derivation branch becomes removable, after repository evidence
  proves that no risk-bearing legacy position remains (closed historical
  positions do not re-derive stops; the stamp on their events suffices for
  audit).

### 5. Persistence and replay

The policy stamp is recorded by `PositionArmed`. Because the executable
plan does not exist until the entry signal is admitted, the admission-time
event must also record at least `initial_executable_stop`,
`executable_span`, `cap_basis_distance`, and the tick size used for
quantization, before any entry order is submitted. `positions_current`
projects the immutable `executable_span` for Active-position recovery and
API use.

For `executable_span`, replay consumes the persisted number; it does not
recompute the arm-time span from current exchange metadata. A missing or
non-positive persisted span on a risk-bearing `executable_span` position is
a fail-closed recovery quarantine with durable operator-visible evidence.

### 6. Versioning discipline

Future changes to stop derivation follow the same rite as this one: a new
ADR defines a new stamped policy name; new arms use it; open positions keep
the stamp they were born with. The version lives in the decision record and
the event log, never in a runtime knob.

## Rollout order and rollback boundary

`20240101000024_add_stop_policy.sql` is already-applied history and MUST
NOT be edited. A new forward-only migration drops and recreates
`chk_positions_stop_policy` as `('legacy_uncapped', 'executable_span')`,
updates the column comment, and adds the projection fields required by
Decision 5.

Rollout order is mandatory:

1. Re-run the Decision 3 durable-history checks; record zero results.
2. Remove `ROBSON_STOP_POLICY` from deployment manifests and quiesce new
   armings; no old daemon may arm during the schema/code transition.
3. Apply all migrations, including migration 000024 on environments that
   still lack it (testnet) and then the ADR-0052 migration.
4. Start the ADR-0052 binary and verify schema readiness, replay, legacy
   recovery, trading-rules availability, and insurance-stop health.
5. Re-enable arm requests.

Rollout compatibility note: for new arms, a quantity below the exchange
minimum is now a governed signal rejection (`HTTP 200`, position remains
`Armed`, detail visible through entry status/events) rather than the former
`HTTP 400` adapter error. New-arm sizing is lot-quantized at the exchange's
quantity precision (six decimal places for the current contract) instead of
the historical generic `round_dp(12)` intermediate. Neither change is applied
retroactively to legacy position replay.

Starting ADR-0052 code against the old constraint is prohibited. Allowing
an old writer to arm between migration and deployment is prohibited.

Rollback to the old binary is safe only before the first `executable_span`
`PositionArmed` event. After that event, the new wire value makes the
release forward-only; remediation is a forward fix or a compatibility build
that understands `executable_span`, never deployment of the pre-ADR binary.

## Failure modes

| Failure | Required behavior | Durable visibility |
| --- | --- | --- |
| Database migration absent or wrong | No new arm is accepted; daemon does not become ready against an incompatible schema | Migration/readiness failure and operator alert |
| Trading rules unavailable on new arm | Fail the arm closed; never use unquantized or legacy derivation | Governed rejection with symbol and cause |
| Trading rules unavailable during recovery | Preserve the existing insurance order and quarantine plan mutation | Durable recovery-check event and high-severity alert |
| Daemon dies | Exchange insurance stop remains authoritative; restart replays the immutable policy/span and heals idempotently | Insurance-stop check/heal event |
| Deleted policy found in durable history | Abort migration/deployment; do not rewrite or remap history | Stored preflight result and release blocker |
| Rollback requested after first new stamp | Reject the old binary; forward repair only | Deployment compatibility check |

## Consequences

- Positive: one Robson. No runtime-selectable risk behavior; the executable
  stop, ladder steps, targets, latent risk, and sizing agree on a single
  persisted unit of risk (`S`) by construction. The operator's original
  anomaly (stop ruler disagreeing with the target ruler) cannot recur for
  new arms.
- Positive: configuration surface shrinks; one env var and one enum variant
  are deleted.
- Trade-off: ladder steps grow relative to the raw technical span. After
  guard release the growth is bounded (buffer capped at
  0.25 × cap_basis_distance, plus one adverse tick); while the entry-time
  guard binds, `S` inherits the guard widening and is **not** bounded
  relative to the raw technical span. A guard-widened arm therefore gets
  proportionally wider rungs and a later breakeven lock; this is the
  honest cost of one ruler, and it is visible at arm time in the persisted
  `S`.
- Trade-off: no fast rollback knob for the new derivation. The next real
  arming is the first production exercise of `executable_span`; reverting
  is bounded by the rollback boundary above. Accepted by the operator
  (armings are manual and individually supervised).
- Testing: no runtime-selectable behavior matrix for new arms; legacy
  provenance and executable-span paths remain separately regression-tested
  until the last live legacy position closes.

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
| `executable_span` single policy | None | Persisted buffer-inclusive `S` | 0.25 × cap_basis_distance, tick-quantized, fail-closed | No fast rollback knob; guard-widened arms get wider rungs | Accepted |
| Activate `span_capped_v1` via env var | `ROBSON_STOP_POLICY` | Raw technical span | 0.25 × cap_basis_distance | Keeps two-product config surface and split geometry | Rejected |
| Cosmetic rename, keep selector | `ROBSON_STOP_POLICY` | Raw technical span | 0.25 × cap_basis_distance | Objection was bifurcation, not the name | Rejected |
| Soak behind env var, then remove | Temporary | Raw span during soak | 0.25 × cap_basis_distance | Keeps selector alive to exercise a variant scheduled for deletion | Rejected |
| Pad the level at analysis time | None | Padded "technical" span | Hidden in analysis | Contaminates chart-level semantics (ADR-0021) | Rejected |

## Implementation Notes

- Code paths: `robson-domain/src/stop_policy.rs` (variant swap),
  `robson-domain/src/executable_stop.rs` (rename the `SpanCappedV1` arm to
  `ExecutableSpan`; emit `S` and `cap_basis_distance` on the plan),
  `robson-domain/src/events.rs` (admission-time payload per Decision 5),
  `robson-engine/src/trailing_stop.rs` (ladder consumes persisted `S`;
  entry-anchored formulas of Decision 1),
  `robson-engine/src/lib.rs` (single application of the buffer via the
  resolver; no second application after ladder advance),
  `robsond/src/config.rs` (delete `ROBSON_STOP_POLICY` parsing),
  `robsond/src/position_manager.rs` (arm path stamps `executable_span`;
  persistence of `S`), `robsond/src/api.rs` (policy strings; expose `S`),
  `frontend/src/lib/presentation/labels.ts` (targets from the
  entry-anchored formulas, not `stop ± k × tech_stop_distance`),
  projector/store projection fields, startup recovery (replay consumes
  persisted `S`), migration replacing `chk_positions_stop_policy`.
- Startup verifies all four ADR-0052 projection columns before the daemon can
  serve traffic, and projection recovery errors abort startup. Recovery also
  requires `initial_executable_stop` for `ExecutableSpan` positions in
  `Entering`; historical `Active` rows may omit it because their live stop is
  resolved from trailing state plus persisted `S` and cap basis.
- The canonical monthly-budget snapshot resolves `ExecutableSpan` Active
  positions from the current trailing stop and cached live trading rules,
  prices the trigger with the ADR-0051 gap/fee envelope, and reuses that exact
  reservation in admission, slots, status, and MonthlyHalt. Entering positions
  use their persisted initial executable trigger. Resolution failure sets the
  snapshot invalid, forces zero remaining capacity, and emits a durable
  high-severity warning; legacy raw-stop arithmetic is unchanged.
- Fill-time insurance resolves against live rules. A persisted/live mismatch
  emits a critical drift event with both triggers and tick sizes; resolver
  failure falls back to the persisted trigger and then the initial technical
  stop so a real fill is protected before operator quarantine evidence is
  persisted.
- The persist-before-execute barrier is intentionally scoped to the entry
  admission sequence (`EntrySignalReceived`, `EntryOrderRequested`, then
  `PlaceEntryOrder`). Exit, protective-order, recovery, and audit-only cycles
  retain execute-all-then-persist batch semantics so an event-log outage does
  not suppress an exit or insurance placement.
- Migration 20240101000026 enforces the four admission columns as structurally
  write-once: `NULL` may become a value and replay may repeat that value, but a
  non-`NULL` value cannot change.
- The architecture decision and risk-engine specification are aligned with
  the executable trigger as the canonical latent-risk basis;
  `TechnicalStopDistance::span()` is retained only for the explicitly labeled
  legacy provenance path and the admission cap basis.
- Regression tests pin that a new arm stamps `executable_span`, persists `S`, and the
  ladder's first completed span moves the candidate technical stop to the
  entry reference with the executable trigger derived exactly once; a
  `legacy_uncapped` position's derivation is byte-identical before and
  after the change (replay invariance); degenerate span fails closed;
  buffer cap at exactly 0.25 × cap_basis_distance; adverse tick
  quantization on both sides; replay after an exchange tick-size change
  consumes the persisted `S` (never re-derives); recovery quarantine on
  missing/non-positive persisted `S`; property tests retain trailing
  monotonicity, guard release, and soft-stop/insurance-stop trigger
  equality. Repository code no longer reads `ROBSON_STOP_POLICY`; deployment
  manifest rejection remains pending in `rbx-infra` as recorded above.
- Related: ADR-0021 (technical stop from chart analysis), ADR-0024 (risk
  parameters are product definition), ADR-0039 (two-layer enforcement,
  preserved), ADR-0041 (amended: raw distance no longer drives the
  ladder), ADR-0042 (guard, preserved), ADR-0049 (event log is audit),
  ADR-0050 (partially superseded as scoped in the header), ADR-0051
  (persisted-state activation precedent).
