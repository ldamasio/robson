# ADR-0050 — Valid-Level Selection, Per-Policy No-Valid-Stop Semantics, and Executable Risk Costing

**Date**: 2026-08-02 (v2, after three-way architecture review)
**Status**: PROPOSED
**Deciders**: RBX Systems (operator + architecture)
**Amends**: ADR-0021 (level selection rule AND the sizing formula in its invariant 5),
ADR-0041 (span-aware cap on the executable stop buffer)
**Related**: ADR-0024, ADR-0039, ADR-0042, ADR-0043, ADR-0048, issue #147, operator branch
`fix/immediate-risk-denial-and-leverage-label`
**Review trail**: v1 reviewed by Codex (gpt-5.6-sol, verdict: reject as written) and GLM 5.2
(verdict: approve with changes); v2 incorporated the consensus; v2.1 fixes four normative
errors found in PR review (wrong amended ADR, 1.25x baseline, ADR-0021 sizing pointer,
four-stage bounds closure). Full reviews on issue #147.

---

## Context

Production incident, 2026-08-02 18:45 UTC. Operator armed an immediate SHORT (free slot);
`TechnicalStopAnalyzer` picked the configured 2nd-closest resistance at 0.094% from entry
without checking bounds (only the ATR fallback path validates bounds), downstream
`TechnicalStopDistance::validate` rejected the signal ("Stop too tight (<0.1%)"), the
position was left Armed and inert, and the operator saw an opaque 500.

The analyzer holds the full ordered list of detected chart levels when it fails; deeper,
in-bounds levels may exist. Failing anyway converts an answerable WHERE question into an
operator-facing outage.

The v1 of this ADR proposed "closest level in bounds" selection and "keep the detector
alive until the chart yields a valid stop". Review rejected both:

- *Closest in bounds* silently overrides `support_level_n` (the configured thesis anchor)
  whenever level 1 is valid, changing position size and trailing profile without
  architectural authorization, and contradicting ADR-0021's second-level rule.
- *Detector alive until valid* converts a WHERE failure into an autonomous WHEN decision:
  the system could enter hours later, in a different price regime, without fresh operator
  intent. In `Immediate` mode, WHEN belongs to the operator.
- The v1 sizing example ignored the 1x margin cap (its quantity implied ~4× capital in
  notional), the insurance-stop offset, fees, and lot quantization.

---

## Decision

### 1. Level selection: anchor at N, walk deeper only

After directional filtering and clustering, with levels ordered closest-first:

1. Evaluate the configured `support_level_n` (default 2) first.
2. If its distance is **below** `min_stop_distance`: walk to `N+1`, `N+2`, … and select
   the first level whose distance is within bounds. Never select a level shallower
   than `N`.
3. If the level at `N` is **above** `max_stop_distance`: deeper levels are wider still;
   do not walk. Proceed to the fallback chain (step 5).
4. If fewer than `N` levels exist: use the deepest available level **only if in bounds**
   (existing degraded path, now bounds-checked); otherwise proceed to the fallback chain.
5. Fallback: ATR-based stop with existing bounds validation (unchanged, still
   fallback-only per ADR-0021).
6. If nothing yields an in-bounds stop, the outcome is `RetryableNoValidStop` (§2).

Audit (durable, additive): `configured_level_n`, `selected_level_n`, `skipped_levels`
(each with `{level, distance, reason}`), `selection_rule: "anchor_n_walk_deeper"`,
plus ATR inputs (`atr_value`, `multiplier`, resulting distance) whenever the fallback
ran. `confidence` downgrades one step whenever `selected_level_n != configured_level_n`.

This preserves ADR-0021: the stop is always a real chart level (or documented ATR
fallback); percentage-of-entry stops remain prohibited; the second-level rule remains
the anchor, amended only in the direction of deeper (more conservative) structure.

### 2. No-valid-stop semantics are per entry policy (never inert, never autonomous)

Analysis produces a typed `TechnicalStopDecision`:
`Selected | RetryableNoValidStop | DataUnavailable | TerminalPolicyError`.

Decision table (normative; state is durable and restart-rehydrated):

| Entry policy | On `RetryableNoValidStop` | On `DataUnavailable` |
|---|---|---|
| `Immediate` | **Single attempt** over a short-validity snapshot. Transition to durable terminal state `needs_operator_rearm` with the rejection numbers; alert the operator; **no autonomous later entry**. | Same terminal transition, distinct reason code. |
| `ImmediateUntilValid` (new, explicit opt-in) | Durable waiting state `armed_waiting_valid_stop`; re-evaluate per candle under governed backoff (5 s doubling, 15 min cap), bounded by operator-configured `ttl` and `max_reference_deviation`. On TTL or deviation breach: terminal `needs_operator_rearm`. | Wait under the same TTL; repeated data failures escalate. |
| Strategy modes (`ConfirmedTrend`, …) | The stale trigger is **not** reused: each retry requires a fresh entry condition (`SignalConfirmed`) before stop analysis runs again. | Same. |

