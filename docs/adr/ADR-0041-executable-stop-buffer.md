# ADR-0041: Executable Stop Buffer

Status: Accepted (operator-initiated 2026-07-03)
Date: 2026-07-03

## Context

The technical stop is the chart-derived invalidation level (second S/R on the
15-minute chart, ADR-0021). Until now it was also the exact execution level:
the soft monitor compared prices against it and the insurance stop (ADR-0039)
was placed exactly on it. Obvious chart levels attract liquidity: wicks that
touch the level and revert can stop out a still-valid trade at the precise
price everyone is watching.

## Decision

Separate the two meanings:

- **Technical stop** — the conceptual and operational reference. Computed
  exactly as before; drives the trailing ladder, sizing distance, domain
  events, persistence, and validations. Unchanged.
- **Executable stop** — where execution actually triggers: the technical
  stop offset by an operator-configured buffer, BELOW it for longs and ABOVE
  it for shorts. Derived, never stored:
  `value_objects::effective_stop_price(side, technical_stop, stop_buffer_bps)`
  is the single source of truth, used by the soft-exit comparison, insurance
  stop placement/replacement, and the startup-recovery heal (which must
  compare open orders against the same derivation or it would replace a
  correctly-priced stop on every restart).

Configuration: `stop_buffer_bps` on `RiskConfig`
(`RiskConfig::with_stop_buffer`, env `ROBSON_STOP_BUFFER_BPS`, basis points
of the stop price, range 0 to 100). **Default 0**: behavior identical to the
pre-ADR system; the buffer is opt-in.

Risk model: the buffer widens the worst realizable distance, so
`calculate_position_size` prices it into the 1% budget alongside the gap
allowance and round-trip fees (Policy 10 stays a worst-case cap):

```text
worst loss per unit = stop_distance + stop_buffer + gap_allowance + round_trip_fees
```

The buffer is an execution offset from the chart level, never a
percentage-of-entry stop — ADR-0021/Rule 6 intact. It is distinct from
`stop_gap_bps` (expected trigger-to-fill slippage, a sizing-only allowance):
the buffer moves WHERE execution triggers; the gap prices HOW FAR past the
trigger a fill may land.

API: `PositionSummary.effective_stop` exposes the derived value next to
`trailing_stop` (the technical value) for Active positions.

## Consequences

Positive: execution no longer sits on the obvious level; both stop layers
(soft and exchange-side) trigger at the same buffered price; zero-default
keeps rollout risk-free.

Trade-offs: a non-zero buffer slightly widens realized losses when the stop
does fill (priced into sizing, so the cap holds); events keep technical
values, so consumers wanting the executable price must derive it or read the
API field.

## Alternatives

- Bake the buffer into `TechnicalStopDistance`: rejected — pollutes the
  conceptual reference, the trailing ladder, and audit history.
- Buffer only the exchange-side stop: rejected — the two layers would
  trigger at different prices, reintroducing execution ambiguity.

## Implementation Notes

`robson-domain`: `effective_stop_price`, `RiskConfig.stop_buffer_bps` +
builder + sizing term. `robson-engine`: `Engine::effective_stop` used by
`should_exit` and all insurance placements. `robsond`: env wiring,
startup-recovery heal, `PositionSummary.effective_stop`. Tests cover long,
short, zero buffer, positive buffer, sizing-budget invariance, and decimal
precision; exchange tick normalization remains the existing exchangeInfo
TODO in the connector.
