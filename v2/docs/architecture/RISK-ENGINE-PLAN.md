# Risk Engine Architectural Plan

**Status**: Approved plan, pending implementation
**Author**: Architecture session 2026-03-28
**Scope**: Risk management formal design for Robson v2.5 → v3
**Prerequisite**: EventLog as source of truth (see source-of-truth decision)

---

## 1. Current State of Risk (Factual)

### Where sizing lives

`robson-domain/src/entities.rs:189-213` — `calculate_position_size()`:
```
Position Size = max_risk_amount / stop_distance
             = (capital × risk_pct / 100) / |entry - technical_stop|
```
Called by `Engine::decide_entry()` at `robson-engine/src/lib.rs:301`.

`robson-domain/src/value_objects.rs:287-346` — `RiskConfig`:
- `capital: Decimal` (available capital in quote currency)
- `risk_per_trade_pct: Decimal` (1-5%)
- `max_risk_amount()` = `capital * pct / 100`
- `LEVERAGE` = 10 (const, hardcoded)

Validation: capital > 0, risk_pct in (0, 5].

### Where the technical stop lives

`robson-domain/src/value_objects.rs:400-603` — `TechnicalStopDistance`:
- `distance: Decimal` (absolute, in quote currency)
- `distance_pct: Decimal`
- `entry_price: Price`
- `initial_stop: Price`
- Constructors: `from_entry_and_stop()`, `new_validated(entry, stop, side)`
- Validation: distance > 0, 0.1% ≤ distance_pct ≤ 10%, side-correct
- Methods: `calculate_trailing_stop_long(peak)`, `calculate_trailing_stop_short(trough)`, `should_exit_long/short()`

### Where the trailing stop lives

`robson-engine/src/trailing_stop.rs` — `update_trailing_stop_anchored()`:
- Uses `TechnicalStopDistance.distance` as fixed offset from favorable extreme
- LONG: stop = peak - distance
- SHORT: stop = trough + distance
- Monotonicity invariant: stop only moves favorably

`robson-engine/src/lib.rs:457+` — `Engine::process_active_position()`:
- Checks exit first (priority)
- Calculates new trailing stop on favorable movement
- Emits `TrailingStopUpdated` or `ExitTriggered` events

### Where the Safety Net lives

`robson-domain/src/detected_position.rs` — `DetectedPosition`:
- Represents rogue positions (opened outside Robson)
- `calculate_safety_stop()`: fixed 2% from entry
- `StopMethod::Fixed2Percent` (only method implemented)

`robsond/src/position_monitor.rs` (1174 lines) — `PositionMonitor`:
- Polls Binance isolated margin via `BinanceRestClient`
- Detects rogue positions, calculates safety stop, executes with retry
- Core exclusion via `CorePositionOpened/Closed` event listener
- Exponential backoff, panic mode after 3+ failures

### Where risk validation EXISTS today

| Check | Location | What it does |
|---|---|---|
| Risk % cap (0-5%) | `RiskConfig::new()` value_objects.rs:305-319 | Rejects risk_pct > 5% |
| Stop distance bounds (0.1-10%) | `TechnicalStopDistance::validate()` value_objects.rs:517-538 | Rejects too wide/tight stops |
| Side-correct stop | `TechnicalStopDistance::new_validated()` value_objects.rs:475-508 | LONG stop below entry, SHORT above |
| Position sizing | `calculate_position_size()` entities.rs:189 | size = risk / distance |
| Signal matches position | `DetectorSignal::validate_for_position()` entities.rs:522-545 | symbol/side/position_id match |
| Margin mode validation | `ExchangePort::validate_margin_settings()` exec/ports.rs:39 | Isolated + 10x before order |
| Idempotency | `IntentJournal` exec/intent.rs | Prevents duplicate order execution |

### Where risk validation DOES NOT exist today

