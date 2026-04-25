# ROBSON v3 — RISK ENGINE SPECIFICATION

**Date**: 2026-04-11
**Status**: APPROVED (revised — replaces the previous L1–L4 escalation design with binary MonthlyHalt)
**Classification**: CRITICAL PATH — Financial Safety Component

---

## Role

The Risk Engine is the single component where a bug means financial loss.
It is a mandatory blocking gate. No action proceeds without Risk Engine clearance.

---

## Governing Invariants

The rules below are stated symbol-agnostically by design. A Risk Engine rule that
hard-codes a specific symbol is a violation of
[ADR-0023](../adr/ADR-0023-symbol-agnostic-policy-invariant.md). `symbol` is always a
variable; tick size, lot step, min notional, max leverage, and fee rate come from
`ExchangePort::exchange_info()` at runtime.

The Risk Engine is also the downstream half of the **Robson-authored position
invariant** ([ADR-0022](../adr/ADR-0022-robson-authored-position-invariant.md)). The
write side (every entry must pass this engine) is unchanged. The read side (every
open exchange position must correspond to an entry that passed this engine) is
enforced by the Position Reconciliation Worker documented in the Runtime spec.
An UNTRACKED position does not pass through the Risk Engine on its way to being
closed — closing an UNTRACKED position is always permitted (reducing exposure is
always safe) and is tagged `UNTRACKED_ON_EXCHANGE` in the audit trail.

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
- Resume: not implemented. MonthlyHalt persists until process restart. Calendar-month boundary auto-reset is follow-up work, not a current feature.

**Trigger condition**: `MonthlyPnL ≤ −(capital × 0.04)`

Where `MonthlyPnL` is defined in the **PnL Model** section of this document.

**Current approximation**: The trigger uses `realized_pnl_gross − fees_paid`.
See "PnL Model — Current Implementation State".

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

## PnL Model

Defines how financial performance is measured and fed into risk decisions.

### Canonical Formula

```
MonthlyPnL = Σ(realized_pnl_gross) - Σ(fees_paid) + unrealized_pnl
```

### Component Definitions

**`realized_pnl_gross`** — Gross P&L of a closed position.
- Formula: `(exit_price − entry_price) × quantity`, signed by side (positive for profit).
- Source: `Event::PositionClosed { realized_pnl }` → stored in `Position.realized_pnl`.
- **Does not include fees.** Fees are in the separate `total_fees` / `fees_paid` fields.

**`fees_paid`** — Commissions incurred by a position.
- Source: `Event::PositionClosed { total_fees }` → stored in `Position.fees_paid`.
- Includes: entry commission, exit commission.
- Does not include: funding rates (not tracked in v3).
- Tracked separately from `realized_pnl_gross`.

**`unrealized_pnl`** — Mark-to-market P&L of currently open Active positions.
- Computed by `Position::calculate_pnl()` using `current_price` (last received tick).
- `current_price` is the last WebSocket tick recorded by the position.
- **Not exchange mark price.** Exchange mark price is not fetched. In high-volatility
  conditions, last tick price may deviate from exchange mark price. This is a known
  approximation.
- Entering positions contribute `Decimal::ZERO` unrealized PnL (order not filled yet).

### Source of Truth

The **exchange (Binance)** is the primary source of financial truth.

Robson maintains a **local projection** derived from events. This projection:
- Is not continuously reconciled against the exchange.
- May diverge from exchange-reported P&L due to: slippage not captured, fees
  partially modeled (no funding rates), fill prices approximated from ticks.

Robson's PnL model is authoritative for **risk gate decisions**. It is not
authoritative for accounting or tax reporting.

### Current Implementation State

| Component | Status | Notes |
|---|---|---|
| `realized_pnl_gross` | Implemented | Gross only. Fees excluded. Source: `find_closed_in_month()`. |
| `fees_paid` (commissions) | Implemented — `p.realized_pnl - p.fees_paid` | Stored in `Position.fees_paid`. Subtracted in `build_risk_context()` monthly PnL calculation. |
| Monthly net PnL (`gross − fees`) | Implemented | `build_risk_context()` sums `realized_pnl - fees_paid` from closed positions. |
| `unrealized_pnl` (Active positions) | Implemented | Uses last tick price via `calculate_pnl()`. Not exchange mark price. |
| Funding rates | Not tracked | Not captured anywhere in the current system. |

---

## Risk Limits

### Hard Limits (non-overridable)

| Limit | Value | Override |
|-------|-------|----------|
| Risk per trade | 1% of capital | NO |
| Max monthly drawdown | 4% of capital | NO |

### Derived Limits — Not Independent Parameters

`max_open_positions`, `max_total_exposure_pct`, and `max_single_position_pct` are
**eliminated** as independent configuration parameters. See ADR-0024.

The only real bound on a single position's size is physical: its notional value cannot
exceed available capital (spot) or available capital × leverage (isolated margin). The
Golden Rule guarantees the risk is exactly 1% of capital regardless of notional size.
Any percentage-based cap on position size would either be redundant or contradict the
Golden Rule silently.

**What replaces max_open_positions:** dynamic slot calculation (see below).

**No-duplicate-position guard** (same symbol+side) is preserved as an operational
constraint.

---

## Dynamic Slot Calculation

The question "can a new position be opened?" is answered at each decision point:

```
monthly_budget   = capital_base_current_month × 4%
realized_loss    = |sum of losses on positions closed this calendar month|
latent_risk      = Σ max(0, loss_if_current_stop_hit) for each open position
remaining_budget = monthly_budget − realized_loss − latent_risk
can_open         = remaining_budget ≥ capital_base_current_month × 1%
slots_available  = floor(remaining_budget / risk_per_trade_amount)
```

**Latent risk per position:**