Resolution requirements for any deferred entry (`ImmediateUntilValid`):
- Re-pin the entry reference to the current market before emitting the signal; recompute
  span against it.
- Revalidate side, distance, quantity, and planned risk with fresh price and exchange
  metadata immediately before the governed action.
- Audit `entry_reference_at_arm` vs `entry_reference_at_resolution`.

Disarm always wins: it invalidates the position's `detector_generation`, so late
analysis results cannot fire.

### 3. Sizing: executable worst-case cost, 1% is a ceiling

This section normalizes, into one admission-time formula, the cost model that ADR-0041
already states (`worst loss per unit = stop_distance + stop_buffer + gap_allowance +
round_trip_fees`) and that ADR-0039's Policy-10 note already requires. It **supersedes
the sizing formula in ADR-0021 invariant 5** (`qty = capital × 1% / distance`):

```text
worst_case_loss_per_unit =
    |entry − executable_stop|        # tick-quantized, includes the ADR-0041 buffer (§4)
    + gap_allowance                  # configured, may be zero
    + entry_fee_per_unit + exit_fee_per_unit

qty = quantize_down(
    min( (capital_base × 1%) / worst_case_loss_per_unit,
         margin_cap_qty ),           # 1x: available margin / entry price (ADR-0024)
    exchange_lot_step
)

planned_risk = qty × worst_case_loss_per_unit    # ≤ capital_base × 1%, may be lower
```

1% is a **ceiling, not a target**. ADR-0043 debits `planned_risk` (the real number),
not the full cap. At 1x leverage, tight spans are margin-bound: the margin cap binds
before the risk cap and planned risk lands well under the ceiling. Deeper anchors can
*raise* planned risk toward (never past) the ceiling when quantization or the margin
cap had been binding; the ceiling is enforced at admission either way.

### 4. The executable stop buffer becomes span-capped (amends ADR-0041)

The execution offset observed in production (0.1% beyond the trailing stop) is not an
ADR constant: it is the operator-configured **ADR-0041 buffer** (`stop_buffer_bps`, env
`ROBSON_STOP_BUFFER_BPS`, default 0; production currently runs 10 bps). ADR-0041
requires the software monitor and the insurance stop to trigger at the **same** buffered
executable price; this ADR preserves that single-price invariant and amends only the
buffer's magnitude:

```text
effective_buffer = min( configured_stop_buffer, 0.25 × span )
```

Rationale: a buffer that is large relative to the span dominates the loss on the
buffer-triggered path. Measured against the **span-only** loss baseline (the pre-§3
mental model), a 0.1% buffer at a 0.1% span means the executable path realizes ~2×
the span loss; the 0.25 × span cap bounds that same ratio at 1.25× for every span.
Under §3 this is not an overrun — the buffer is priced into `worst_case_loss_per_unit`,
so `planned_risk` already includes it and the 1% ceiling holds regardless. The cap
exists to stop the buffer from silently consuming the risk budget that should be
buying stop distance.

ADR-0041's semantics (chart level untouched, buffer is execution-only, zero-default,
same price for both stop layers, events store raw values) are otherwise unchanged.

### 5. One source of truth for bounds