| Missing check | Impact |
|---|---|
| **Max open positions** | Can open unlimited positions simultaneously |
| **Total exposure limit** | No cap on aggregate notional value |
| **Available capital check** | Can place order even if margin is insufficient |
| **Daily loss limit / circuit breaker** | No automatic shutdown on drawdown |
| **Position concentration** | Can put all capital in one symbol |
| **Correlation check** | Can open LONG and SHORT on same symbol |
| **Post-trade risk update** | RiskConfig.capital never decreases after losses |

---

## 2. What Is Strong and Must Be Preserved

### TechnicalStopDistance — the core invariant

This is the strongest design element in the system. Preserve entirely:

1. **Dual-use pattern**: same distance drives both sizing and trailing stop
2. **Immutability**: distance is fixed at position creation, never changes
3. **Hard invariants**: distance > 0, side-correct, within 0.1%-10%
4. **Direct derivation from chart**: the user provides entry + stop from technical analysis; the system computes everything else

### The sizing formula

`position_size = max_risk_amount / stop_distance` in `calculate_position_size()` is correct and well-tested. The test `test_position_sizing_risk_stays_constant` at entities.rs:709 proves that risk amount is constant regardless of stop width. Do not change this.

### RiskConfig as per-trade parameterization

`RiskConfig` correctly encapsulates the user's risk appetite (capital, risk %). The validation (risk ≤ 5%, capital > 0) is sensible. The hardcoded LEVERAGE=10 is appropriate for the current single-exchange model.

### Engine as pure decision function

`Engine::decide_entry()`, `process_entry_fill()`, `process_active_position()` are deterministic and I/O-free. Risk checks that are pure computations (sizing, stop validation) belong here. This pattern must be preserved.

### Safety Net as parallel protection

`PositionMonitor` + `DetectedPosition` correctly handle the "rogue position" scenario independently from the core flow. The core exclusion mechanism (`CorePositionOpened/Closed`) is the right boundary. Do not merge Safety Net into the core risk engine.

---

## 3. Fragmentation of Risk

### What is scattered

| Concept | Domain | Engine | Exec | Daemon |
|---|---|---|---|---|
| Capital amount | `RiskConfig.capital` | `Engine.risk_config` | — | `Config.engine` |
| Risk percentage | `RiskConfig.risk_per_trade_pct` | `Engine.risk_config` | — | `Config.engine` |
| Sizing calculation | `calculate_position_size()` | called in `decide_entry()` | — | — |
| Stop distance validation | `TechnicalStopDistance::validate()` | called in `decide_entry()` | — | — |
| Margin mode check | — | — | `ExchangePort::validate_margin_settings()` | — |
| Safety stop (2%) | `DetectedPosition::calculate_safety_stop()` | — | — | `PositionMonitor` |
| Trailing stop logic | `TechnicalStopDistance.calculate_trailing_stop_*()` | `trailing_stop.rs` + `process_active_position()` | — | — |

Risk is split across 4 crates with no single place that answers: "can this trade happen?"

### What is duplicated

- Leverage = 10 is defined in BOTH `RiskConfig::LEVERAGE` (domain) AND `executor::FIXED_LEVERAGE` (exec). If they diverge, the system breaks silently.
- Stop distance percentage is validated in `TechnicalStopDistance::validate()` AND checked conceptually in `new_validated()` (but with different rules — `validate()` checks bounds, `new_validated()` checks direction only).

### What is implicit

- **Available capital is never checked**. `RiskConfig.capital` is set at daemon startup and never updated. After a losing trade, the system still sizes positions as if full capital is available.
- **Number of open positions is never checked**. Nothing prevents opening 10 simultaneous positions, each risking 1%, totaling 10% exposure.
- **Margin availability is checked on the exchange** (`validate_margin_settings`) but not pre-validated in the risk logic. The exchange rejects the order, not Robson.

### What depends on inconsistent runtime

