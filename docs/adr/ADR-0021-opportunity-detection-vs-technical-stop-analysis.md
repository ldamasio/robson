# ADR-0021 — Separation of Opportunity Detection and Technical Stop Analysis

**Date**: 2026-04-15
**Status**: DECIDED — IMPLEMENTATION GAP RESOLVED (2026-04-15): `TechnicalStopAnalyzer` implemented; detector now emits chart-derived stops. VAL-001 Phase 1 PASS. The later VAL-001 Phase 2 static exposure blocker was resolved by ADR-0024 / MIG-v3#11 on 2026-04-19; Phase 2 is now pending testnet redeploy and execution.
**Deciders**: RBX Systems (operator + architecture)

---

## Context

The current `DetectorTask` in `v2/robsond/src/detector.rs` conflates two distinct
responsibilities into a single component:

1. Detecting WHEN to enter a position (MA crossover on market data)
2. Determining WHERE the stop is (computed as `entry × (1 − 0.02)`)

This conflation produces a policy violation: `TechnicalStopDistance` is computed from a
fixed percentage (`stop_loss_percent = dec!(0.02)`) instead of from chart analysis
(second support/resistance on the 15-minute chart). The `stop_loss_percent` field in
`DetectorConfig` must not exist — percentage-based stops are explicitly prohibited by
`REQ-CORE-TECHSTOP-001`.

The domain model is correct: `DetectorSignal.stop_loss: Price` expects an absolute price
level from chart analysis. The implementation does not fulfill this contract.

---

## Decision

These two responsibilities are **architecturally separate** and must never be conflated:

### Responsibility 1 — Opportunity Detection

**Question answered**: Is there a valid entry condition right now?

**Input**: Market data stream (price ticks, OHLCV)
**Logic**: Configurable detection strategy (MA crossover, pattern recognition, etc.)
**Output**: Entry trigger — "entry condition exists at price X"

The Opportunity Detector does NOT know or care about stops or position sizing.
It answers only: is this a good moment to enter?

### Responsibility 2 — Technical Stop Analysis

**Question answered**: Where is the technical invalidation level for this trade?

**Input**: OHLCV data (15-minute timeframe, ≥100 candles), entry price, side
**Logic**: Identifies second support (for LONG) or second resistance (for SHORT)
  on the 15-minute chart. Fallback chain per REQ-CORE-TECHSTOP-001:
  1. Second support/resistance level (primary)
  2. Recent swing low/high
  3. ATR-based stop (1.5× ATR, fallback only)
**Output**: `TechnicalStopDistance` — an absolute stop price from which position size
  is calculated as `(capital × 1%) / span`

The Technical Stop Analyzer does NOT make entry decisions.
It answers only: if we enter here, where does the thesis fail?

---

## The Complete Signal

A `DetectorSignal` is valid only when BOTH responsibilities have completed:

```
Opportunity Detector fires (entry condition met)
        +
Technical Stop Analyzer computes stop level (from chart analysis)
        =
DetectorSignal {
    entry_price: Price,       // from Opportunity Detector
    stop_loss:   Price,       // from Technical Stop Analyzer — CHART-DERIVED PRICE LEVEL
}
```

Neither alone is sufficient. An entry without a technical stop is a gambling act, not
a governed trade.

---

## Invariants (non-negotiable)

These invariants apply to every `DetectorSignal` produced by the system, without exception:

1. `stop_loss` MUST be a price level derived from chart analysis — NEVER a percentage of entry
2. `stop_loss` MUST be on the correct side: below entry for LONG, above entry for SHORT
3. `stop_loss` MUST be within 0.1%–10% of entry (enforced at engine layer)
4. `TechnicalStopDistance.distance` MUST be positive
5. Position size MUST be computed as `(capital × 1%) / TechnicalStopDistance.distance`
6. No `stop_loss_percent` field may exist in any configuration that feeds the signal path

---

## Consequences

### Immediate (implementation gap — RESOLVED 2026-04-15)

`v2/robsond/src/detector.rs` previously violated this ADR. Resolved:
- `DetectorConfig.stop_loss_percent` removed
- `calculate_stop_loss()` (percentage-based) replaced
- `TechnicalStopAnalyzer` implemented: fetches OHLCV data and performs chart analysis
- VAL-001 Run Log (2026-04-15) confirms detector emits chart-derived stops

VAL-001 Phase 1 now PASS. The Phase 2 exposure-limit blocker was not caused by
this ADR and has been resolved by ADR-0024 / MIG-v3#11. Phase 2 remains pending
testnet redeploy and execution.

### Architecture going forward

- `DetectorTask` is responsible for Responsibility 1 only (entry conditions)
- A `TechnicalStopAnalyzer` component (to be implemented) handles Responsibility 2
- `DetectorTask` calls `TechnicalStopAnalyzer` before emitting `DetectorSignal`
- `TechnicalStopAnalyzer` requires historical OHLCV data from Binance REST
  (15-minute candles, ≥100 periods)

### What changes in signal injection (testing)

The `POST /positions/:id/signal` endpoint used for testing must also respect these
invariants. When injecting a signal for testing, `stop_loss` must be a technically
meaningful price level — it may be a known historical support level, not a
computed percentage.

---

## Alternatives Rejected

### Alternative: Keep percentage stop, tune the percentage

**Rejected**: A percentage stop is not a technical stop. It does not respect market
structure. A 2% stop during low volatility may be hit by noise; a 2% stop during high
volatility may be too tight. The entire value proposition of Robson is that position
sizing derives from market structure, not from an arbitrary number.

### Alternative: Use ATR as the only method

**Rejected**: ATR is a fallback, not the primary method. Support/resistance levels
derived from price action are the primary method per REQ-CORE-TECHSTOP-001.

---

## References

- `docs/requirements/technical-stop-requirements.md` — full policy specification
- `docs/specs/TECHNICAL-STOP-RULE.md` — technical stop rule documentation
- `v2/robson-domain/src/value_objects.rs` — `TechnicalStopDistance` implementation
- `v2/robson-domain/src/entities.rs` — `DetectorSignal` domain type
- `v2/robsond/src/detector.rs` — current implementation (violates this ADR)
- `docs/runbooks/val-001-testnet-e2e-validation.md` — Phase 1 PASS; see Run Log for current Phase 2 status
