# ADR-0042: Invalidation Guard

Status: Accepted (operator-initiated 2026-07-03)
Date: 2026-07-03

## Context

The technical stop is the chart-derived invalidation level (second S/R on the
15-minute chart, ADR-0021). A real BTCUSDT short exposed a gap: entry
61,909.10 → technical stop 62,214.70, while a recent high at **62,386.70**
sat *above* the chosen stop. The analyzer never used 62,386.70 because swing
detection excludes recent/unconfirmed candles and the rule deliberately skips
the first resistance. Result: the stop sat below a recent high, so a mere
retest of 62,386.70 would have stopped out a still-valid trade.

The executable buffer (ADR-0041) does **not** fix this: it offsets execution
from *whichever level was chosen*, and the chosen technical level (62,214.70)
was below the recent high. Bumping the technical model's support/resistance
index was rejected because it changes the chart model globally and
unpredictably.

## Decision

Add an opt-in, configurable **invalidation guard** as a distinct post-analysis
layer that clamps the effective stop beyond a recent adverse extreme (recent
high for shorts, recent low for longs). The guard is applied **after** the
technical analyzer and **before** the ADR-0041 buffer.

Layered model, all distinct and audited:

| Layer | Meaning | Source |
|---|---|---|
| `technical_stop` | Analyzer output (2nd S/R) — **unchanged** | `TechnicalStopAnalyzer` |
| `invalidation_guard_level` | Recent high (short) / low (long) over last N 15m candles | New detector helper |
| `effective_stop` | `clamp(technical, guard)` then ± ADR-0041 buffer | `effective_stop_price_with_guard` |
| `buffer` | Bps offset on the clamped level | ADR-0041, unchanged |

Core semantics: `value_objects::effective_stop_price_with_guard` first clamps
the technical stop to the guard (when the guard is on the adverse side of
entry), then applies the existing ADR-0041 buffer to that clamped base.
`effective_stop_price` remains as the zero-guard delegate, so when the guard
is disabled behavior is byte-for-byte identical to today.

Configuration (opt-in, default off):

- `ROBSON_STOP_INVALIDATION_GUARD_ENABLED` — default `false`.
- `ROBSON_STOP_INVALIDATION_LOOKBACK_CANDLES` — default `20` (5 hours on the
  15-minute chart). The lookback **includes the forming candle** and is
  sampled **once at signal time**; no ongoing repaint.
- Buffer reuse: `ROBSON_STOP_BUFFER_BPS` from ADR-0041 is applied to the
  clamped base.

Lifecycle:

- **Entry-time only.** The guard travels in `PositionState::Active`
  (`invalidation_guard_level`) and in the `EntryFilled` event so replay and
  crash recovery reproduce the same effective stop.
- **Release on first trailing-stop advance.** For a short the trailing
  technical stop only moves down; a permanent guard would pin the stop at the
  entry high forever and block tightening. After the first advance the guard
  is cleared and effective = trailing technical + buffer (ADR-0041 unchanged).
- **Guard-too-wide rejects entry.** If the clamped effective distance exceeds
  `max_tech_stop_percent`, `calculate_position_size` returns an error and the
  entry is rejected with an explicit audit reason. This is coherent with the
  existing distance-validation philosophy and is safer than capping below the
  recent extreme.

Risk model: sizing uses the clamped effective distance as the denominator so
the 1% worst-case cap still holds. The guard widens the realizable loss when
it binds, so it is priced into the budget alongside the buffer, gap
allowance, and round-trip fees (Policy 10).

## Consequences

Positive: the stop no longer sits below an obvious recent adverse extreme;
both stop layers (soft monitor and exchange-side insurance) trigger at the
same guard-aware executable price; the default-off setting keeps rollout
risk-free; the technical stop rule itself is unchanged.

Trade-offs: when the guard binds, position size is smaller (wider effective
stop); a binding guard that later releases gives up the extra protection by
design, because trailing must be free to tighten; consumers must derive the
effective stop through the single helper or read the API field.

## Alternatives

- Bake the guard into `TechnicalStopAnalyzer` by increasing the resistance
  index: rejected — it changes the chart model globally and unpredictably.
- Keep the guard permanently active after entry: rejected — for shorts it
  would prevent the trailing stop from ever tightening past the entry high.
- Cap position size instead of rejecting when the guard is too wide: rejected
  — silently accepting a level beyond the validated maximum distance would
  violate the distance-validation policy.

## Implementation Notes

`robson-domain`: `effective_stop_price_with_guard`, `EntryFilled`
`invalidation_guard_level`, `PositionState::Active::invalidation_guard_level`.
`robson-engine`: `Engine::effective_stop` threads the guard; sizing uses the
clamped effective distance; the first trailing advance clears the guard.
`robsond`: detector computes the recent extreme, env wiring, startup-recovery
heal derives the guard-aware executable stop.
`robson-projector`: migration adds `invalidation_guard_level` to
`positions_current`; `handle_entry_filled` sets it; `handle_trailing_stop_updated`
clears it. `robson-store`: hydrates the guard into `Active` on recovery.
Tests cover short/long clamping, guard no-op when on the favorable side,
zero-buffer clamp, disabled identity, persistence round-trip, and release on
the first trailing advance.
