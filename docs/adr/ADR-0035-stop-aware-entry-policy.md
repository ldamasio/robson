# ADR-0035 — Stop-Aware Entry Policy (v4)

**Date**: 2026-04-28
**Status**: PROPOSED
**Deciders**: RBX Systems (operator + architecture)

---

## Context

Robson v3 calculates `TechnicalStopDistance` from a technical event on the 15m chart
(e.g., support, resistance, swing low/high). The event used as the stop anchor exists
implicitly in the calculation but is not explicit metadata. The quality of the stop
region is not classified or exposed for observation.

This creates two gaps:

1. **Observability**: The stop anchor event is not recorded, making it impossible to
   audit why a specific stop distance was chosen or to invalidate it when the anchor
   breaks.

2. **Signal quality modulation**: A entry signal with a high-quality stop (e.g., recent
   support with clear invalidation) is treated identically to a signal with a weak stop
   (e.g., distant anchor with poor structure), assuming both pass the same distance check.

The Risk Engine remains the final authority on position authorization, but the EntryPolicy
layer lacks a mechanism to modulate signals based on stop quality without altering the
core `TechnicalStopDistance` calculation.

---

## Decision

Introduce **StopAnchor** as explicit metadata and **StopQuality** as a capped, additive
boost to entry signal evaluation. Preserve `TechnicalStopDistance` behavior unchanged.

### StopAnchor (new explicit metadata)

The technical event used as the basis for `TechnicalStopDistance` becomes explicit:

```rust
struct StopAnchor {
    anchor_type: AnchorType,        // support|resistance|swing_low|swing_high|
                                     // breakout_retest|liquidity_level
    anchor_price: Price,
    timeframe: Timeframe,           // 15m in v3
    source_event_id: EventId,       // reference to the technical event
    invalidation_reason: Option<InvalidationReason>,
}
```

### StopQuality (new classification)

Classify the quality of the stop region independent of distance:

| Class      | Boost | Description                                                                 |
|------------|-------|-----------------------------------------------------------------------------|
| None       | 0%    | Valid anchor, no structural advantage                                      |
| Weak       | +5%   | Valid but distant or old anchor, low confluence                            |
| Good       | +10%  | Recent anchor, reasonable distance, clean 15m structure                    |
| Premium    | +15%  | Recent + clean anchor, well-defined support/resistance or swing, good ATR  |
| Exceptional| +20%  | Rare: strong confluence, efficient distance, clear invalidation, liquidity sweep/retest (feature-flagged) |

### EntryCandidateEvaluation (new v4 contract)

```rust
struct EntryCandidateEvaluation {
    base_score: Score,                  // from EntryPolicy (may not exist in v3)
    stop_quality_class: StopQuality,
    stop_quality_boost: Boost,
    boosted_score: Score,               // base_score + stop_quality_boost
    rejection_reasons: Vec<RejectionReason>,
    shadow_scores: Option<ShadowScores>, // +15% vs +20% comparison
}
```

**Note**: Concrete Rust types may differ from this ADR sketch, but the semantic contract
must be preserved. If v3 does not have a formal `EntryScore`, this is a new v4 layer.

### Behavioral Rules

1. **`TechnicalStopDistance`**: calculation remains behaviorally unchanged.
2. **`boosted_score` is INPUT to Risk Engine**: `boosted_score` does NOT imply authorization.
   The Risk Engine evaluates the boosted candidate and may still reject based on slots,
   exposure, drawdown, correlation, or other limits.
3. **StopQualityBoost**: additive only, capped at +15% in production (+20% feature-flagged).
4. **No boost is not rejection**: absence of `StopQualityBoost` must not create a rejection.
5. **v3 live positions**: read-only, authoritative. v4 must not revalidate, cancel, or
   reclassify existing v3 positions. This does NOT prevent v3 from receiving new features
   — it only protects existing live state from revalidation.
6. **Risk Engine authority**: Risk Engine rejection always wins.
7. **Shadow mode**: must not affect execution decisions (telemetry only).

### StopAnchor Invalidation by Phase

| Phase              | Behavior                                                        |
|--------------------|-----------------------------------------------------------------|
| **Candidate**      | Invalid anchor rejects or expires candidate with explicit reason |
| **Open Position**  | Anchor invalidation becomes risk/exit event. This does NOT rewrite |
|                    | the original entry thesis, nor does it revalidate the v3 or v4 entry |
|                    | decision. It is purely a position management signal.              |

### Rollout Phases

| Phase | Description                                                          |
|-------|----------------------------------------------------------------------|
| 0     | ADR only — document decision, boundaries, invariants                 |
| 1     | Shadow metadata — emit `StopAnchor` and `StopQuality`, no decision change |
| 2     | Telemetry — log `base_score`, `stop_quality_class`, `boosted_score` (hypothetical), `decision_delta` |
| 3     | Boost cap +15% — apply boost up to Premium, keep +20% disabled       |
| 4     | Exceptional evaluation — run +20% in shadow mode, compare impact    |
| 5     | Feature-flagged +20% — enable only if operational evidence supports  |