Today the bounds exist in three incompatible representations (fractions in
`TechnicalStopConfig`, percentage points in `TechnicalStopDistance.distance_pct`,
hard-coded 0.1%–10% in `validate`), and `DetectorConfig::from_position_with_policy`
always constructs `TechnicalStopConfig::default()`, so daemon configuration never
reaches the analyzer (tracked as its own bug, issue #148).

This ADR institutes a single typed `StopDistanceBounds` (basis points) in
`robson-domain`, validated at startup and injected into both the analyzer and sizing.
The spec distinguishes four validity layers, in order:

1. raw technical level (this ADR §1) — validated at selection;
2. guard-aware stop (ADR-0042 invalidation guard applied) — a `guard_too_wide`
   outcome maps to `RetryableNoValidStop`, never to inert Armed;
3. executable stop (tick-quantized, §4 buffer applied) — may exceed the maximum
   even when the raw level was in bounds; this yields the explicit outcome
   `executable_stop_out_of_bounds`, which also maps to `RetryableNoValidStop`
   (walk-deeper-only means selection never retreats to a shallower level to
   compensate);
4. planned cost (§3), validated with fresh price immediately before the order —
   a violation here is an admission denial, audited with the same reason codes.

Every stage re-checks against the same `StopDistanceBounds` instance; a level that
passes stage 1 is not assumed valid at stages 2–4.

### 6. Durability, atomicity, idempotency

- Durable events: `TechnicalStopSelectionRejected`, `TechnicalStopRetryScheduled`
  (with attempt counter and `next_retry_at`). Rejected analyses emit durable audit;
  SSE is presentation, never evidence.
- Projection: `entry_status`, `last_rejection`, `next_retry_at`, `detector_generation`.
- The waiting/terminal transition is persisted **before** scheduling any retry; startup
  rehydrates the deadline and exactly one detector generation, and reconciles with the
  exchange (no entry order, fill, or position may exist) before resuming a wait.
- Event-schema additions ship with `serde(default)`/upcasting so historical replay
  is unaffected.
- Arm HTTP contract: `Idempotency-Key` required; 422 only for failures **before**
  `PositionArmed` is persisted; once persisted, the response is 201/202 with the id
  and current `entry_status`.

### 7. Operator surfacing

- SSE `entry_rejected` events are deduplicated: emit on change of
  `(code, numbers)` only, with a periodic heartbeat while a waiting state persists.
- The Armed representation exposes `entry_status` and `last_rejection` so the FE
  renders "waiting for valid stop (nearest level 63,551.40 at 0.094%; min 0.1%)"
  instead of an error, and `needs_operator_rearm` as an actionable state.
- HTTP 500 is reserved for infrastructure failure.

---

## Invariants (restated and extended)

1. Stops are chart-derived levels (or documented ATR fallback) — never a percentage
   of entry (ADR-0021, unchanged).
2. The raw level's distance is validated at selection; the guard-aware, executable,
   and planned-cost distances are validated at their own stages before admission —
   all four against the same single bounds source. No stage inherits validity from
   an earlier stage.
3. `planned_risk ≤ capital_base × 1%`, where planned risk prices the executable stop,
   insurance offset, gap allowance, and fees. The cap is enforced at admission with
   fresh market data.
4. In `Immediate` mode the operator decides WHEN; the system never converts a rejected
   immediate entry into an autonomous later entry. Deferred entry exists only under an
   explicit operator-configured policy with TTL and deviation bounds.
5. An Armed position always has exactly one of: a live (single-generation) detector, a
   durable waiting state with a deadline, or a terminal state — including across
   restarts. Inert Armed is a defect.
6. Every selection decision (anchor, skips, fallback inputs) is durably audited.

---

## Consequences

### Worked example (corrected; the real incident)

SHORT BTCUSDT, entry reference 63,491.70, capital base 1,558.99 USDT, 1x, cap 15.59 USDT.

- Pre-ADR: level 2 = 63,551.40 (0.094%) → rejected → Armed inert + opaque 500.
- Post-ADR §1: level 2 too tight (audited skip) → walk deeper; suppose level 3 =
  63,650.00 → raw span 158.30 (0.249%), in bounds.
- §4 buffer: configured 10 bps gives `0.1% × 63,491.70 ≈ 63.49`; span cap gives
  `0.25 × 158.30 ≈ 39.58`; effective buffer = 39.58.
- §3 worst-case per unit (illustrative, fees 0.05% per side, no gap allowance):
  `158.30 + 39.58 + 63.57 ≈ 261.45` → risk-derived qty ≈ 15.59 / 261.45 × ... ≈ 0.0596 BTC.
- Margin cap at 1x: ≈ 1,558.99 / 63,491.70 ≈ **0.0245 BTC** — the binding constraint.
- `qty = quantize_down(min(0.0596, 0.0245), lot)` ≈ 0.024 BTC;
  `planned_risk ≈ 0.024 × 261.45 ≈ 6.27 USDT` (≈ 0.40% of capital, under the 1% ceiling).

The margin cap dominating at 1x is expected and honest: the ceiling protects; it is not
a quota to be filled.

### Testing obligations

- Regression fixture from the incident: anchor-N walk selects the deeper level; skips
  audited; sizing respects margin cap and lot step.
- All-levels-too-tight → ATR; nothing valid → per-policy table outcomes, including
  restart rehydration, TTL expiry, deviation breach, disarm-vs-late-result race
  (generation invalidation), and idempotent arm retries.
- Property tests over interleavings (disarm, retry timer, OHLCV completion) proving
  "at most one live detector/signal per position" and budget serialization when
  multiple waiting positions resolve on the same candle.
- OHLCV staleness/incomplete-candle guard: stale data yields `DataUnavailable`,
  never a stop.

---

## Alternatives Rejected

- **Closest-in-bounds selection** (v1): silently overrides the configured anchor;
  biases toward minimum spans where offset, slippage, and gap risk compound.
- **Detector-alive-until-valid as default** (v1): autonomous WHEN; rejected on
  product philosophy. Kept only as the explicit `ImmediateUntilValid` opt-in.
- **Auto-cancel on first rejection**: converts a transient market condition into a
  revoked operator decision without escalation; replaced by the durable
  `needs_operator_rearm` terminal state with alerting.
- **Synthetic clamp to 0.1%**: a percentage stop in costume; ADR-0021 prohibition
  stands.
- **Status quo strict-fail**: provably discards valid deeper levels and leaves inert
  Armed residue.

---

## References

- `robson-engine/src/technical_stop_analyzer.rs`, `robson-domain/src/value_objects.rs`
- ADR-0021, ADR-0024, ADR-0039, ADR-0041, ADR-0042, ADR-0043, ADR-0048
- Issue #147 (incident, reviews, acceptance); issue #148 (bounds/config plumbing bug)
- Reviews: Codex gpt-5.6-sol and GLM 5.2, 2026-08-02 (archived on #147)