| Stop position | Latent risk |
|---------------|-------------|
| At original stop level | ≈ 1% of capital (full span risk) |
| At breakeven (trailing = entry) | 0 |
| Above entry (locked profit) | 0 (floor at zero; cannot lose) |

**Consequence:** the number of simultaneously open positions is unbounded by policy.
With multiple positions at breakeven, the operator may hold 6, 9, or more positions
simultaneously, all within the 4% monthly budget. Each new batch of positions moves
to breakeven, freeing budget for the next batch.

---

## Month Boundary Rule

Open positions are **never closed** by a calendar event. Stop levels, span, and
trailing stop logic are unaffected by the month change. What changes is the budget
accounting.

**At 00:00 UTC on the first day of a new month:**

```
latent_risk_carried = Σ max(0, loss_if_current_stop_hit) for all currently open positions
current_equity      = realized_capital + unrealized_pnl_of_open_positions (mark-to-market)
capital_base        = current_equity − latent_risk_carried
monthly_budget      = capital_base × 4%
realized_loss       = 0  (reset)
```

The capital base assumes worst case for all carried positions. This absorbs inherited
risk into the base rather than carrying it as a debt against the new month's budget.

**Result:** the new month always starts with `slots_available = 4`, regardless of how
many positions are carried over.

**Wins from carried positions** that close during the new month flow into `current_equity`
and feed the capital base of the month after. They do not retroactively increase the
current month's budget.

**Month boundary implementation status:** follow-up work, tracked as MIG-v3#12. The
daemon must detect the UTC calendar boundary, compute the new capital base, persist it,
reset the monthly loss accumulator, and emit a `MonthBoundaryReset` event.

---

## What v3 Does NOT Include

The following were in the previous spec and are explicitly **removed from v3 scope**:

- **Escalation Ladder (L1/L2/L3/L4)**: replaced by binary MonthlyHalt
- **Static soft limits (max_open_positions=3, max_single_position_pct=15%, max_total_exposure_pct=30%)**: replaced by dynamic slot calculation derived from policy (ADR-0024)
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
| Risk per trade | `v3/robson-domain/src/value_objects.rs` | `RiskConfig::RISK_PER_TRADE_PCT = 1` |
| Position sizing | `v3/robson-domain/src/entities.rs` | `calculate_position_size()` |
| Trailing stop | `v3/robson-engine/src/trailing_stop.rs` | `update_trailing_stop_discrete()` |
| Risk gate | `v3/robson-engine/src/risk.rs` | `RiskGate::evaluate()` — monthly check uses `<=` (blocks at exactly 4.00%) |
| MonthlyHalt gate | `v3/robsond/src/circuit_breaker.rs` | Binary `Active | MonthlyHalt` state machine |
| MonthlyHalt trigger | `v3/robsond/src/position_manager.rs` | `trigger_monthly_halt()` — closes all positions, blocks new entries |
| Span definition | `v3/robson-domain/src/value_objects.rs` | `TechnicalStopDistance::span()` |

## Follow-up Required

| Gap | Status | Impact |
|-----|--------|--------|
| Monthly PnL — gross aggregation | Implemented | `build_risk_context()` sums `realized_pnl_gross - fees_paid` from `find_closed_in_month()` and `calculate_pnl()` from Active positions. `evaluate_monthly_halt()` auto-triggers MonthlyHalt when threshold crossed. |
| Monthly PnL — fees deduction | Implemented | `fees_paid` is tracked per position and subtracted: `p.realized_pnl - p.fees_paid`. MonthlyHalt triggers on net PnL. |
| Unrealized PnL — exchange mark price | Not implemented | `unrealized_pnl` uses last tick price, not exchange mark price. Known approximation. |
| Entering position cancel on halt | Not implemented | `trigger_monthly_halt()` cannot cancel pending entry orders on exchange. Entering positions remain until fill or exchange session expiry. |
| MonthlyHalt auto-reset | Not implemented — tracked as MIG-v3#12 | Calendar-month boundary detection required. On boundary: compute new `capital_base`, reset `realized_loss`, emit `MonthBoundaryReset`, clear MonthlyHalt state. MonthlyHalt persists until process restart in current implementation. |
| Policy Layer — `TradingPolicy` + `TechStopConfig` | Done — repository-verified (2db23ad2, corrected by 0b3653a7) | `robson-domain::policy` module created. `RiskGate` consumes `TradingPolicy` for slot calculation. Static `RiskLimits` struct preserved for compatibility but fields no longer enforced. See ADR-0024. |
| Dynamic slot calculation | Done — repository-verified (2db23ad2, corrected by 0b3653a7) | `RiskContext::slots_available()`, `latent_risk_sum()`, `realized_loss_abs()` added. `RiskGate::evaluate()` checks slots instead of `max_open_positions`. Enables unbounded simultaneous positions within monthly budget. `capital_base` approximated from current in-memory state; persisted base lands in MIG-v3#12. |
| MonthlyHalt latent-risk trigger | Done — repository-verified (2db23ad2, corrected by 0b3653a7) | Dynamic slot check uses `remaining_budget = monthly_budget − realized_loss − latent_risk`. When slots = 0, rejection uses `RiskCheck::MonthlyDrawdown` which triggers MonthlyHalt. |
| Monthly state persistence (`capital_base`, `realized_loss`) | Not implemented — tracked as MIG-v3#12 | Prerequisite for real capital operations. New domain event `MonthBoundaryReset { capital_base, carried_positions_risk, month, year, timestamp }`. New DB migration: `monthly_state { month, year, capital_base, created_at }`. New projector handler. Daemon: UTC month boundary detection that survives restarts. `RiskContext` reads persisted `capital_base` from projection. Required before VAL-002. |