### Rejected Alternatives

- **Replace `TechnicalStopDistance` with StopAnchor.** Rejected — the existing calculation
  is battle-tested; StopAnchor is metadata only.
- **StopQuality as hard filter.** Rejected — would reject entries that v3 would accept,
  violating the no-regression rule.
- **Uncapped boost.** Rejected — risk of over-leveraging on weak signals with "good" stops.
- **ML-based classification.** Rejected — adds opacity and calibration risk before telemetry
  exists; start rule-based.
- **v4 revalidates v3 positions.** Rejected — existing positions are authoritative live state.

---

## Consequences

### Positive

- Stop anchor becomes observable and auditable.
- Entry signals are modulated by stop quality without altering distance calculation.
- Telemetry enables data-driven evolution from rule-based to ML-based classification.
- Shadow mode allows safe validation before production decisions.
- v3 positions remain untouched — zero production disruption.

### Negative / Trade-offs

- Implementation cost: new metadata layer, classification logic, telemetry.
- Shadow mode requires storage for hypothetical scores without affecting decisions.
- Rule-based classification is initially subjective; requires calibration.
- +20% exceptional boost requires operational evidence before release.

### Operational

- Existing v3 production slots remain occupied and read-only.
- v4 evaluates only new candidates.
- Risk Engine continues as final authority.
- Feature flags: `STOP_QUALITY_BOOST_ENABLED` (Phase 3), `STOP_QUALITY_EXCEPTIONAL_ENABLED` (Phase 5).

---

## Implementation Notes

**Pre-implementation discovery** (required before coding):

The agent must first locate in the current codebase:

1. Where `TechnicalStopDistance` is calculated
2. Whether `EntryScore` or equivalent exists in v3
3. How `EntryPolicy` generates candidates
4. Where `RiskEngine` rejects/authorizes entries
5. How current slots are read and enforced
6. How v3 live positions appear in state

**Then**, create a separate Implementation Guide for safe execution.

**Follow-up work** (tracked as `MIG-v4#TBD — Stop-Aware Entry`):

1. **StopAnchor emission**: extend event/metadata schema to include anchor metadata
2. **StopQuality classifier**: rule-based implementation using heuristics (separate spec)
3. **Shadow mode logging**: emit hypothetical boosted scores without applying them
4. **Telemetry pipeline**: aggregate `base_score` vs `boosted_score` deltas
5. **Feature flags**: add `STOP_QUALITY_BOOST_ENABLED`, `STOP_QUALITY_EXCEPTIONAL_ENABLED`
6. **EntryCandidateEvaluation**: if v3 lacks `EntryScore`, create as new v4 layer
7. **VAL-001 scenario**: test shadow mode on testnet, confirm zero decision impact
8. **Phase 3 validation**: enable +15% boost, compare decision distribution vs shadow data

---

## Invariants (Non-Negotiable)

1. `TechnicalStopDistance` calculation must remain behaviorally unchanged.
2. `StopQuality` absence must not create rejection.
3. `StopQualityBoost` must be additive only (never subtractive).
4. Existing v3 production positions must never be revalidated by v4 `EntryPolicy`.
5. `StopQualityBoost` must not rescue an otherwise invalid signal.
6. `RiskEngine` rejection always wins.
7. `boosted_score` is NOT authorization; it is INPUT to Risk Engine only.
8. Shadow mode must not affect execution decisions.
9. Exceptional +20% boost must be disabled by default.
10. All boosted decisions must log: `base_score`, `boost_class`, `boost_pct`, `boosted_score`, `decision_delta`.
11. `StopAnchor` invalidation on open positions MUST NOT rewrite the original entry thesis.

---

## Related Components

- `robsond/src/entry_policy.rs` (or equivalent) — candidate generation
- `robsond/src/technical_stop.rs` (to be located) — `TechnicalStopDistance` calculation
- `robsond/src/risk_engine.rs` — final authority
- `robson-eventlog/` — `StopAnchor` metadata schema
- `robsond/src/position_manager.rs` — v3 live position read-only state

---

## References

- [ADR-0021 — Opportunity Detection vs Technical Stop Analysis](ADR-0021-opportunity-detection-vs-technical-stop-analysis.md)
- [ADR-0022 — Robson-Authored Position Invariant](ADR-0022-robson-authored-position-invariant.md)
- [docs/architecture/v3-runtime-spec.md](../architecture/v3-runtime-spec.md)
- [docs/architecture/v3-risk-engine-spec.md](../architecture/v3-risk-engine-spec.md)