- `RiskConfig` is constructed once at daemon startup with a static capital value. It does not reflect actual balance. If the source of truth is EventLog, `RiskConfig.capital` should be derived from events (deposits, realized PnL, fees).
- `Engine` holds `RiskConfig` as an immutable field. There is no mechanism to update capital after a trade closes. After 5 winning trades, the engine still sizes from the original capital.

### What would not survive replay correctly

- `RiskConfig.capital` is set from environment/config, not from events. Replay cannot reconstruct "what was the capital at the time of this trade?" because no event records it.
- `tech_stop_distance` is set via direct save on `Position` in `arm_position()` and in `decide_entry()`. It is NOT in the `PositionArmed` event. Replay of `PositionArmed` cannot reconstruct the position's tech_stop_distance.
- `binance_position_id` is set in `handle_entry_fill()` via direct save. Not in any event.

---

## 4. Architectural Decision: Responsibilities

### RiskConfig — what it IS and IS NOT

**IS**: Per-trade risk parameterization. Answers: "how much of my capital do I risk per trade?"

**IS NOT**: Portfolio state. Should NOT hold mutable state like current balance, open position count, or daily PnL. These belong in the risk context, derived from events.

**Decision**: RiskConfig remains as-is (capital + risk_pct). But it must be understood as the **input policy**, not as a live view of risk state.

### TechnicalStopDistance — what it IS and IS NOT

**IS**: The structural anchor for both sizing and exit. The immutable distance that binds the position to its original technical analysis.

**IS NOT**: A runtime risk monitor. It does not check portfolio-level exposure, drawdown, or concentration. Those are RiskGate responsibilities.

**Decision**: TechnicalStopDistance stays in robson-domain. No changes needed.

### RiskGate (new) — what it IS

**IS**: The pre-trade approval gate that answers: "given the current portfolio state, should this trade be allowed?"

Checks:
1. Max open positions not exceeded
2. Total exposure (aggregate notional) within limit
3. Available margin sufficient for the proposed order
4. No duplicate position on same symbol+side
5. Daily loss limit not breached (circuit breaker)

