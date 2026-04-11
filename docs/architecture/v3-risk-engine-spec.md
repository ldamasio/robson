# ROBSON v3 — RISK ENGINE SPECIFICATION

**Date**: 2026-04-11
**Status**: APPROVED (revised — replaces previous spec with L1–L4 escalation)
**Classification**: CRITICAL PATH — Financial Safety Component

---

## Role

The Risk Engine is the single component where a bug means financial loss.
It is a mandatory blocking gate. No action proceeds without Risk Engine clearance.

---

## v3 Policy Decisions (Closed)

These decisions are final for v3. No alternatives, no overrides, no flexibility.

### 1. Risk Per Trade: Fixed 1%

- Every position risks exactly **1% of total capital**.
- Position size is derived from the technical stop distance (Golden Rule).
- Formula: `position_size = (capital × 0.01) / stop_distance`
- This is not configurable. There is no environment variable, API parameter, or mode selector.

### 2. Monthly Drawdown: 4% Hard Halt

- Maximum monthly loss: **4% of capital**.
- When reached:
  - Close all open positions immediately.
  - Block all new entries.
  - System enters `MonthlyHalt` state.
- No gradual escalation (no L1/L2/L3/L4).
- No dynamic adjustment based on market conditions.
- Resume: next calendar month or manual operator reset.

### 3. Daily Loss Limit: 3%

- Maximum daily loss: **3% of capital**.
- When reached: block new entries for the remainder of the day.
- Existing positions continue to be managed (trailing stop still active).

---

## The Span (Palmo)

The span is the central unit of the position. It is defined at entry and never changes.

```
span = abs(entry_price - technical_stop)
```

The span serves as:
- **Unit of risk**: position is sized so that 1 span of loss = 1% of capital
- **Unit of movement**: trailing stop moves in integer multiples of span
- **Unit of decision**: only complete span events trigger system action

---

## Stop Loss

The stop loss is defined at position entry. It is structural, not arbitrary.

- **Method**: Second technical event on the 15-minute chart
  - LONG: second relevant support level
  - SHORT: second relevant resistance level
- The stop is the reference point for the span.
- The stop is never moved against the position (monotonic).

---

## Trailing Stop: Discrete Step

The v3 trailing stop is **discrete**, not continuous.

### Properties

1. **Monotonic**: stop only moves in the favorable direction, never against.
2. **Discrete**: stop moves only in complete span steps.
3. **Deterministic**: no reaction to partial movements or micro-variations.
4. **Anchored to entry**: steps are computed from entry price, not from peak.

### Algorithm (LONG)

```
completed_spans = floor((peak_price - entry_price) / span)
trailing_stop = initial_stop + completed_spans × span
```

### Algorithm (SHORT)

```
completed_spans = floor((entry_price - low_price) / span)
trailing_stop = initial_stop - completed_spans × span
```

### Example (LONG)

```
entry = 95,000
technical_stop = 93,500
span = 1,500

Price reaches 96,500 (entry + 1×span) → stop moves to 95,000 (breakeven)
Price reaches 98,000 (entry + 2×span) → stop moves to 96,500
Price reaches 99,500 (entry + 3×span) → stop moves to 98,000
Price recedes to 97,200              → stop stays at 98,000 (no partial reaction)
Price recedes to 98,000              → position closed at 98,000
```

### Behavioral Rule

If the price nearly hits the stop, doesn't hit it, and then recovers to entry:
**Robson does nothing.** The stop stays at its original position.
The system reacts only to complete events. It does not react to "almost".

---

## Architecture

```
            EngineAction (proposed)
                   │
                   ▼
          ┌────────────────┐
          │  RISK ENGINE   │
          │                │
          │  Monthly Check │──── 4% drawdown → MonthlyHalt
          │  Daily Check   │──── 3% daily loss → Block new entries
          │  Position Check│──── Max positions, exposure, concentration
          │  Verdict        │
          └───────┬────────┘
                  │
       ┌──────────┴──────────┐
       │                     │
RiskClearance::        RiskClearance::
Approved                Denied(reason)
       │                     │
       ▼                     ▼
GovernedAction          RiskDenied Event
(proceed)               (logged, blocked)
```

---

## Risk Limits

### Hard Limits (non-overridable)

| Limit | Value | Override |
|-------|-------|----------|
| Risk per trade | 1% of capital | NO |
| Max monthly drawdown | 4% of capital | NO |
| Max daily loss | 3% of capital | NO |

### Soft Limits (existing from v2, not expanded in v3)

| Limit | Default | Note |
|-------|---------|------|
| Max open positions | 3 | Preserved from v2 |
| Max total exposure | 30% of capital | Preserved from v2 |
| Max single position | 15% of capital | Preserved from v2 |
| No duplicate position | same symbol+side | Preserved from v2 |

These soft limits are preserved as-is. v3 does not add new soft limits.

---

## What v3 Does NOT Include

The following were in the previous spec and are explicitly **removed from v3 scope**:

- **Escalation Ladder (L1/L2/L3/L4)**: replaced by binary MonthlyHalt
- **Dynamic Limits**: volatility adjustment, funding rate adjustment, correlation adjustment
- **Soft Limit Overrides with expiry**: no operator override mechanism
- **Circuit Breaker with auto-escalation**: no timed escalation
- **Half-Open recovery state**: no gradual recovery testing

These may be evaluated for future versions but do not enter v3.

---

## Design Philosophy

Robson v3 is a **disciplined executor of directional risk management**.

Priorities:
1. Robustness — deterministic behavior under all conditions
2. Predictability — operator always knows what the system will do
3. Simplicity — fewer moving parts, fewer failure modes
4. Structural safety — the span protects the position by design
5. No micro-sensitivity — the system does not chase noise

---

## Implementation Reference

| Component | File | Description |
|-----------|------|-------------|
| Risk per trade | `v2/robson-domain/src/value_objects.rs` | `RiskConfig::RISK_PER_TRADE_PCT = 1` |
| Position sizing | `v2/robson-domain/src/entities.rs` | `calculate_position_size()` |
| Trailing stop | `v2/robson-engine/src/trailing_stop.rs` | `update_trailing_stop_discrete()` |
| Risk gate | `v2/robson-engine/src/risk.rs` | `RiskGate::evaluate()` with monthly check |
| Span definition | `v2/robson-domain/src/value_objects.rs` | `TechnicalStopDistance::span()` |
