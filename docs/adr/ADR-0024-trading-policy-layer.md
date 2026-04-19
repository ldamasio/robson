# ADR-0024 — Trading Policy Layer

**Date**: 2026-04-19
**Status**: DECIDED — MIG-v3#11 IMPLEMENTED (2026-04-19)
**Deciders**: RBX Systems (operator + architecture)

---

## Context

The Robson v3 risk engine carries several structural problems inherited from v2:

1. `RiskLimits` (max_open_positions=3, max_total_exposure_pct=30, max_single_position_pct=15)
   are legacy values documented as "preserved from v2" with no derivation from policy.
2. Risk logic is scattered across four crates with no single authoritative source of truth
   (identified in `RISK-ENGINE-PLAN.md` §3).
3. The relationship between the technical stop distance and position sizing is the core
   invariant of the system, yet the exposure limits contradict it silently: with
   risk_per_trade=1% and min_stop=0.1%, the derived max single position is 10x capital —
   but the static limit of 15% implies a minimum stop of 6.67%, which is never enforced.
4. The static max_open_positions=3 has no policy basis and produces wrong behavior:
   a user with positions at breakeven should be allowed to open more, but the static
   value blocks them.
5. The month boundary rule — how capital base is calculated at the start of a new month
   in the presence of open positions — was undefined, creating implementation ambiguity.

---

## Decisions

### 1. Policy Layer Location

All trading policies live in `robson-domain` as pure Rust structs with no external
dependencies. `robson-domain` is the single source of truth. `robson-engine` consumes
policies to derive its behavior; `robsond` reads configurable parameters via environment
variables and constructs the policy structs at startup.

### 2. Primary Immutable Policies

These values are fixed by product definition. They are not configurable via environment
variables, operator API, or any runtime mechanism.

| Policy | Value | Rationale |
|--------|-------|-----------|
| risk_per_trade_pct | 1% | Golden Rule anchor: position is sized so that 1 span of loss = 1% of capital |
| max_monthly_drawdown_pct | 4% | 4 consecutive losses × 1% = 4%; after 4 errors in a month the user is blocked |

### 3. Technical Stop Configuration (Configurable)

These parameters govern how chart analysis produces a technical stop. They are
configurable per environment (via environment variables) but constrained by validation.

| Parameter | Default | Min | Max | Env var |
|-----------|---------|-----|-----|---------|
| min_tech_stop_pct | 1.0% | 0.1% | — | ROBSON_MIN_TECH_STOP_PCT |
| max_tech_stop_pct | 10.0% | — | 20.0% | ROBSON_MAX_TECH_STOP_PCT |
| support_level_n | 2 | 1 | 5 | ROBSON_TECH_STOP_SUPPORT_N |
| lookback_candles | 100 | 50 | 500 | ROBSON_TECH_STOP_LOOKBACK |
| timeframe | 15m | — | — | fixed by policy, not configurable |

### 4. All Exposure Limits Are Derived — Not Independent Parameters

`max_single_position_pct`, `max_total_exposure_pct`, and `max_open_positions` are
eliminated as independent configuration parameters.

The only real limit on a single position is physical: its notional value cannot exceed
available capital (spot) or available capital × leverage (isolated margin). Position
sizing via the Golden Rule already ensures the risk is exactly 1%. Any additional
percentage-based cap would be either redundant (already guaranteed by the formula) or
contradictory (it would silently fail trades that are policy-compliant).

The duplicate-position guard (no same symbol+side) is preserved as an operational
constraint, not a risk limit.

### 5. Dynamic Slot Calculation Replaces Static max_open_positions

The question "can a new position be opened?" is answered dynamically by the risk engine
at each decision point. The calculation:

```
monthly_budget       = capital_base_current_month × max_monthly_drawdown_pct
                     = capital_base_current_month × 4%

realized_loss        = |sum of losses on positions closed this calendar month|

latent_risk          = Σ max(0, loss_if_current_stop_hit) for each currently open position

remaining_budget     = monthly_budget − realized_loss − latent_risk

can_open             = remaining_budget ≥ risk_per_trade_amount
                     = remaining_budget ≥ capital_base_current_month × 1%

slots_available      = floor(remaining_budget / risk_per_trade_amount)
```

