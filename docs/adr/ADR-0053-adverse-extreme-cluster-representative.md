# ADR-0053: Adverse-Extreme Cluster Representative for the Technical Stop

Status: Proposed (implementation PR open; operator review pending)
Date: 2026-08-13
Related: issue #173, ADR-0021, ADR-0041, ADR-0050, ADR-0052, REQ-CORE-TECHSTOP-001

## Context

The technical stop analyzer (`robson-engine/src/technical_stop_analyzer.rs`)
merges nearby swing levels into clusters before counting the N-th
support/resistance level (`level_tolerance`, default 0.5% of entry). Since its
introduction in 04efa813 (2026-04-15), the cluster representative has been the
**mean** of the members.

REQ-CORE-TECHSTOP-001 never specified a mean. It specifies a *level*:

> For LONG: Stop **below** the 2nd support level on the timeframe
> For SHORT: Stop **above** the 2nd resistance level on the timeframe

A mean of clustered swing points is not a level on the chart. For stop
placement it is systematically biased toward entry: the representative sits at
the statistical center of the zone, so roughly half of the zone's historical
touches exceeded it. The ADR-0041 buffer then offsets *beyond the mean*, which
can still be *inside the zone*.

### Production incident (2026-08-13, position 019ffb62)

BTCUSDT Short, entry 63794.30. Seven swing highs (63800.0 through 64131.4)
merged into a single resistance cluster; the mean representative put the
technical stop at 63927.78 and the buffered insurance stop at 63967.60. Price
probed the zone, topping at 63980.6 — the 63990.7 swing high never broke — and
the position stopped out at 63967.90. The operator had flagged the stop as
"short of the technical event" before the stop-out. A representative at the
zone's adverse extreme (64131.4) would have kept the stop clear of the entire
probe.

### Companion failure mode (2026-08-12, position 019ff820)

BTCUSDT Long, entry 63356.40. Four swing lows merged into one cluster whose
mean (63312.6) fell 0.069% from entry — below the ADR-0050 minimum distance —
so the chart level was skipped and the stop silently degraded to the ATR
fallback. The adverse extreme (63283.0, 0.116% from entry) is in bounds: the
mean did not only misplace stops, it also destroyed valid chart levels.

## Decision

1. **Cluster membership is unchanged**: levels within `level_tolerance` of a
   cluster's running mean merge into that cluster. The mean remains the right
   statistic for *grouping* (geometric center of the zone).
2. **The cluster representative is the adverse extreme of its members**: the
   minimum for `Long` (deepest support), the maximum for `Short` (highest
   resistance). Selection, bounds-walking (ADR-0050 §1), the executable stop
   plan (ADR-0041/0052), and sizing consume this representative unchanged.
3. **Audit**: `TechnicalStopAnalysis` gains a `cluster_representative` field
   (`adverse_extreme` | `mean`), serde-defaulted to `mean` so historical
   payloads deserialize truthfully. Current analysis always emits
   `adverse_extreme`.

## Consequences

- Stops anchor beyond the whole support/resistance zone; the ADR-0041 buffer
  recovers its documented meaning (an execution offset beyond the technical
  event, not a partial compensation for a biased representative).
- Wider stop distances where zones are wide: under the 1% rule
  (REQ-CORE-TECHSTOP-002) the same risk buys a smaller quantity. This is the
  intended trade: the stop measures thesis invalidation, not zone noise.
- The 2026-08-12 class of BelowMin degradations to ATR shrinks, because the
  adverse extreme is farther from entry than the mean by construction.
- `level_tolerance = 0.5%` still collapses most 15m structure into a single
  cluster, so `support_level_n = 2` rarely finds a second cluster. Tolerance
  retuning (0.1-0.2% for 15m) is a **follow-up decision**, deliberately out of
  scope here: it changes cluster geometry for every arm and deserves its own
  evidence window on top of this fix.

## Alternatives rejected

- **Keep the mean, widen the buffer**: the buffer is capped at 0.25 x the
  cap-basis distance (ADR-0052) and is an execution offset by definition;
  using it to compensate a biased anchor conflates two concerns and still
  fails for wide zones.
- **Median representative**: still a statistical center; the 2026-08-13
  incident would have played out identically.
- **Tolerance reduction alone**: with a small tolerance the nearest thin
  cluster becomes the anchor, but each cluster's representative would still be
  its mean; probes of that thin level would still fill inside it.

## Test evidence

`robson-engine/src/technical_stop_analyzer.rs` test module:

- `long_cluster_representative_is_the_minimum_member`,
  `short_cluster_representative_is_the_maximum_member` — unit contract.
- `replay_2026_08_13_short_stop_anchors_beyond_the_whole_resistance_zone` —
  the incident fixture; asserts the stop clears the swing high the probe never
  broke.
- `replay_2026_08_12_long_adverse_extreme_rescues_the_chart_level_from_atr` —
  the companion fixture; asserts the chart level carries the stop instead of
  the ATR fallback.
