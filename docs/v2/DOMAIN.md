# Robson v2 Domain Model

**Version**: 2.0.0-alpha
**Last Updated**: 2026-01-12
**Status**: Planning Phase

---

## Table of Contents

1. [Core Concepts](#core-concepts)
2. [Entities](#entities)
3. [Value Objects](#value-objects)
4. [State Machine](#state-machine)
5. [Palma da Mão (Technical Stop Distance)](#palma-da-mão-technical-stop-distance)
6. [Position Sizing](#position-sizing)
7. [Risk Management](#risk-management)
8. [Invariants](#invariants)
9. [Events](#events)

---

## Core Concepts

### User-Initiated, System-Managed

**Key Principle**: User arms positions; system decides entries/exits based on technical analysis and risk rules.

- **User**: Chooses symbol, strategy, capital allocation
- **System**: Decides entry price, stop loss, stop gain, position size, exit timing

### "Palma da Mão" (Palm of the Hand)

**Definition**: Distance between entry price and technical stop loss

**Why Universal?**:
- Structural foundation for position sizing
- Risk is ALWAYS defined by technical invalidation level
- NOT arbitrary percentage or dollar amount

**Formula**:
```
Palma = |Entry Price - Technical Stop Loss|
Palma % = (Palma / Entry Price) × 100
```

**Example**:
```
Entry: $95,000
Technical SL: $93,500
Palma: $1,500 (1.58%)
```

### All Exits Are Market Orders

**Rule**: NO limit orders for exits (SL/SG)

**Rationale**:
- Guarantee execution (no slippage risk denial)
- Market moves fast; we need certainty
- Profit target is guidance, not hard requirement

---

## Entities

### Position

**Definition**: A managed trading position with lifecycle management

```rust
pub struct Position {
    pub id: PositionId,
    pub account_id: AccountId,
    pub symbol: Symbol,
    pub side: Side,                 // Long or Short
    pub state: PositionState,
    pub strategy: StrategyConfig,

    // Entry
    pub entry_price: Option<Price>,
    pub entry_filled_at: Option<DateTime<Utc>>,

    // Risk Parameters
    pub palma: Option<PalmaDaMao>,
    pub stop_loss: Price,
    pub stop_gain: Price,
    pub quantity: Quantity,
    pub leverage: Leverage,

    // P&L Tracking
    pub realized_pnl: Decimal,
    pub fees_paid: Decimal,

    // Orders
    pub entry_order_id: Option<OrderId>,
    pub exit_order_id: Option<OrderId>,
    pub insurance_stop_id: Option<OrderId>,

    // Audit
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

impl Position {
    pub fn new(
        account_id: AccountId,
        symbol: Symbol,
        side: Side,
        strategy: StrategyConfig,
    ) -> Self {
        Self {
            id: PositionId::new(),
            account_id,
            symbol,
            side,
            state: PositionState::Armed { detector_config: strategy.detector },
            strategy,
            entry_price: None,
            entry_filled_at: None,
            palma: None,
            stop_loss: Price::zero(),
            stop_gain: Price::zero(),
            quantity: Quantity::zero(),
            leverage: Leverage::one(),
            realized_pnl: Decimal::ZERO,
            fees_paid: Decimal::ZERO,
            entry_order_id: None,
            exit_order_id: None,
            insurance_stop_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            closed_at: None,
        }
    }

    pub fn can_enter(&self) -> bool {
        matches!(self.state, PositionState::Armed { .. })
    }

    pub fn can_exit(&self) -> bool {
        matches!(self.state, PositionState::Active { .. })
    }

    pub fn is_closed(&self) -> bool {
        matches!(self.state, PositionState::Closed { .. })
    }
}
```

### Order

**Definition**: An instruction to buy/sell on the exchange

```rust
pub struct Order {
    pub id: OrderId,
    pub position_id: PositionId,
    pub exchange_order_id: Option<String>,
    pub client_order_id: String,  // intent_id

    pub symbol: Symbol,
    pub side: Side,
    pub order_type: OrderType,
    pub quantity: Quantity,
    pub price: Option<Price>,     // None for market orders

    pub status: OrderStatus,
    pub filled_quantity: Quantity,
    pub average_fill_price: Option<Price>,

    pub created_at: DateTime<Utc>,
    pub filled_at: Option<DateTime<Utc>>,
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
    Cancelled,    // Cancelled by user or system
    Rejected,     // Rejected by exchange
    Expired,      // Expired (GTC not used)
}
```

### Trade

**Definition**: An executed fill (part of an order)

```rust
pub struct Trade {
    pub id: TradeId,
    pub order_id: OrderId,
    pub exchange_trade_id: String,

    pub symbol: Symbol,
    pub side: Side,
    pub quantity: Quantity,
    pub price: Price,
    pub fee: Decimal,
    pub fee_asset: String,

    pub executed_at: DateTime<Utc>,
}
```

---

## Value Objects

### PalmaDaMao

```rust
pub struct PalmaDaMao {
    pub distance: Decimal,         // Absolute distance in quote currency
    pub distance_pct: Decimal,     // Percentage of entry price
    pub entry_price: Price,
    pub stop_loss: Price,
}

impl PalmaDaMao {
    pub fn from_entry_and_stop(entry: Price, stop_loss: Price) -> Self {
        let distance = (entry.as_decimal() - stop_loss.as_decimal()).abs();
        let distance_pct = distance / entry.as_decimal() * Decimal::from(100);

        Self {
            distance,
            distance_pct,
            entry_price: entry,
            stop_loss,
        }
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        if self.distance <= Decimal::ZERO {
            return Err(DomainError::InvalidPalma("Distance must be positive"));
        }

        if self.distance_pct > Decimal::from(10) {
            return Err(DomainError::InvalidPalma("Stop too wide (>10%)"));
        }

        if self.distance_pct < Decimal::new(1, 1) {  // 0.1%
            return Err(DomainError::InvalidPalma("Stop too tight (<0.1%)"));
        }

        Ok(())
    }
}
```

### Price

```rust
pub struct Price(Decimal);

impl Price {
    pub fn new(value: Decimal) -> Result<Self, DomainError> {
        if value <= Decimal::ZERO {
            return Err(DomainError::InvalidPrice("Price must be positive"));
        }
        Ok(Self(value))
    }

    pub fn as_decimal(&self) -> Decimal {
        self.0
    }

    pub fn zero() -> Self {
        Self(Decimal::ZERO)
    }
}
```

### Quantity

```rust
pub struct Quantity(Decimal);

impl Quantity {
    pub fn new(value: Decimal) -> Result<Self, DomainError> {
        if value <= Decimal::ZERO {
            return Err(DomainError::InvalidQuantity("Quantity must be positive"));
        }
        Ok(Self(value))
    }

    pub fn as_decimal(&self) -> Decimal {
        self.0
    }

    pub fn zero() -> Self {
        Self(Decimal::ZERO)
    }
}
```

### Symbol

```rust
pub struct Symbol {
    pub base: String,    // e.g., "BTC"
    pub quote: String,   // e.g., "USDT"
}

impl Symbol {
    pub fn from_pair(pair: &str) -> Result<Self, DomainError> {
        // Parse "BTCUSDT" → base: "BTC", quote: "USDT"
        // (Implementation with known quote assets: USDT, USDC, BTC, ETH)
    }

    pub fn as_pair(&self) -> String {
        format!("{}{}", self.base, self.quote)
    }
}
```

### Side

```rust
pub enum Side {
    Long,   // Buy to open, sell to close
    Short,  // Sell to open, buy to close
}

impl Side {
    pub fn entry_action(&self) -> OrderSide {
        match self {
            Side::Long => OrderSide::Buy,
            Side::Short => OrderSide::Sell,
        }
    }

    pub fn exit_action(&self) -> OrderSide {
        match self {
            Side::Long => OrderSide::Sell,
            Side::Short => OrderSide::Buy,
        }
    }
}
```

### Leverage

```rust
pub struct Leverage(u8);

impl Leverage {
    pub fn new(value: u8) -> Result<Self, DomainError> {
        if value == 0 || value > 10 {
            return Err(DomainError::InvalidLeverage("Leverage must be 1-10"));
        }
        Ok(Self(value))
    }

    pub fn as_u8(&self) -> u8 {
        self.0
    }

    pub fn one() -> Self {
        Self(1)
    }
}
```

---

## State Machine

```
┌──────────┐
│  Armed   │  (Waiting for detector signal)
└────┬─────┘
     │ detector_signal(entry_price, side)
     ▼
┌──────────┐
│ Entering │  (Entry market order placed)
└────┬─────┘
     │ entry_filled(fill_price, quantity)
     ▼
┌──────────┐
│  Active  │  (Monitoring SL/SG)
└────┬─────┘
     │ trigger_exit(reason) OR user_panic()
     ▼
┌──────────┐
│ Exiting  │  (Exit market order placed)
└────┬─────┘
     │ exit_filled(fill_price)
     ▼
┌──────────┐
│  Closed  │  (PnL calculated)
└──────────┘

     │ error_at_any_stage()
     ▼
┌──────────┐
│  Error   │  (Manual intervention required)
└──────────┘
```

### State Definitions

```rust
pub enum PositionState {
    /// Position armed, waiting for entry signal
    Armed {
        detector_config: DetectorConfig,
    },

    /// Entry order submitted, waiting for fill
    Entering {
        entry_order_id: OrderId,
        expected_entry: Price,
    },

    /// Position active, monitoring stop loss/gain
    Active {
        monitor_active: bool,
        last_price: Price,
        insurance_stop_id: Option<OrderId>,
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

    /// Error state, requires manual intervention
    Error {
        error: DomainError,
        recoverable: bool,
    },
}

pub enum ExitReason {
    StopLoss,
    StopGain,
    UserPanic,
    DegradedMode,
    InsuranceStop,
}
```

### State Transitions

```rust
impl Position {
    pub fn apply_detector_signal(
        &mut self,
        signal: DetectorSignal,
    ) -> Result<EngineAction, DomainError> {
        match self.state {
            PositionState::Armed { .. } => {
                // Calculate palma, stop loss, stop gain, position size
                let palma = PalmaDaMao::from_entry_and_stop(
                    signal.entry_price,
                    signal.stop_loss,
                );
                palma.validate()?;

                let position_size = self.calculate_position_size(
                    signal.entry_price,
                    signal.stop_loss,
                )?;

                // Transition to Entering
                self.state = PositionState::Entering {
                    entry_order_id: OrderId::new(),
                    expected_entry: signal.entry_price,
                };
                self.palma = Some(palma);
                self.entry_price = Some(signal.entry_price);
                self.stop_loss = signal.stop_loss;
                self.stop_gain = signal.stop_gain;
                self.quantity = position_size;

                Ok(EngineAction::PlaceOrder(OrderIntent {
                    symbol: self.symbol.clone(),
                    side: self.side.entry_action(),
                    quantity: position_size,
                    order_type: OrderType::Market,
                }))
            }
            _ => Err(DomainError::InvalidStateTransition {
                from: self.state.clone(),
                event: "detector_signal",
            }),
        }
    }

    pub fn apply_entry_filled(
        &mut self,
        fill: OrderFill,
    ) -> Result<EngineAction, DomainError> {
        match self.state {
            PositionState::Entering { .. } => {
                // Transition to Active
                self.state = PositionState::Active {
                    monitor_active: true,
                    last_price: fill.price,
                    insurance_stop_id: None,
                };
                self.entry_filled_at = Some(Utc::now());

                // Place insurance stop (optional)
                if self.strategy.insurance_stop_enabled {
                    let insurance_stop = self.calculate_insurance_stop()?;
                    return Ok(EngineAction::PlaceOrder(insurance_stop));
                }

                Ok(EngineAction::EmitEvent(Event::PositionActivated {
                    position_id: self.id,
                    entry_price: fill.price,
                }))
            }
            _ => Err(DomainError::InvalidStateTransition {
                from: self.state.clone(),
                event: "entry_filled",
            }),
        }
    }

    pub fn apply_stop_loss_trigger(
        &mut self,
        current_price: Price,
    ) -> Result<EngineAction, DomainError> {
        match self.state {
            PositionState::Active { .. } => {
                // Transition to Exiting
                self.state = PositionState::Exiting {
                    exit_order_id: OrderId::new(),
                    exit_reason: ExitReason::StopLoss,
                };

                Ok(EngineAction::PlaceOrder(OrderIntent {
                    symbol: self.symbol.clone(),
                    side: self.side.exit_action(),
                    quantity: self.quantity,
                    order_type: OrderType::Market,
                }))
            }
            _ => Err(DomainError::InvalidStateTransition {
                from: self.state.clone(),
                event: "stop_loss_trigger",
            }),
        }
    }

    pub fn apply_exit_filled(
        &mut self,
        fill: OrderFill,
    ) -> Result<(), DomainError> {
        match self.state {
            PositionState::Exiting { exit_reason, .. } => {
                // Calculate PnL
                let entry_value = self.entry_price.unwrap().as_decimal()
                    * self.quantity.as_decimal();
                let exit_value = fill.price.as_decimal()
                    * self.quantity.as_decimal();

                let pnl = match self.side {
                    Side::Long => exit_value - entry_value,
                    Side::Short => entry_value - exit_value,
                };

                let pnl_with_fees = pnl - self.fees_paid;

                // Transition to Closed
                self.state = PositionState::Closed {
                    exit_price: fill.price,
                    realized_pnl: pnl_with_fees,
                    exit_reason,
                };
                self.closed_at = Some(Utc::now());

                Ok(())
            }
            _ => Err(DomainError::InvalidStateTransition {
                from: self.state.clone(),
                event: "exit_filled",
            }),
        }
    }
}
```

---

## Palma da Mão (Technical Stop Distance)

### Calculation

**Input**: Detector signal with entry price and technical stop loss

**Output**: Palma structure with distance and percentage

```rust
pub fn calculate_palma(
    entry_price: Price,
    stop_loss: Price,
) -> Result<PalmaDaMao, DomainError> {
    let distance = (entry_price.as_decimal() - stop_loss.as_decimal()).abs();
    let distance_pct = distance / entry_price.as_decimal() * Decimal::from(100);

    let palma = PalmaDaMao {
        distance,
        distance_pct,
        entry_price,
        stop_loss,
    };

    palma.validate()?;
    Ok(palma)
}
```

### Validation Rules

```rust
impl PalmaDaMao {
    pub fn validate(&self) -> Result<(), DomainError> {
        // 1. Distance must be positive
        if self.distance <= Decimal::ZERO {
            return Err(DomainError::InvalidPalma(
                "Stop loss must be different from entry"
            ));
        }

        // 2. Stop cannot be too wide (>10%)
        if self.distance_pct > Decimal::from(10) {
            return Err(DomainError::InvalidPalma(
                format!("Stop too wide: {:.2}% (max 10%)", self.distance_pct)
            ));
        }

        // 3. Stop cannot be too tight (<0.1%)
        if self.distance_pct < Decimal::new(1, 1) {  // 0.1%
            return Err(DomainError::InvalidPalma(
                format!("Stop too tight: {:.2}% (min 0.1%)", self.distance_pct)
            ));
        }

        Ok(())
    }
}
```

### Example

```
Entry Price: $95,000
Technical SL: $93,500
Palma Distance: $1,500
Palma %: 1.58%

✅ Valid (0.1% < 1.58% < 10%)
```

---

## Position Sizing

### Formula

**Golden Rule**: Position size is DERIVED from technical stop, NOT chosen arbitrarily.

```
Position Size = (Capital × Risk %) / Palma Distance
```

### Implementation

```rust
impl Position {
    pub fn calculate_position_size(
        &self,
        entry_price: Price,
        stop_loss: Price,
    ) -> Result<Quantity, DomainError> {
        // 1. Calculate palma
        let palma = PalmaDaMao::from_entry_and_stop(entry_price, stop_loss);
        palma.validate()?;

        // 2. Get risk config
        let risk_config = &self.strategy.risk_config;
        let capital = risk_config.capital;
        let risk_pct = risk_config.risk_per_trade_pct;  // Default: 1%

        // 3. Calculate max risk amount
        let max_risk = capital * risk_pct / Decimal::from(100);

        // 4. Calculate position size
        let position_size_quote = max_risk / palma.distance;
        let position_size_base = position_size_quote / entry_price.as_decimal();

        // 5. Apply leverage
        let leveraged_size = position_size_base * Decimal::from(self.leverage.as_u8());

        // 6. Round to exchange precision
        let quantity = self.round_to_precision(leveraged_size)?;

        // 7. Validate min/max notional
        self.validate_notional(quantity, entry_price)?;

        Ok(Quantity::new(quantity)?)
    }

    fn validate_notional(
        &self,
        quantity: Decimal,
        price: Price,
    ) -> Result<(), DomainError> {
        let notional = quantity * price.as_decimal();

        // Binance min notional: $10
        if notional < Decimal::from(10) {
            return Err(DomainError::PositionTooSmall(
                format!("Notional ${} < $10 min", notional)
            ));
        }

        // Max notional: Check against available capital
        let max_notional = self.strategy.risk_config.capital
            * Decimal::from(self.leverage.as_u8());

        if notional > max_notional {
            return Err(DomainError::PositionTooLarge(
                format!("Notional ${} > ${} available", notional, max_notional)
            ));
        }

        Ok(())
    }
}
```

### Example

```
Capital: $10,000
Risk per trade: 1%
Entry: $95,000
Technical SL: $93,500
Palma: $1,500
Leverage: 3x

Max Risk: $10,000 × 0.01 = $100
Position Size (quote): $100 / $1,500 = 0.0667 BTC
Leveraged Size: 0.0667 × 3 = 0.2001 BTC
Notional: 0.2001 × $95,000 = $19,009.50

✅ Valid (> $10 min, < $30,000 max)
```

---

## Risk Management

### Risk Configuration

```rust
pub struct RiskConfig {
    pub capital: Decimal,                  // Total capital allocated
    pub risk_per_trade_pct: Decimal,       // Default: 1%
    pub max_open_positions: usize,         // Default: 3
    pub max_daily_loss_pct: Decimal,       // Default: 3%
    pub max_leverage: u8,                  // Default: 3x
    pub insurance_stop_enabled: bool,      // Default: true
    pub insurance_stop_buffer_pct: Decimal,// Default: 1%
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            capital: Decimal::ZERO,
            risk_per_trade_pct: Decimal::ONE,  // 1%
            max_open_positions: 3,
            max_daily_loss_pct: Decimal::from(3),  // 3%
            max_leverage: 3,
            insurance_stop_enabled: true,
            insurance_stop_buffer_pct: Decimal::ONE,  // 1%
        }
    }
}
```

### Risk Checks

```rust
pub struct RiskManager {
    config: RiskConfig,
}

impl RiskManager {
    pub fn can_open_position(
        &self,
        position: &Position,
        open_positions: &[Position],
    ) -> Result<(), RiskError> {
        // 1. Check max open positions
        if open_positions.len() >= self.config.max_open_positions {
            return Err(RiskError::MaxPositionsReached);
        }

        // 2. Check daily loss limit
        let today_pnl = self.calculate_daily_pnl(open_positions);
        let max_daily_loss = self.config.capital
            * self.config.max_daily_loss_pct
            / Decimal::from(100);

        if today_pnl < -max_daily_loss {
            return Err(RiskError::DailyLossLimitReached {
                current: today_pnl,
                limit: max_daily_loss,
            });
        }

        // 3. Check leverage limit
        if position.leverage.as_u8() > self.config.max_leverage {
            return Err(RiskError::LeverageTooHigh {
                requested: position.leverage.as_u8(),
                max: self.config.max_leverage,
            });
        }

        Ok(())
    }

    fn calculate_daily_pnl(&self, positions: &[Position]) -> Decimal {
        let today = Utc::now().date_naive();

        positions
            .iter()
            .filter(|p| {
                p.closed_at
                    .map(|closed| closed.date_naive() == today)
                    .unwrap_or(false)
            })
            .map(|p| match &p.state {
                PositionState::Closed { realized_pnl, .. } => *realized_pnl,
                _ => Decimal::ZERO,
            })
            .sum()
    }
}
```

---

## Invariants

### Position Invariants

1. **Entry price must be positive**: `entry_price > 0`
2. **Stop loss must be valid**: `0 < |entry - stop_loss| < entry × 10%`
3. **Quantity must be positive**: `quantity > 0`
4. **Leverage must be 1-10x**: `1 <= leverage <= 10`
5. **State transitions must be valid**: See state machine
6. **Closed position must have PnL**: `state == Closed → realized_pnl != null`

### System Invariants

1. **Single active trader per (account, symbol)**: Enforced by lease
2. **All orders must have intent journal entry**: WAL before execution
3. **Total risk across positions <= capital**: Sum of position risks
4. **No manual closes**: All exits via system (SL/SG/panic)

---

## Events

### Event Types

```rust
pub enum Event {
    // Position lifecycle
    PositionArmed {
        position_id: PositionId,
        symbol: Symbol,
        strategy: String,
    },

    EntrySignalReceived {
        position_id: PositionId,
        entry_price: Price,
        stop_loss: Price,
        stop_gain: Price,
    },

    EntryOrderPlaced {
        position_id: PositionId,
        order_id: OrderId,
        quantity: Quantity,
    },

    EntryOrderFilled {
        position_id: PositionId,
        order_id: OrderId,
        fill_price: Price,
        fill_quantity: Quantity,
    },

    PositionActivated {
        position_id: PositionId,
        entry_price: Price,
    },

    StopLossTriggered {
        position_id: PositionId,
        trigger_price: Price,
        stop_loss: Price,
    },

    StopGainTriggered {
        position_id: PositionId,
        trigger_price: Price,
        stop_gain: Price,
    },

    ExitOrderPlaced {
        position_id: PositionId,
        order_id: OrderId,
        reason: ExitReason,
    },

    ExitOrderFilled {
        position_id: PositionId,
        order_id: OrderId,
        fill_price: Price,
    },

    PositionClosed {
        position_id: PositionId,
        realized_pnl: Decimal,
        exit_reason: ExitReason,
    },

    // Error events
    PositionError {
        position_id: PositionId,
        error: String,
    },

    // Reconciliation
    ReconciliationStarted {
        position_id: PositionId,
    },

    DiscrepancyDetected {
        position_id: PositionId,
        discrepancy: Discrepancy,
    },

    DegradedModeEntered {
        position_id: PositionId,
        reason: String,
    },
}
```

### Event Sourcing

All events are stored in append-only log:

```sql
CREATE TABLE events (
    id BIGSERIAL PRIMARY KEY,
    position_id UUID NOT NULL,
    event_type TEXT NOT NULL,
    event_data JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_events_position_id ON events(position_id);
CREATE INDEX idx_events_created_at ON events(created_at);
```

---

## Validation Checklist

Before marking domain model complete:

- [ ] All entities have clear identity and lifecycle
- [ ] All value objects are immutable and validated
- [ ] State machine transitions are exhaustive
- [ ] Palma da Mão calculation is correct
- [ ] Position sizing formula matches golden rule
- [ ] Risk management checks all constraints
- [ ] Invariants are enforced at compile time where possible
- [ ] Events capture all state changes
- [ ] Examples validate realistic scenarios

---

**Next Steps**: Implement domain types in `robson-domain` crate

See [EXECUTION-PLAN.md](./EXECUTION-PLAN.md) for detailed roadmap.