**IS NOT**: Sizing logic (that's TechnicalStopDistance + calculate_position_size). Not trailing stop logic (that's Engine). Not exchange validation (that's ExchangePort).

**Decision**: RiskGate is a new struct in `robson-engine/src/risk.rs`. It lives in the engine crate because it is pure computation (no I/O) and is called by the Engine before producing entry actions.

### Safety Net — what it IS and IS NOT

**IS**: A parallel protection system for positions outside the core flow (rogue positions). Operates independently with its own simplified risk rules (fixed 2%).

**IS NOT**: Part of the core risk engine. It does not participate in pre-trade approval. It does not share state with RiskGate.

**Decision**: Safety Net remains in robsond. The boundary between core and safety net is the `CorePositionOpened/Closed` event pair. No merging.

### Boundary summary

```
                    ┌────────────────────────────────────┐
                    │         robson-domain               │
                    │                                      │
                    │  RiskConfig (policy input)           │
                    │  TechnicalStopDistance (sizing+stop)  │
                    │  calculate_position_size()           │
                    │  DetectedPosition (safety net domain)│
                    └────────────────────────────────────┘
                                     │
                    ┌────────────────────────────────────┐
                    │         robson-engine               │
                    │                                      │
                    │  Engine (decision: entry/trail/exit) │
                    │  RiskGate (pre-trade approval)  NEW  │
                    │  trailing_stop.rs (trailing logic)   │
                    └────────────────────────────────────┘
                                     │
                    ┌────────────────────────────────────┐
                    │         robson-exec                 │
                    │                                      │
                    │  Executor (I/O: exchange + persist)  │
                    │  ExchangePort.validate_margin()      │
                    │  IntentJournal (idempotency)         │
                    └────────────────────────────────────┘
                                     │
                    ┌────────────────────────────────────┐
                    │         robsond                     │
                    │                                      │
                    │  PositionManager (lifecycle)         │
                    │  PositionMonitor (safety net runtime)│
                    └────────────────────────────────────┘
```

---

## 5. Formalization of the Risk Engine

### Core types (conceptual, not final code)

```rust
// robson-engine/src/risk.rs

/// Portfolio-level risk limits (configured at startup, static)
pub struct RiskLimits {
    pub max_open_positions: usize,           // e.g., 3
    pub max_total_exposure_pct: Decimal,     // e.g., 30% of capital
    pub max_single_position_pct: Decimal,    // e.g., 15% of capital
    pub daily_loss_limit_pct: Decimal,       // e.g., 3% — circuit breaker
}

/// Snapshot of current portfolio risk state (derived from events/positions)
pub struct RiskContext {
    pub capital: Decimal,                    // current balance (from events)
    pub open_positions: Vec<PositionSummary>,
    pub total_notional_exposure: Decimal,
    pub daily_realized_pnl: Decimal,
    pub daily_unrealized_pnl: Decimal,
}

/// Minimal position info needed for risk checks
pub struct PositionSummary {
    pub position_id: PositionId,
    pub symbol: Symbol,
    pub side: Side,
    pub notional_value: Decimal,
    pub margin_used: Decimal,
    pub unrealized_pnl: Decimal,
}

/// The proposed trade to be evaluated
pub struct ProposedTrade {
    pub symbol: Symbol,
    pub side: Side,
    pub quantity: Quantity,
    pub entry_price: Price,
    pub tech_stop_distance: TechnicalStopDistance,
    pub notional_value: Decimal,
    pub margin_required: Decimal,
}

/// Result of risk evaluation
pub enum RiskVerdict {
    Approved,
    Rejected { check: RiskCheck, reason: String },
}

/// Which check failed
pub enum RiskCheck {
    MaxOpenPositions,
    TotalExposure,
    SinglePositionConcentration,
    InsufficientMargin,
    DailyLossLimit,
    DuplicatePosition,
}

/// The gate itself
pub struct RiskGate {
    limits: RiskLimits,
}

impl RiskGate {
    pub fn evaluate(&self, proposed: &ProposedTrade, context: &RiskContext) -> RiskVerdict { ... }
}
```

### Checks: pre-trade

Called by `Engine::decide_entry()` AFTER sizing, BEFORE producing `PlaceEntryOrder` action.

1. **Max open positions**: `context.open_positions.len() < limits.max_open_positions`
2. **Total exposure**: `(context.total_notional_exposure + proposed.notional_value) / context.capital ≤ limits.max_total_exposure_pct / 100`
3. **Single position concentration**: `proposed.notional_value / context.capital ≤ limits.max_single_position_pct / 100`
4. **Duplicate position**: no existing open position with same symbol + same side
5. **Daily loss circuit breaker**: `context.daily_realized_pnl + context.daily_unrealized_pnl > -(context.capital * limits.daily_loss_limit_pct / 100)`

If any check fails → `RiskVerdict::Rejected` → Engine returns no entry actions, emits a `RiskCheckFailed` event for audit.

### Checks: intra-trade

During position lifetime, the trailing stop mechanism in `Engine::process_active_position()` already handles per-position risk. No additional RiskGate involvement needed intra-trade.

The circuit breaker is the exception: if daily loss exceeds the limit, the engine should refuse to process new entries even for already-armed positions. This is checked in `decide_entry()`, not in `process_active_position()`.

### Checks: emergency

Emergency is the domain of `PositionManager::panic_close_all()` (user-initiated) and `PositionMonitor` (safety net). These bypass RiskGate entirely — they are about getting OUT of positions, not entering them.

---

## 6. "Palma da Mão" / TechnicalStopDistance

### Why TechnicalStopDistance is the correct formalization

The "palma da mão" metaphor captures: "the hand span between your entry and your technical invalidation is the ONE measurement that determines everything."

In code, `TechnicalStopDistance` does exactly this:

1. **User identifies**: entry price + technical stop (from chart analysis)
2. **System derives distance**: `|entry - stop|` (immutable for position lifetime)
3. **Distance determines sizing**: `position_size = max_risk / distance` — wider hand = smaller position
4. **Distance determines trailing**: `trailing_stop = favorable_extreme - distance` — the hand travels with the price

The dual-use is what makes this powerful: it is not two separate concepts that happen to use the same number. It is ONE concept (the technical distance) that has two consequences (sizing and trailing). If you change the distance, both change proportionally.

### Invariants created

1. **Risk amount is constant**: regardless of stop width, the dollar risk per trade is `capital * pct%`. Proven by test `test_position_sizing_risk_stays_constant` at entities.rs:709.
2. **Trailing stop is anchored**: the stop-to-peak distance equals the original entry-to-stop distance. The position "breathes" by the same amount it was born with.
3. **Monotonicity**: trailing stop only moves favorably. `calculate_trailing_stop_long()` returns `max(peak - distance, initial_stop)`. It can never go below the initial stop.
4. **Immutability**: once `TechnicalStopDistance` is set on a position, it never changes. No field in the struct is `&mut`. The trailing stop moves, but the DISTANCE does not.

### What should be documented

Add a doc comment to `TechnicalStopDistance` that explicitly states the dual-use invariant:

```
/// The technical stop distance anchors BOTH position sizing AND trailing stop.
///
/// # Dual-use invariant
/// - **Sizing**: `position_size = max_risk_amount / distance`
/// - **Trailing**: `stop = favorable_extreme - distance` (long) or `+ distance` (short)
///
/// The distance is immutable for the lifetime of the position.
/// It is derived from chart analysis (e.g., 2nd support level on 15m).
/// This is sometimes called the "palm" — the fixed hand-span from entry
/// to invalidation that drives all downstream risk calculations.
```

### What should NOT be renamed

Do not rename `TechnicalStopDistance` to `PalmDistance` or any other metaphorical name. The technical name is correct, self-documenting, and greppable. The metaphor can live in documentation.

---

## 7. Risk Flow Across Position Lifecycle

### Before entry (arm_position → decide_entry)

**Current**: `arm_position()` creates position with `tech_stop_distance`, spawns detector. `decide_entry()` validates signal, runs `TechnicalStopDistance::validate()`, calls `calculate_position_size()`. No portfolio-level check.

**Target**: Between sizing and action emission, `decide_entry()` calls `RiskGate::evaluate()` with the proposed trade and current risk context.

```
Signal → Engine.decide_entry()
  → calculate_position_size()     [per-trade sizing]
  → RiskGate.evaluate(proposed, context)  [portfolio check]
  → if Approved: emit EntrySignalReceived + PlaceEntryOrder
  → if Rejected: emit RiskCheckFailed, return no actions
```

**What must be in the event for replay**:

The `PositionArmed` event currently lacks `tech_stop_distance`. For replay to reconstruct the position's risk parameters, either:
- (a) Add `tech_stop_distance` (or entry_price + initial_stop, from which it's derived) to `PositionArmed`, or
- (b) Rely on `EntrySignalReceived` which already has `entry_price` and `stop_loss`

**Decision**: Option (b) is acceptable for v2.5. The `EntrySignalReceived` event already has the data. For Armed positions that never received a signal (no entry), the tech_stop_distance is not needed for risk (no trade happened). Add `tech_stop_distance` to `PositionArmed` only if we need to reconstruct Armed positions' risk parameters on replay (useful for dashboard but not for risk engine).

### At entry (EntryFilled)

**Current**: `EntryFilled` event contains `fill_price`, `filled_quantity`, `initial_stop`, `fee`. Does NOT contain `tech_stop_distance` or `risk_config` snapshot.

**Target**: Add to `EntryFilled`:
- `tech_stop_distance: Decimal` (the absolute distance, not the full struct — it can be derived from `fill_price` and `initial_stop` which are already present)

Actually, `initial_stop` IS in `EntryFilled`. And `fill_price` IS in `EntryFilled`. So `tech_stop_distance = |fill_price - initial_stop|`. **No event change needed for trailing stop reconstruction.**

What IS missing for risk audit: the `RiskConfig` snapshot (capital, risk_pct) that was used for sizing. Without this, replay can verify the position size but cannot verify it was correct for the risk config at the time.

**Decision**: Add `risk_snapshot_capital: Decimal` and `risk_snapshot_pct: Decimal` to `EntrySignalReceived` event (which is where sizing happens). This enables full audit: "at the time of entry, capital was X, risk was Y%, so position size of Z was correct."

The `binance_position_id` is also missing from events. Add it to `EntryFilled` so that replay can coordinate with Safety Net.

### During position (Active state)

**Current**: `process_active_position()` checks exit condition, updates trailing stop. No portfolio-level check.

**Target**: No change needed. Intra-trade risk is handled by the trailing stop mechanism, which is already correct. The circuit breaker (daily loss) only blocks NEW entries, not existing positions.

Trailing stop events (`TrailingStopUpdated`) already contain `previous_stop`, `new_stop`, `trigger_price` — sufficient for replay.

### At exit (ExitTriggered → PositionClosed)

**Current**: `ExitTriggered` has `reason`, `trigger_price`, `stop_price`. `PositionClosed` has `entry_price`, `exit_price`, `realized_pnl`, `total_fees`.

**Target**: No change needed for risk. `PositionClosed.realized_pnl` is what updates the portfolio risk context for subsequent trades.

### In emergency

**Current**: `PositionManager::panic_close_all()` iterates active positions and closes them. `PositionMonitor` handles rogue positions independently.

**Target**: No change needed. Emergency bypass is correct — it should NOT go through RiskGate (you're reducing risk, not adding it).

**Boundary rule**: Core flow never interferes with Safety Net. `CorePositionOpened/Closed` events are the handshake. If a core position becomes a "rogue" (daemon crashes after entry but before registering CorePositionOpened), the Safety Net will pick it up and apply 2% stop. This is acceptable degraded-mode behavior.

---

## 8. Compatibility with Source of Truth Decision

### EventLog as truth — implications for risk

The risk engine must be replayable. This means:

1. **Every risk-relevant fact must be in an event.** If RiskGate rejects a trade, a `RiskCheckFailed` event is emitted. If it approves, the `EntrySignalReceived` event implicitly records approval (the trade happened).

2. **RiskContext must be derivable from events.** On replay:
   - Capital at any point = initial_capital + sum(realized_pnl) - sum(fees)
   - Open positions = positions with EntryFilled but no PositionClosed
   - Daily PnL = sum of PositionClosed.realized_pnl where closed_at is today

3. **RiskConfig snapshot must be in events.** The capital and risk_pct used for sizing must be recorded in `EntrySignalReceived` so that replay can verify sizing was correct.

### Fields that MUST be added to events

| Event | Field to add | Why |
|---|---|---|
| `EntrySignalReceived` | `risk_capital: Decimal` | Audit: what capital was used for sizing |
| `EntrySignalReceived` | `risk_pct: Decimal` | Audit: what risk % was used |
| `EntryFilled` | `binance_position_id: Option<String>` | Core/SafetyNet coordination on replay |

### Fields that are already sufficient

| Event | Existing fields | What they enable |
|---|---|---|
| `EntryFilled` | `fill_price`, `initial_stop` | Derive tech_stop_distance on replay |
| `EntrySignalReceived` | `entry_price`, `stop_loss`, `quantity` | Verify sizing formula |
| `TrailingStopUpdated` | `previous_stop`, `new_stop`, `trigger_price` | Reconstruct trailing stop history |
| `PositionClosed` | `realized_pnl`, `total_fees` | Update capital for next trade |

### RiskContext reconstruction on replay

```
For each event in sequence:
  EntryFilled → add to open_positions, add notional to total_exposure
  TrailingStopUpdated → update position's unrealized PnL estimate
  PositionClosed → remove from open_positions, update capital, update daily_pnl
```

This means `MemoryStore.apply_event()` must maintain enough state for `RiskContext` to be derived. Today `apply_event_internal` only maintains position state. It would need to also maintain:
- A running capital balance (initial + sum of realized PnL - fees)
- A count of open positions

**Decision**: Do NOT put portfolio-level state in MemoryStore. Instead, RiskGate builds its RiskContext by querying MemoryStore's existing position data:
- `store.positions().find_active()` → count + sum notional for exposure
- Capital tracking: either (a) query from events, or (b) maintain a separate `PortfolioState` in MemoryStore

**Recommendation**: Option (b) — add a minimal `PortfolioState { current_capital: Decimal, daily_pnl: Decimal, daily_pnl_reset_date: NaiveDate }` to MemoryStore, updated by `apply_event` when processing `PositionClosed` events. Simple, no new crate, replay-compatible.

---

## 9. Incremental Plan

### v2.5 — Minimal viable risk gate

**Goal**: A simple, hardcoded RiskGate that catches the most dangerous scenarios.

**Changes**:

1. **New file**: `robson-engine/src/risk.rs`
   - `RiskLimits` struct with hardcoded defaults (max 3 positions, 30% exposure, 3% daily loss)
   - `RiskContext` struct (built from position queries)
   - `RiskVerdict` enum (Approved / Rejected)
   - `RiskGate` struct with `evaluate()` method
   - All pure functions, no I/O, no traits, no generics

2. **Modify**: `robson-engine/src/lib.rs`
   - `Engine` gains a `risk_gate: RiskGate` field (set at construction)
   - `decide_entry()` calls `risk_gate.evaluate()` after sizing, before producing actions
   - On rejection: returns `EngineDecision::no_action()` (or a new variant with rejection info)

3. **New event variant**: Add `RiskCheckFailed` to `robson-domain/src/events.rs`
   - Fields: position_id, check (string), reason (string), proposed_notional, current_exposure, timestamp

4. **Modify**: `robson-domain/src/events.rs`
   - Add `risk_capital: Decimal` and `risk_pct: Decimal` to `EntrySignalReceived`
   - Add `binance_position_id: Option<String>` to `EntryFilled`

5. **Modify**: `robson-exec/src/executor.rs`
   - Remove `FIXED_LEVERAGE` constant (use `RiskConfig::LEVERAGE` from domain to eliminate duplication)

6. **Modify**: `robsond/src/daemon.rs` or `position_manager.rs`
   - Build `RiskContext` from `store.positions().find_active()` before calling engine
   - Pass context to engine (or engine queries store — see tradeoff below)

**What v2.5 does NOT do**:
- No trait/policy abstraction
- No dynamic capital updates (capital stays fixed from config)
- No portfolio state in MemoryStore (RiskContext is built on-demand from position queries)
- No separate risk crate

### v3 — Formal risk engine with dynamic capital

**Changes**:

1. **Add `PortfolioState`** to MemoryStore:
   - `current_capital: Decimal` (updated on PositionClosed via apply_event)
   - `daily_realized_pnl: Decimal` (reset daily)
   - `daily_fees: Decimal`
   - Rebuilt from events on replay

2. **Extract `RiskPolicy` trait** (only if there's a second implementation):
   ```rust
   pub trait RiskPolicy: Send + Sync {
       fn evaluate_pre_trade(&self, proposed: &ProposedTrade, context: &RiskContext) -> RiskVerdict;
   }
   ```
   Implementations: `StandardRiskPolicy` (the v2.5 logic), `BacktestRiskPolicy` (no limits, for simulation).

3. **Dynamic capital**: After each `PositionClosed` event, update `PortfolioState.current_capital`. `RiskContext.capital` reads from `PortfolioState` instead of static config.

4. **Circuit breaker as event**: When daily loss limit is hit, emit `CircuitBreakerTriggered` event. This event, on replay, blocks all subsequent entries for that day.

5. **Risk metrics projection**: `robson-projector/handlers/risk.rs` already has `handle_risk_check_failed()`. Extend to maintain `risk_state_current` projection with running totals.

---

## 10. Tradeoffs

### 1. RiskGate in Engine vs in Executor

**Option A (recommended)**: RiskGate lives in Engine (robson-engine). Engine calls it before producing actions.

**Option B**: RiskGate lives in Executor (robson-exec). Executor checks before placing orders.

**Decision**: A. The Engine is where all decision logic lives. The Executor is for I/O. Risk approval is a decision, not an I/O operation. Putting it in the Engine keeps it testable without async/exchange stubs.

### 2. RiskContext passed in vs Engine queries store

**Option A (recommended)**: Caller (PositionManager) builds RiskContext and passes it to Engine.

**Option B**: Engine holds a reference to Store and queries it.

**Decision**: A. Engine should remain pure (no store dependency). PositionManager already has the store reference and can build the context. This keeps Engine testable with plain structs.

### 3. Static capital (v2.5) vs dynamic capital (v3)

**Decision**: Static capital in v2.5 is acceptable because:
- Single user, few trades per day
- Capital can be updated by restarting daemon with new config
- Dynamic capital requires PortfolioState in MemoryStore + event replay, which is v3 scope

The risk is underestimating exposure after losses. With 1% risk per trade and max 3 positions, the maximum error is ~3% of actual capital, which is within tolerance for v2.5.

### 4. Separate risk crate vs module in engine

**Decision**: Module in engine (v2.5 and v3). Risk is inseparable from decision logic. A separate crate would add dependency management overhead for a module that has exactly one consumer (Engine). Only extract if a third consumer appears.

### 5. Duplicate leverage constant

`RiskConfig::LEVERAGE = 10` (domain) and `executor::FIXED_LEVERAGE = 10` (exec) must be unified. The domain constant is authoritative; the exec constant should reference it.

---

## 11. Decisions That Need User Input

### Decision 1: Max open positions limit

**Recommendation**: 3 positions max for v2.5.

**Rationale**: With 1% risk per trade and 10x leverage, 3 positions means ~30% margin utilization. Conservative enough for single-user operation, permissive enough to not block normal trading.

**User action**: Confirm or adjust. This becomes a field in `RiskLimits`.

### Decision 2: Daily loss circuit breaker threshold

**Recommendation**: 3% of capital.

**Rationale**: At 1% risk per trade, 3 consecutive losses hit this limit. This prevents emotional revenge trading while allowing normal losing streaks.

**User action**: Confirm or adjust.

### Decision 3: Should RiskGate rejection be a hard error or a soft warning?

**Recommendation**: Hard rejection. `decide_entry()` returns no entry actions. A `RiskCheckFailed` event is emitted for audit.

**Rationale**: The core principle is "USER initiates → ROBSON calculates → USER confirms." If Robson determines the trade violates risk limits, it should refuse, not warn. The user can adjust parameters and retry.

**User action**: Confirm. This affects whether the API returns an error or a warning.

### Decision 4: Event schema changes — when to apply?

Adding `risk_capital` and `risk_pct` to `EntrySignalReceived`, and `binance_position_id` to `EntryFilled`, are breaking changes to the event enum.

**Recommendation**: Apply in the same batch as the compilation fixes (the daemon doesn't compile anyway, so no backward compatibility concern). The PostgreSQL event_log is empty in production (events were never written there), so no migration needed.

**User action**: Confirm that there is no production data in the event_log table that would be affected by schema changes.

### Decision 5: Should RiskConfig.capital be queryable from Binance balance?

**Recommendation**: Not in v2.5. In v3, derive capital from events (initial deposit + realized PnL).

**Rationale**: Querying Binance for balance introduces I/O into the risk path. And the balance may include funds not allocated to Robson. Better to track capital internally via events.

**User action**: Confirm approach. If you want Binance balance as source of truth for capital, the design changes significantly (risk path becomes async).