**Latent risk per position:**

- Stop at original level (no trailing movement): risk = (entry−stop) × qty ≈ 1% of capital
- Stop at breakeven (trailing stop = entry): risk = 0
- Stop above entry (locked profit zone): risk = 0 (cannot lose; floor at zero)

**Implication for simultaneous positions:**

The number of simultaneously open positions is unbounded by policy. The budget is the
only constraint. Positions whose stops have advanced to breakeven or beyond contribute
zero latent risk, freeing budget for new positions. In a month with sustained wins
and trailing stops advancing to breakeven, the operator may hold 6, 9, or more positions
simultaneously, all within the 4% monthly budget.

**Implication for the MonthlyHalt trigger:**

The MonthlyHalt fires when `remaining_budget < risk_per_trade_amount`, i.e., when the
monthly loss budget is fully consumed (realized + latent). This replaces the current
implementation which fires on `MonthlyPnL ≤ −(capital × 4%)` using only realized PnL.
The corrected trigger also accounts for latent risk of open positions.

### 6. Month Boundary Rule

At the boundary between calendar months, the following procedure applies:

**Step 1 — Compute the new month's capital base:**

```
latent_risk_carried  = Σ max(0, loss_if_current_stop_hit) for each position still open
                       at 00:00 UTC on the first day of the new month

capital_base         = current_equity − latent_risk_carried

Where current_equity = realized_capital + unrealized_pnl_of_open_positions (mark-to-market)
```

The capital base assumes the worst case: every open position from the prior month hits
its current stop. This absorbs inherited risk into the base rather than carrying it as
a debt against the new month's budget.

**Step 2 — Open positions are not closed:**

Open positions from the prior month continue without interruption. `robsond` does not
close positions at month boundaries. Stop levels, span, and trailing stop logic are
unaffected by the month change.

**Step 3 — Risk tracking resets:**

`realized_loss` resets to zero. `latent_risk` of carried positions is zero for budget
purposes (already absorbed in `capital_base`). The new month starts with
`slots_available = 4` regardless of how many positions are carried over.

**Step 4 — Wins from carried positions:**

If a carried position closes in profit during the new month, that gain flows into
`current_equity` and will feed the capital base of the month after. It does not
retroactively increase the current month's `capital_base` or budget.

**Rationale for the abstraction:**

This design gives the operator a clean, predictable invariant: every month begins with
the ability to open at least 4 positions. Performance from prior months does not
penalize nor inflate the current month's budget. The capital base grows month-over-month
through compounding of realized gains, which is the intended incentive.

### 7. What Is Eliminated

The following structures and parameters are removed:

| Removed | Replaced by |
|---------|-------------|
| `RiskLimits.max_open_positions` | Dynamic slot calculation (Decision 5) |
| `RiskLimits.max_total_exposure_pct` | Physical capital bound (enforced by exchange) |
| `RiskLimits.max_single_position_pct` | Physical capital bound (enforced by exchange) |
| `RiskLimits` struct (as independent config) | `TradingPolicy` + `TechStopConfig` in `robson-domain::policy` |
| Soft limit section in risk engine spec | Derived limits section (see updated spec) |

The no-duplicate-position guard is preserved.

---

## Consequences

**Immediate (design):**

- `robson-domain` gains a `policy` module with `TradingPolicy` and `TechStopConfig`.
- `robson-engine::RiskGate` is refactored to accept `TradingPolicy` and compute slots
  dynamically from `RiskContext`.
- `robsond::config` drops `ROBSON_RISK_MAX_*` env vars (which were planned but never
  implemented) and gains `ROBSON_MIN_TECH_STOP_PCT`, `ROBSON_MAX_TECH_STOP_PCT`,
  `ROBSON_TECH_STOP_SUPPORT_N`, `ROBSON_TECH_STOP_LOOKBACK`.
- `robsond::circuit_breaker` MonthlyHalt trigger must be updated to account for latent
  risk, not only realized PnL.

**Testnet unblock:**

With `min_tech_stop_pct = 1%` (env override for testnet), and BTCUSDT typical stops of
2-3%, the derived max position notional = 50% of capital. There is no independent soft
limit to block this. The testnet loop unblocks without requiring artificial limit
overrides.

