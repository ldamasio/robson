# Robson v2 Domain Model

**Version**: 2.0.0-alpha
**Last Updated**: 2026-01-16
**Status**: Implementation Phase (Phases 0-3 Complete)

---

## Table of Contents

1. [Core Concepts](#core-concepts)
2. [Entities](#entities)
3. [Value Objects](#value-objects)
4. [State Machine](#state-machine)
5. [Technical Stop Distance](#technical-stop-distance)
6. [Trailing Stop Strategy](#trailing-stop-strategy)
7. [Position Sizing](#position-sizing)
8. [Detector Architecture](#detector-architecture)
9. [Risk Management](#risk-management)
10. [Events](#events)
11. [Invariants](#invariants)

---

## Core Concepts

### User-Initiated, System-Managed

**Key Principle**: User arms positions; system decides entries/exits based on detector signals and risk rules.

- **User**: Chooses symbol, strategy, capital allocation, arms position
- **Detector**: Monitors market, fires single entry signal when conditions met
- **Engine**: Validates signal, calculates position size, manages trailing stop
- **Executor**: Places orders on exchange with idempotency guarantees

### Technical Stop Distance (formerly "Palma da Mão")

**Definition**: Distance between entry price and technical invalidation level (stop loss from chart analysis).

**Why Universal?**:
- Structural foundation for position sizing (Golden Rule)
- Risk is ALWAYS defined by technical invalidation level
- NOT arbitrary percentage or dollar amount
- Position size is DERIVED from this distance

**Formula**:
```
Tech Stop Distance = |Entry Price - Technical Stop Loss|
Tech Stop % = (Distance / Entry Price) × 100
```

**Example**:
```
Entry: $95,000
Technical SL: $93,500
Distance: $1,500 (1.58%)
```

### Trailing Stop (Single Exit Mechanism)

**Key Decision**: Robson v2 uses a **single trailing stop** instead of separate stop-loss and stop-gain.

**How it works**:
1. Initial trailing stop = Entry price - Tech stop distance (for Long)
2. When price moves favorably, trailing stop follows by tech stop distance
3. When price hits trailing stop, exit triggers

**Benefits**:
- Captures more profit in trending markets
- Maintains consistent risk per trade
- Simpler logic, fewer edge cases

### All Exits Are Market Orders

**Rule**: NO limit orders for exits

**Rationale**:
- Guarantee execution (no slippage risk denial)
- Market moves fast; we need certainty
- Trailing stop provides dynamic target

### No Insurance Stop on Exchange

**Decision**: Robson manages all exits in runtime. No backup stop-limit on exchange.

**Rationale**:
- Simpler architecture
- Avoids race conditions between local monitor and exchange stop
- Daemon reliability is the safety mechanism

---

## Entities

### Position

**Definition**: A managed trading position with full lifecycle management.

```rust
pub struct Position {
    // Identity
    pub id: PositionId,
    pub account_id: AccountId,
    pub symbol: Symbol,
    pub side: Side,
    pub state: PositionState,

    // Entry
    pub entry_price: Option<Price>,
    pub entry_filled_at: Option<DateTime<Utc>>,

    // Risk Parameters
    pub tech_stop_distance: Option<TechnicalStopDistance>,
    pub quantity: Quantity,

    // P&L Tracking
    pub realized_pnl: Decimal,
    pub fees_paid: Decimal,

    // Orders
    pub entry_order_id: Option<OrderId>,
    pub exit_order_id: Option<OrderId>,

    // Audit
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

impl Position {
    pub fn new(account_id: AccountId, symbol: Symbol, side: Side) -> Self;
    pub fn can_enter(&self) -> bool;  // true if Armed
    pub fn can_exit(&self) -> bool;   // true if Active
    pub fn is_closed(&self) -> bool;  // true if Closed
}
```

**Key Design Notes**:
- No `leverage` field (fixed 10x for Binance isolated margin)
- No `stop_gain` field (trailing stop handles exits)
- `tech_stop_distance` captures the risk distance

### Order

**Definition**: An instruction to buy/sell on the exchange.

```rust
pub struct Order {
    pub id: OrderId,
    pub position_id: PositionId,
    pub exchange_order_id: Option<String>,
    pub client_order_id: String,  // UUID v7 for idempotency

    pub symbol: Symbol,
    pub side: OrderSide,
    pub order_type: OrderType,
    pub quantity: Quantity,
    pub price: Option<Price>,     // None for market orders

    pub status: OrderStatus,

    // Fill information (consolidated from Trade)
    pub filled_quantity: Option<Quantity>,
    pub fill_price: Option<Price>,
    pub filled_at: Option<DateTime<Utc>>,
    pub fee_paid: Option<Decimal>,

    pub created_at: DateTime<Utc>,
}

pub enum OrderType {
    Market,
    Limit,
    StopLossLimit,
}

pub enum OrderStatus {
    Pending,      // Created locally, not sent yet
    Submitted,    // Sent to exchange
    PartialFill,  // Partially filled
    Filled,       // Completely filled
    Cancelled,    // Cancelled
    Rejected,     // Rejected by exchange
}
```

**Design Note**: `Trade` entity was removed. Fill information is stored directly in `Order` since market orders fill immediately in isolated margin.

### DetectorSignal

**Definition**: Single-shot signal from a detector to trigger entry.

```rust
pub struct DetectorSignal {
    /// Unique signal identifier for idempotency
    pub signal_id: Uuid,
    /// Position this signal belongs to (detector is per-position)
    pub position_id: PositionId,
    /// Trading pair symbol
    pub symbol: Symbol,
    /// Position direction (must match armed position)
    pub side: Side,
    /// Suggested entry price (current market price when signal fired)
    pub entry_price: Price,
    /// Technical stop loss from chart analysis
    pub stop_loss: Price,
    /// When the signal was generated
    pub timestamp: DateTime<Utc>,
}

impl DetectorSignal {
    pub fn new(position_id, symbol, side, entry_price, stop_loss) -> Self;
    pub fn tech_stop_distance(&self) -> TechnicalStopDistance;
    pub fn validate_for_position(&self, position: &Position) -> Result<(), DomainError>;
}
```

**Key Design Notes**:
- `signal_id` enables idempotent processing (same signal processed only once)
- No `stop_gain` - trailing stop handles profit taking
- Detector is **per-position** (not market scanner)

---

## Value Objects

### TechnicalStopDistance

**Renamed from**: `PalmaDaMao` (clearer, self-documenting name)

```rust
pub struct TechnicalStopDistance {
    distance: Decimal,         // Absolute distance in quote currency
    distance_percent: Decimal, // Percentage of entry price
}

impl TechnicalStopDistance {
    pub fn from_entry_and_stop(entry: Price, stop_loss: Price) -> Self;

    pub fn validate(&self) -> Result<(), DomainError> {
        // Too tight: < 0.1%
        // Too wide: > 10%
    }

    /// Calculate initial trailing stop for Long position
    pub fn calculate_trailing_stop_long(&self, current_price: Decimal) -> Price;

    /// Calculate initial trailing stop for Short position
    pub fn calculate_trailing_stop_short(&self, current_price: Decimal) -> Price;
}
```

**Validation Rules**:
- Minimum: 0.1% (prevents noise-triggered exits)
- Maximum: 10% (prevents excessive risk)

### RiskConfig

```rust
pub struct RiskConfig {
    capital: Decimal,           // Total capital allocated
    risk_percent: Decimal,      // Risk per trade (default: 1%)
    max_drawdown_percent: Decimal, // Max account drawdown (default: 10%)
}

impl RiskConfig {
    pub fn new(capital: Decimal, risk_percent: Decimal) -> Result<Self, DomainError>;
    pub fn max_risk_amount(&self) -> Decimal;  // capital × risk_percent / 100
}
```

**Design Note**: No `max_leverage` - leverage is fixed at 10x.

### Price

```rust
pub struct Price(Decimal);

impl Price {
    pub fn new(value: Decimal) -> Result<Self, DomainError>;  // Must be > 0
    pub fn as_decimal(&self) -> Decimal;
}
```

### Quantity

```rust
pub struct Quantity(Decimal);

impl Quantity {
    pub fn new(value: Decimal) -> Result<Self, DomainError>;  // Must be > 0
    pub fn as_decimal(&self) -> Decimal;
    pub fn zero() -> Self;
}
```

### Symbol

```rust
pub struct Symbol {
    base: String,    // e.g., "BTC"
    quote: String,   // e.g., "USDT"
}

impl Symbol {
    pub fn from_pair(pair: &str) -> Result<Self, DomainError>;
    pub fn as_pair(&self) -> String;  // "BTCUSDT"
}
```

### Side

```rust
pub enum Side {
    Long,   // Buy to open, sell to close
    Short,  // Sell to open, buy to close
}

impl Side {
    pub fn entry_action(&self) -> OrderSide;  // Long→Buy, Short→Sell
    pub fn exit_action(&self) -> OrderSide;   // Long→Sell, Short→Buy
}
```

---

## State Machine

```
┌──────────┐
│  Armed   │  (Waiting for detector signal)
└────┬─────┘
     │ DetectorSignal received
     │ → validate signal
     │ → calculate position size
     │ → place entry order
     ▼
┌──────────┐
│ Entering │  (Entry market order placed, waiting for fill)
│          │  Contains: entry_order_id, expected_entry, signal_id
└────┬─────┘
     │ Entry order filled
     │ → set initial trailing stop
     │ → start monitoring
     ▼
┌──────────┐
│  Active  │  (Monitoring price, managing trailing stop)
│          │  Contains: current_price, trailing_stop, favorable_extreme
└────┬─────┘
     │ Price hits trailing stop
     │ → place exit order
     ▼
┌──────────┐
│ Exiting  │  (Exit market order placed, waiting for fill)
│          │  Contains: exit_order_id, exit_reason
└────┬─────┘
     │ Exit order filled
     │ → calculate PnL
     ▼
┌──────────┐
│  Closed  │  (Position closed, PnL realized)
│          │  Contains: exit_price, realized_pnl, exit_reason
└──────────┘

     │ Error at any stage
     ▼
┌──────────┐
│  Error   │  (Manual intervention required)
│          │  Contains: error message, recoverable flag
└──────────┘
```

### State Definitions

```rust
pub enum PositionState {
    /// Position armed, waiting for detector signal
    Armed,

    /// Entry order submitted, waiting for fill
    Entering {
        entry_order_id: OrderId,
        expected_entry: Price,
        signal_id: Uuid,  // For idempotency
    },

    /// Position active, monitoring trailing stop
    Active {
        current_price: Price,
        trailing_stop: Price,
        favorable_extreme: Price,  // Highest (Long) or lowest (Short) price seen
        extreme_at: DateTime<Utc>,
        insurance_stop_id: Option<OrderId>,  // Always None in v2 (no insurance stop)
    },

    /// Exit order submitted, waiting for fill
    Exiting {
        exit_order_id: OrderId,
        exit_reason: ExitReason,
    },

    /// Position closed, PnL realized
    Closed {
        exit_price: Price,
        realized_pnl: Decimal,
        exit_reason: ExitReason,
    },

    /// Error state, requires intervention
    Error {
        error: String,
        recoverable: bool,
    },
}

pub enum ExitReason {
    TrailingStop,   // Price hit trailing stop
    UserPanic,      // User triggered emergency exit
    Error,          // System error forced exit
}
```

---

## Technical Stop Distance

### Calculation

```rust
pub fn calculate_tech_stop_distance(
    entry_price: Price,
    stop_loss: Price,
) -> Result<TechnicalStopDistance, DomainError> {
    let distance = (entry_price.as_decimal() - stop_loss.as_decimal()).abs();
    let distance_percent = distance / entry_price.as_decimal() * dec!(100);

    let tech_stop = TechnicalStopDistance::new(distance, distance_percent);
    tech_stop.validate()?;

    Ok(tech_stop)
}
```

### Validation

| Constraint | Value | Reason |
|------------|-------|--------|
| Minimum | 0.1% | Prevents noise-triggered exits |
| Maximum | 10% | Prevents excessive risk per trade |

### Example

```
Entry Price: $95,000
Technical SL: $93,500 (from chart: 2nd support level on 15m)

Distance: $1,500
Distance %: 1.58%

✅ Valid (0.1% < 1.58% < 10%)
```

---

## Trailing Stop Strategy

### How It Works

The trailing stop **follows** favorable price movement by maintaining a fixed distance (tech stop distance) from the most favorable price seen.

#### For Long Positions

```
Initial: trailing_stop = entry_price - tech_distance
Update:  IF current_price > favorable_extreme THEN
           favorable_extreme = current_price
           trailing_stop = current_price - tech_distance
Exit:    IF current_price <= trailing_stop THEN trigger exit
```

#### For Short Positions

```
Initial: trailing_stop = entry_price + tech_distance
Update:  IF current_price < favorable_extreme THEN
           favorable_extreme = current_price
           trailing_stop = current_price + tech_distance
Exit:    IF current_price >= trailing_stop THEN trigger exit
```

### Example: Long Position

```
Entry: $95,000
Tech Distance: $1,500
Initial Stop: $93,500

Price moves to $96,000 (new high!)
→ favorable_extreme = $96,000
→ trailing_stop = $96,000 - $1,500 = $94,500

Price moves to $97,000 (new high!)
→ favorable_extreme = $97,000
→ trailing_stop = $97,000 - $1,500 = $95,500

Price drops to $95,500 (hits trailing stop)
→ EXIT TRIGGERED
→ Profit: $95,500 - $95,000 = $500

Without trailing: would have exited at fixed stop gain or held longer
With trailing: captured $2,000 of the $2,000 move, then protected $500 profit
```

### Why Trailing Stop Instead of Fixed Stop Gain?

| Fixed Stop Gain | Trailing Stop |
|-----------------|---------------|
| Exits at predetermined target | Follows favorable movement |
| May exit too early in trends | Captures more profit in trends |
| Requires predicting target | No prediction needed |
| Two parameters (SL + SG) | One parameter (tech distance) |

---

## Position Sizing

### Golden Rule

**Position size is DERIVED from technical stop, NOT chosen arbitrarily.**

```
Position Size = Max Risk Amount / Tech Stop Distance
             = (Capital × Risk%) / |Entry - Stop|
```

### Implementation

```rust
pub fn calculate_position_size(
    risk_config: &RiskConfig,
    tech_stop: &TechnicalStopDistance,
) -> Result<Quantity, DomainError> {
    // 1. Validate tech stop
    tech_stop.validate()?;

    // 2. Calculate max risk amount
    let max_risk = risk_config.max_risk_amount();

    // 3. Calculate position size
    let size = max_risk / tech_stop.distance();

    // 4. Validate minimum
    if size < Decimal::from_str("0.00001")? {
        return Err(DomainError::PositionSizingError("Size too small"));
    }

    Ok(Quantity::new(size)?)
}
```

### Example

```
Capital: $10,000
Risk per trade: 1%
Entry: $95,000
Technical SL: $93,500

Max Risk: $10,000 × 1% = $100
Tech Distance: $1,500
Position Size: $100 / $1,500 = 0.0667 BTC

Notional Value: 0.0667 × $95,000 = $6,333

If stopped at $93,500:
  Loss = 0.0667 × $1,500 = $100 = 1% ✓
```

---

## Detector Architecture

### Per-Position Watchers

Detectors are **per-position watchers**, NOT market scanners.

```
┌─────────────────────────────────────────────────────────────────┐
│                         DAEMON                                  │
│                                                                 │
│   Position Armed                                                │
│        │                                                        │
│        ▼                                                        │
│   ┌─────────────────────────────────────────────────────────┐  │
│   │              DetectorTask(position_id)                   │  │
│   │                                                          │  │
│   │  1. Load position context (symbol, side, strategy)       │  │
│   │  2. Subscribe to market data feed                        │  │
│   │  3. Evaluate entry conditions in loop                    │  │
│   │  4. On condition met: emit ONE DetectorSignal            │  │
│   │  5. Die (single-shot)                                    │  │
│   │                                                          │  │
│   └──────────────────────────┬──────────────────────────────┘  │
│                              │                                  │
│                              ▼                                  │
│                    DetectorSignal {                             │
│                      signal_id,      // UUID for idempotency    │
│                      position_id,    // Which position          │
│                      entry_price,    // Current market price    │
│                      stop_loss,      // From chart analysis     │
│                    }                                            │
│                              │                                  │
│                              ▼                                  │
│                         Event Bus                               │
│                              │                                  │
│                              ▼                                  │
│                          Engine                                 │
│                    (validates, sizes, executes)                 │
└─────────────────────────────────────────────────────────────────┘
```

### Lifecycle

| Event | Detector Action |
|-------|-----------------|
| Position Armed | Daemon spawns DetectorTask |
| Condition Met | Detector emits ONE signal, then dies |
| Position Cancelled | Daemon kills DetectorTask |
| Position Enters | DetectorTask already dead (fired signal) |

### Idempotency

The `signal_id` in `DetectorSignal` ensures:
1. Engine checks if signal already processed before transitioning
2. Duplicate signals (e.g., from retry) are safely ignored
3. `Entering` state stores `signal_id` for double-check

```rust
// In Engine::decide_entry
if position.state == Entering { signal_id } && signal_id == new_signal.signal_id {
    return Ok(EngineDecision::no_action());  // Already processed
}
```

---

## Risk Management

### Risk Configuration

```rust
pub struct RiskConfig {
    pub capital: Decimal,              // Total allocated capital
    pub risk_percent: Decimal,         // Risk per trade (default: 1%)
    pub max_drawdown_percent: Decimal, // Max account drawdown (default: 10%)
}
```

**Removed from original design**:
- `max_leverage` - Fixed at 10x (Binance isolated margin)
- `insurance_stop_enabled` - No insurance stops in v2

### Risk Checks (Future: Phase 5)

```rust
impl RiskManager {
    /// Check if new position can be opened
    pub fn can_open_position(&self, account: &Account) -> Result<(), RiskError> {
        // 1. Check daily drawdown limit
        // 2. Check max open positions
        // 3. Check account equity
    }
}
```

---

## Events

### Event Types

```rust
pub enum Event {
    // === Position Lifecycle ===

    /// Position created and armed
    PositionArmed {
        position_id: PositionId,
        account_id: AccountId,
        symbol: Symbol,
        side: Side,
        timestamp: DateTime<Utc>,
    },

    /// Detector fired entry signal
    EntrySignalReceived {
        position_id: PositionId,
        signal_id: Uuid,
        entry_price: Price,
        stop_loss: Price,
        quantity: Quantity,
        timestamp: DateTime<Utc>,
    },

    /// Entry order placed on exchange
    EntryOrderPlaced {
        position_id: PositionId,
        order_id: OrderId,
        symbol: Symbol,
        side: OrderSide,
        quantity: Quantity,
        timestamp: DateTime<Utc>,
    },

    /// Entry order filled
    EntryFilled {
        position_id: PositionId,
        order_id: OrderId,
        fill_price: Price,
        filled_quantity: Quantity,
        fee: Decimal,
        initial_stop: Price,  // Initial trailing stop
        timestamp: DateTime<Utc>,
    },

    /// Trailing stop updated (price moved favorably)
    TrailingStopUpdated {
        position_id: PositionId,
        previous_stop: Price,
        new_stop: Price,
        trigger_price: Price,  // Price that triggered update
        timestamp: DateTime<Utc>,
    },

    /// Exit triggered (trailing stop hit)
    ExitTriggered {
        position_id: PositionId,
        trigger_price: Price,
        stop_price: Price,
        reason: ExitReason,
        timestamp: DateTime<Utc>,
    },

    /// Exit order placed
    ExitOrderPlaced {
        position_id: PositionId,
        order_id: OrderId,
        symbol: Symbol,
        side: OrderSide,
        quantity: Quantity,
        reason: ExitReason,
        timestamp: DateTime<Utc>,
    },

    /// Exit order filled
    ExitFilled {
        position_id: PositionId,
        order_id: OrderId,
        fill_price: Price,
        filled_quantity: Quantity,
        fee: Decimal,
        timestamp: DateTime<Utc>,
    },

    /// Position closed
    PositionClosed {
        position_id: PositionId,
        entry_price: Price,
        exit_price: Price,
        quantity: Quantity,
        realized_pnl: Decimal,
        fees_paid: Decimal,
        exit_reason: ExitReason,
        duration_seconds: u64,
        timestamp: DateTime<Utc>,
    },

    /// Position error
    PositionError {
        position_id: PositionId,
        error: String,
        recoverable: bool,
        timestamp: DateTime<Utc>,
    },
}
```

### Event Accessors

```rust
impl Event {
    pub fn position_id(&self) -> PositionId;
    pub fn timestamp(&self) -> DateTime<Utc>;
    pub fn event_type(&self) -> &'static str;
}
```

---

## Invariants

### Position Invariants

1. **Entry price must be positive**: `entry_price > 0`
2. **Tech stop must be valid**: `0.1% <= distance_percent <= 10%`
3. **Quantity must be positive**: `quantity > 0`
4. **State transitions must be valid**: Armed → Entering → Active → Exiting → Closed
5. **Signal ID is unique**: No duplicate signal processing

### System Invariants

1. **One detector per armed position**: Detector spawned on arm, killed on transition
2. **Single-shot signals**: Each detector emits at most ONE signal
3. **Idempotent execution**: Same signal_id processed only once
4. **All exits via system**: No manual closes (trailing stop, panic, or error)

### Trailing Stop Invariants

1. **Never moves against position**:
   - Long: trailing stop only increases
   - Short: trailing stop only decreases
2. **Distance is constant**: Always equals tech_stop_distance
3. **Exit is immediate**: When price hits stop, exit triggers (no delay)

---

## Validation Checklist

Domain model implementation status:

- [x] All entities have clear identity and lifecycle
- [x] All value objects are immutable and validated
- [x] State machine transitions are exhaustive
- [x] Technical stop distance calculation is correct
- [x] Trailing stop logic handles Long and Short
- [x] Position sizing formula matches Golden Rule
- [x] DetectorSignal includes signal_id for idempotency
- [x] Events capture all state changes
- [x] 73 tests validate all business logic

---

## Implementation Reference

| Concept | File | Tests |
|---------|------|-------|
| Value Objects | `robson-domain/src/value_objects.rs` | 22 |
| Entities | `robson-domain/src/entities.rs` | 10 |
| Events | `robson-domain/src/events.rs` | 6 |
| Engine (Entry) | `robson-engine/src/lib.rs` | 9 |
| Engine (Exit) | `robson-engine/src/lib.rs` | 12 |
| Repository | `robson-store/src/repository.rs` | - |
| Memory Store | `robson-store/src/memory.rs` | 14 |

---

**Next**: See [EXECUTION-PLAN.md](./EXECUTION-PLAN.md) for implementation roadmap (Phase 4+)