**Month boundary implementation (MIG-v3#12 — separate from MIG-v3#11):**

MIG-v3#11 implements the policy layer and dynamic slot calculation with a best-effort
in-memory approximation of `capital_base` (sufficient for testnet / VAL-001). MIG-v3#12
delivers the event-sourced persistence required before real capital operations (VAL-002).

MIG-v3#12 scope:

1. **New domain event** in `robson-domain::events`:
   ```
   MonthBoundaryReset {
     capital_base: Decimal,          // current_equity − latent_risk_carried
     carried_positions_risk: Decimal, // Σ latent risk of open positions absorbed
     month: u32,
     year: u32,
     timestamp: DateTime<Utc>,
   }
   ```

2. **New DB migration**: `monthly_state` table:
   ```sql
   CREATE TABLE monthly_state (
     id            UUID PRIMARY KEY,
     year          SMALLINT NOT NULL,
     month         SMALLINT NOT NULL,
     capital_base  NUMERIC(20,8) NOT NULL,
     created_at    TIMESTAMPTZ NOT NULL,
     UNIQUE (year, month)
   );
   ```

3. **New projector handler** for `MonthBoundaryReset` → upserts `monthly_state`.

4. **Daemon month boundary detection**: UTC calendar check on each daemon tick.
   On boundary:
   - Compute `current_equity` (realized capital + mark-to-market of open positions).
   - Compute `latent_risk_carried` (Σ max(0, loss_if_stop_hit) for open positions).
   - Compute `capital_base = current_equity − latent_risk_carried`.
   - Emit `MonthBoundaryReset` event to event log.
   - Reset monthly realized loss accumulator.
   - Clear `MonthlyHalt` state if active (auto-reset).
   - Must be idempotent: if the daemon restarts mid-month, the event must not
     be re-emitted. Gate on `monthly_state` having no entry for the current month.

5. **RiskContext**: on startup, read `capital_base` from `monthly_state` projection
   for the current month. Fall back to current equity if no entry exists (first month).

MIG-v3#12 is a hard prerequisite for VAL-002 (real capital activation).

---

## Implementation Status (2026-04-19)

MIG-v3#11 is implemented and repository-verified:

- `robson` commit `2db23ad2` added `robson-domain::policy::{TradingPolicy,
  TechStopConfig}`, wired `TradingPolicy` into `RiskGate`, and removed enforcement
  of legacy `max_open_positions`, `max_total_exposure_pct`, and
  `max_single_position_pct`.
- `robson` commit `0b3653a7` corrected dynamic slot accounting so
  `realized_loss` is the sum of absolute losing closed positions for the current
  month. Winning positions do not offset consumed loss budget.
- `robson` commit `19130cf3` updated architecture verification references.
- `rbx-infra` commit `c3b1bc3` added the testnet `ROBSON_MIN_TECH_STOP_PCT`
  configuration.

Validation status:

- `cargo fmt --all --check`: pass
- `cargo build --all`: pass
- `cargo test --all`: pass (`409 passed, 24 ignored`)
- `cargo clippy --all-targets -- -D warnings`: still blocked by pre-existing
  repository baseline issues outside MIG-v3#11 (`missing_docs` in domain support
  modules and clippy config deprecation warnings)

Operational status:

- VAL-001 Phase 2 is unblocked in repository state.
- Testnet still needs a new image, ArgoCD sync, and live Phase 2 execution.
- MIG-v3#12 remains required before VAL-002 because `capital_base` and monthly
  realized loss are not yet event-sourced across restarts/month boundaries.

---

## Related

- ADR-0021 — Opportunity Detection vs Technical Stop Analysis
- ADR-0022 — Robson-Authored Position Invariant
- ADR-0023 — Symbol-Agnostic Policy Invariant
- `docs/requirements/POSITION-SIZING-GOLDEN-RULE.md`
- `docs/specs/TECHNICAL-STOP-RULE.md`
- `docs/architecture/v3-risk-engine-spec.md` (updated by this ADR)
- `v2/docs/architecture/RISK-ENGINE-PLAN.md`
