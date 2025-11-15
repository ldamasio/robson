# Robson Bot - Domain Behavioral Specification

**Purpose**: Domain-layer behavioral specification describing entity workflows, state machines, business rules, calculations, and invariants.

**Last Updated**: 2025-11-14

**Version**: 1.0 (Current Implementation)

---

## Table of Contents

1. [Overview](#1-overview)
2. [Trading Entity Lifecycles](#2-trading-entity-lifecycles)
3. [Business Rules and Validations](#3-business-rules-and-validations)
4. [Calculations and Formulas](#4-calculations-and-formulas)
5. [Risk Management](#5-risk-management)
6. [Technical Indicators](#6-technical-indicators)
7. [Pattern Recognition System](#7-pattern-recognition-system)
8. [Trading Configuration Rules](#8-trading-configuration-rules)
9. [Technical Analysis Workflow](#9-technical-analysis-workflow)
10. [Domain Principles](#10-domain-principles)
11. [Multi-Tenant Data Isolation](#11-multi-tenant-data-isolation)
12. [Future Planned Features](#12-future-planned-features)

---

## 1. Overview

### 1.1 Domain Model

The Robson Bot domain model consists of the following core entities:

**Trading Entities**:
- **Symbol**: Trading pair representation (e.g., BTCUSDT)
- **Strategy**: Trading strategy configuration and performance tracking
- **Order**: Individual trade instruction (BUY/SELL)
- **Operation**: Grouping of related entry/exit orders
- **Position**: Open market exposure with unrealized P&L
- **Trade**: Completed trading round-trip with realized P&L

**Supporting Entities**:
- **Risk Rules**: Capital allocation limits (1%, 4%)
- **Indicators**: Technical analysis indicators (MA, RSI, MACD, Bollinger, Stochastic)
- **Client**: Tenant for multi-tenant isolation

**References**: REQ-CUR-DOMAIN-001 to REQ-CUR-DOMAIN-028

### 1.2 Entity Relationships

```
Client (Tenant)
  ├── Symbol (trading pairs)
  ├── Strategy (trading strategies)
  │     └── Orders (linked to strategy)
  ├── Order (individual trades)
  │     ├── Belongs to Symbol
  │     ├── Optional: Belongs to Strategy
  │     └── Optional: Belongs to Operation
  ├── Operation (grouped orders)
  │     ├── entry_orders (many-to-many)
  │     └── exit_orders (many-to-many)
  ├── Position (open exposure)
  │     └── Belongs to Symbol
  ├── Trade (completed round-trip)
  │     └── Belongs to Symbol
  ├── Risk Rules (capital limits)
  └── Indicators (technical analysis)
        └── Belong to Symbol
```

---

## 2. Trading Entity Lifecycles

### 2.1 Order Lifecycle

**References**: REQ-CUR-DOMAIN-005, REQ-CUR-DOMAIN-006, REQ-CUR-DOMAIN-007, REQ-CUR-DOMAIN-008

#### 2.1.1 Order States

Orders follow a defined state machine with 5 possible states:

| State | Description | Terminal? |
|-------|-------------|-----------|
| PENDING | Order created, awaiting fill | No |
| PARTIALLY_FILLED | Some quantity filled, remainder pending | No |
| FILLED | Entire quantity filled | Yes |
| CANCELLED | Order cancelled before complete fill | Yes |
| REJECTED | Order rejected by exchange | Yes |

#### 2.1.2 State Transition Diagram

```
                    ┌──────────┐
                    │  PENDING │
                    └────┬─────┘
                         │
        ┌────────────────┼────────────────┐
        │                │                │
        ▼                ▼                ▼
  ┌──────────┐   ┌───────────────┐  ┌──────────┐
  │CANCELLED │   │PARTIALLY_FILLED│  │ FILLED  │
  └──────────┘   └───────┬────────┘  └──────────┘
                         │
                ┌────────┴────────┐
                │                 │
                ▼                 ▼
          ┌──────────┐      ┌──────────┐
          │CANCELLED │      │ FILLED  │
          └──────────┘      └──────────┘

          (From PENDING)
                 │
                 ▼
          ┌──────────┐
          │ REJECTED │
          └──────────┘
```

#### 2.1.3 Valid Transitions

| From State | Event | To State | Condition |
|------------|-------|----------|-----------|
| PENDING | First partial fill | PARTIALLY_FILLED | 0 < filled_qty < quantity |
| PENDING | Full fill | FILLED | filled_qty >= quantity |
| PENDING | User cancels | CANCELLED | No fills yet |
| PENDING | Exchange rejects | REJECTED | Exchange validation failed |
| PARTIALLY_FILLED | Remaining fills | FILLED | filled_qty >= quantity |
| PARTIALLY_FILLED | User cancels | CANCELLED | User action |
| FILLED | - | - | Terminal state |
| CANCELLED | - | - | Terminal state |
| REJECTED | - | - | Terminal state |

#### 2.1.4 Order Creation Flow

**Sequence Diagram**:
```
User          API           Domain          Database       Exchange
 |             |              |               |              |
 |--Create---->|              |               |              |
 |  Order      |              |               |              |
 |             |--Validate--->|               |              |
 |             |  fields      |               |              |
 |             |<--OK---------|               |              |
 |             |              |               |              |
 |             |--Save--------|-------------->|              |
 |             |  Order       |               |              |
 |             |<--Saved------|<--------------|              |
 |<--201-------|              |               |              |
 |   Created   |              |               |              |
 |             |              |               |              |
 |             |--Submit (async)--------------|------------->|
 |             |              |               |              |
 |             |              |<--Exchange Order ID---------|
 |             |              |               |              |
 |             |--Update------|-------------->|              |
 |             |  exchange_id |               |              |
```

**Preconditions**:
- Symbol exists and is active
- Quantity within symbol min/max constraints
- If LIMIT order: price provided
- If BUY order with stop-loss: stop_loss_price < order price
- If SELL order with stop-loss: stop_loss_price > order price
- User has associated client (tenant)

**Postconditions**:
- Order saved with status PENDING
- Order associated with client
- Order has created_at timestamp
- Order submission triggered asynchronously

#### 2.1.5 Order Fill Flow

**Method**: `Order.mark_as_filled(avg_price: Decimal, filled_qty: Decimal = None)`

**Behavior**:
1. If `filled_qty` not provided, assume full quantity filled
2. Update `filled_quantity` (cumulative)
3. Update `avg_fill_price` (weighted average if multiple fills)
4. Determine new status:
   - If `filled_quantity >= quantity`: FILLED
   - Else: PARTIALLY_FILLED
5. If FILLED, set `filled_at` timestamp
6. Save order

**Example - Single Fill**:
```python
order = Order(symbol="BTCUSDT", side="BUY", quantity=Decimal("1.0"), price=Decimal("50000"))
order.save()  # Status: PENDING

order.mark_as_filled(avg_price=Decimal("50100"))
# Status: FILLED, filled_quantity: 1.0, avg_fill_price: 50100
```

**Example - Partial Fills**:
```python
order = Order(symbol="BTCUSDT", side="BUY", quantity=Decimal("1.0"), price=Decimal("50000"))
order.save()  # Status: PENDING

order.mark_as_filled(avg_price=Decimal("50100"), filled_qty=Decimal("0.3"))
# Status: PARTIALLY_FILLED, filled_quantity: 0.3, avg_fill_price: 50100

order.mark_as_filled(avg_price=Decimal("50200"), filled_qty=Decimal("0.7"))
# Status: FILLED, filled_quantity: 1.0, avg_fill_price: 50170 (weighted avg)
```

#### 2.1.6 Order P&L Calculation

**Method**: `Order.calculate_pnl(current_price: Decimal) -> Decimal`

**Formula**:
```python
# BUY side
pnl = (current_price - fill_price) * quantity

# SELL side
pnl = (fill_price - current_price) * quantity

# Where:
fill_price = avg_fill_price if avg_fill_price is not None else order.price
quantity = filled_quantity if filled_quantity > 0 else order.quantity
```

**Examples**:
```python
# BUY order - profitable
order = Order(side="BUY", quantity=Decimal("1.0"), avg_fill_price=Decimal("50000"))
pnl = order.calculate_pnl(current_price=Decimal("51000"))
# pnl = (51000 - 50000) * 1.0 = 1000.00

# BUY order - loss
pnl = order.calculate_pnl(current_price=Decimal("49000"))
# pnl = (49000 - 50000) * 1.0 = -1000.00

# SELL order - profitable
order = Order(side="SELL", quantity=Decimal("1.0"), avg_fill_price=Decimal("50000"))
pnl = order.calculate_pnl(current_price=Decimal("49000"))
# pnl = (50000 - 49000) * 1.0 = 1000.00

# SELL order - loss
pnl = order.calculate_pnl(current_price=Decimal("51000"))
# pnl = (50000 - 51000) * 1.0 = -1000.00
```

---

### 2.2 Position Lifecycle

**References**: REQ-CUR-DOMAIN-011, REQ-CUR-DOMAIN-012, REQ-CUR-DOMAIN-013

#### 2.2.1 Position Creation

**Trigger**: Order fills (mechanism not yet specified - see [Known Gap](#81-order-position-linkage))

**Initial State**:
- `symbol`: From order
- `side`: "LONG" (from BUY) or "SHORT" (from SELL)
- `quantity`: From order filled_quantity
- `average_price`: From order avg_fill_price
- `unrealized_pnl`: 0
- `status`: "OPEN"

#### 2.2.2 Position Update (Adding Orders)

**Method**: `Position.add_order(order: Order)`

**Behavior**:
1. Check order has avg_fill_price (skip if None)
2. Calculate new average price:
   ```python
   new_avg = (
       (current_avg * current_qty) + (order_fill_price * order_qty)
   ) / (current_qty + order_qty)
   ```
3. Update quantity: `quantity += order.filled_quantity`
4. Update average_price: `average_price = new_avg`
5. Save position

**Example**:
```python
position = Position(symbol="BTCUSDT", side="LONG", quantity=Decimal("1.0"), average_price=Decimal("50000"))

# Add second order at higher price
order2 = Order(avg_fill_price=Decimal("51000"), filled_quantity=Decimal("0.5"))
position.add_order(order2)

# Result:
# quantity: 1.5
# average_price: (50000 * 1.0 + 51000 * 0.5) / 1.5 = 50333.33
```

#### 2.2.3 Position Unrealized P&L Update

**Method**: `Position.update_unrealized_pnl(current_price: Decimal)`

**Formula**:
```python
# Long position
unrealized_pnl = (current_price - average_price) * quantity

# Short position
unrealized_pnl = (average_price - current_price) * quantity
```

**Behavior**:
1. Calculate unrealized P&L based on side
2. Update `unrealized_pnl` field
3. Save position

**Example**:
```python
position = Position(side="LONG", quantity=Decimal("1.0"), average_price=Decimal("50000"))

position.update_unrealized_pnl(current_price=Decimal("52000"))
# unrealized_pnl: (52000 - 50000) * 1.0 = 2000.00

position.update_unrealized_pnl(current_price=Decimal("48000"))
# unrealized_pnl: (48000 - 50000) * 1.0 = -2000.00
```

#### 2.2.4 Position Closure

**Method**: `Position.close_position(exit_price: Decimal) -> Decimal`

**Behavior**:
1. Calculate realized P&L:
   - Long: `(exit_price - average_price) * quantity`
   - Short: `(average_price - exit_price) * quantity`
2. Set `status = "CLOSED"`
3. Set `closed_at = timezone.now()`
4. Save position
5. Return realized P&L

**Postconditions**:
- Position status is CLOSED
- Position has closed_at timestamp
- Realized P&L returned (for Trade creation)

**Example**:
```python
position = Position(
    side="LONG",
    quantity=Decimal("1.0"),
    average_price=Decimal("50000"),
    status="OPEN"
)

realized_pnl = position.close_position(exit_price=Decimal("52000"))
# realized_pnl: 2000.00
# position.status: "CLOSED"
# position.closed_at: 2025-11-14T10:30:00Z
```

#### 2.2.5 Position State Diagram

```
                ┌──────────┐
       create   │   OPEN   │
      ─────────>│          │
                └────┬─────┘
                     │
                     │ add_order()
                     │ update_unrealized_pnl()
                     │
                     │ close_position()
                     ▼
                ┌──────────┐
                │  CLOSED  │ (terminal)
                └──────────┘
```

---

### 2.3 Trade Lifecycle

**References**: REQ-CUR-DOMAIN-014, REQ-CUR-DOMAIN-015, REQ-CUR-DOMAIN-016, REQ-CUR-DOMAIN-017

#### 2.3.1 Trade Creation

**Trigger**: Position closed (mechanism not fully specified - see [Known Gap](#87-trade-p&l-vs-position-p&l))

**Required Fields**:
- `symbol`: Trading pair
- `side`: "BUY" or "SELL"
- `quantity`: Position quantity
- `entry_price`: Position average_price
- `entry_time`: Position created_at
- `exit_price`: Price at position closure
- `exit_time`: Position closed_at
- `entry_fee`: Commission on entry (default 0)
- `exit_fee`: Commission on exit (default 0)

#### 2.3.2 Automatic P&L Calculation

**Trigger**: `Trade.save()` with exit_price set

**Formula**:
```python
# BUY side
gross_pnl = (exit_price - entry_price) * quantity

# SELL side
gross_pnl = (entry_price - exit_price) * quantity

# Net P&L
total_fees = entry_fee + exit_fee
net_pnl = gross_pnl - total_fees
```

**Behavior**:
1. On save, check if `exit_price` is not None
2. Calculate gross P&L based on side
3. Calculate total fees
4. Set `pnl = gross_pnl - total_fees`
5. Continue save

**Example**:
```python
trade = Trade(
    symbol="BTCUSDT",
    side="BUY",
    quantity=Decimal("1.0"),
    entry_price=Decimal("50000"),
    exit_price=Decimal("52000"),
    entry_fee=Decimal("25.00"),  # 0.05% * 50000
    exit_fee=Decimal("26.00")    # 0.05% * 52000
)
trade.save()

# Calculated values:
# gross_pnl: (52000 - 50000) * 1.0 = 2000.00
# total_fees: 25.00 + 26.00 = 51.00
# trade.pnl: 2000.00 - 51.00 = 1949.00
```

#### 2.3.3 Trade Duration

**Properties**:
- `duration`: `timedelta` between entry_time and exit_time
- `duration_hours`: `float` hours between entry and exit

**Example**:
```python
trade = Trade(
    entry_time=datetime(2025, 11, 14, 10, 0, 0, tzinfo=timezone.utc),
    exit_time=datetime(2025, 11, 14, 14, 30, 0, tzinfo=timezone.utc)
)

trade.duration
# timedelta(hours=4, minutes=30)

trade.duration_hours
# 4.5
```

#### 2.3.4 Trade Winner/Loser Identification

**Property**: `is_winner` (boolean)

**Logic**:
```python
if exit_price is None:
    return False  # Not closed yet

if side == "BUY":
    return exit_price > entry_price
else:  # SELL
    return exit_price < entry_price
```

**Usage**: Strategy performance tracking (win rate calculation)

---

### 2.4 Operation Lifecycle

**References**: REQ-CUR-DOMAIN-009, REQ-CUR-DOMAIN-010

#### 2.4.1 Operation Purpose

Operations group related entry and exit orders for unified tracking. Use cases:
- Scaling into position (multiple entry orders)
- Scaling out of position (multiple exit orders)
- Complex strategies with staged entries/exits

#### 2.4.2 Operation Structure

**Fields**:
- `entry_orders`: ManyToManyField to Order (BUY orders)
- `exit_orders`: ManyToManyField to Order (SELL orders)
- `stop_gain_percent`: Optional take-profit percentage
- `stop_loss_percent`: Optional stop-loss percentage
- `status`: PLANNED, ACTIVE, CLOSED, CANCELLED

**Calculated Properties**:
- `total_entry_quantity`: Sum of entry order quantities
- `total_exit_quantity`: Sum of exit order quantities
- `average_entry_price`: Weighted average of entry prices
- `average_exit_price`: Weighted average of exit prices

#### 2.4.3 Operation Unrealized P&L

**Method**: `Operation.calculate_unrealized_pnl(current_price: Decimal) -> Decimal`

**Formula**:
```python
# BUY side (long)
unrealized_pnl = (current_price - avg_entry_price) * total_entry_quantity

# SELL side (short)
unrealized_pnl = (avg_entry_price - current_price) * total_entry_quantity
```

**Note**: Only considers entry orders; exit orders reduce position.

---

### 2.5 Strategy Lifecycle

**References**: REQ-CUR-DOMAIN-003, REQ-CUR-DOMAIN-004

#### 2.5.1 Strategy Configuration

**Configuration Storage**: JSON fields for flexibility

**Fields**:
- `config`: JSONField - Strategy parameters (indicators, conditions, etc.)
- `risk_config`: JSONField - Risk management parameters

**Methods**:
- `get_config_value(key: str)`: Retrieve config value by dotted path
- `set_config_value(key: str, value: Any)`: Update config value

**Example**:
```python
strategy = Strategy(
    name="BTC Trend Follower",
    config={
        "indicators": {
            "ma": {"period": 50, "type": "SMA"},
            "rsi": {"period": 14}
        },
        "entry_condition": "price > ma_50 and rsi < 30",
        "exit_condition": "price < ma_50 or rsi > 70"
    },
    risk_config={
        "max_position_size": 1000.00,
        "stop_loss_percent": 2.00
    }
)

strategy.get_config_value("indicators.ma.period")  # 50
strategy.set_config_value("indicators.ma.period", 100)
```

#### 2.5.2 Strategy Performance Tracking

**Method**: `Strategy.update_performance(pnl: Decimal, is_winner: bool)`

**Behavior**:
1. Increment `total_trades`
2. If `is_winner`, increment `winning_trades`
3. Add `pnl` to `total_pnl`
4. Save strategy

**Calculated Properties**:
```python
win_rate = (winning_trades / total_trades) * 100 if total_trades > 0 else 0.0

average_pnl_per_trade = total_pnl / total_trades if total_trades > 0 else 0.0
```

**Example**:
```python
strategy = Strategy(total_trades=0, winning_trades=0, total_pnl=Decimal("0"))

# Trade 1: Winner (+100)
strategy.update_performance(pnl=Decimal("100"), is_winner=True)
# total_trades: 1, winning_trades: 1, total_pnl: 100, win_rate: 100%

# Trade 2: Loser (-50)
strategy.update_performance(pnl=Decimal("-50"), is_winner=False)
# total_trades: 2, winning_trades: 1, total_pnl: 50, win_rate: 50%

# Trade 3: Winner (+150)
strategy.update_performance(pnl=Decimal("150"), is_winner=True)
# total_trades: 3, winning_trades: 2, total_pnl: 200, win_rate: 66.67%
# average_pnl_per_trade: 66.67
```

---

## 3. Business Rules and Validations

### 3.1 Symbol Quantity Constraints

**References**: REQ-CUR-DOMAIN-002

**Validation Method**: `Symbol.is_quantity_valid(quantity: Decimal) -> bool`

**Rules**:
1. `quantity >= min_qty`
2. If `max_qty` set: `quantity <= max_qty`

**Example**:
```python
symbol = Symbol(name="BTCUSDT", min_qty=Decimal("0.001"), max_qty=Decimal("100"))

symbol.is_quantity_valid(Decimal("0.01"))   # True
symbol.is_quantity_valid(Decimal("0.0001")) # False (below min)
symbol.is_quantity_valid(Decimal("150"))    # False (above max)
```

**Model Validation** (`Symbol.clean()`):
```python
def clean(self):
    if self.min_qty <= 0:
        raise ValidationError("min_qty must be greater than 0")

    if self.max_qty is not None:
        if self.max_qty <= 0:
            raise ValidationError("max_qty must be greater than 0")
        if self.max_qty <= self.min_qty:
            raise ValidationError("max_qty must be greater than min_qty")
```

---

### 3.2 Order Stop-Loss Validation

**References**: REQ-CUR-DOMAIN-008

**Validation Method**: `Order.clean()`

**Rules**:
- **BUY order**: If `stop_loss_price` set, must be `< price`
- **SELL order**: If `stop_loss_price` set, must be `> price`

**Rationale**: Stop-loss must trigger on adverse price movement.

**Example**:
```python
# Valid BUY with stop-loss
order = Order(side="BUY", price=Decimal("50000"), stop_loss_price=Decimal("49000"))
order.clean()  # OK

# Invalid BUY with stop-loss
order = Order(side="BUY", price=Decimal("50000"), stop_loss_price=Decimal("51000"))
order.clean()  # Raises ValidationError: "Stop-loss must be below entry price for BUY orders"

# Valid SELL with stop-loss
order = Order(side="SELL", price=Decimal("50000"), stop_loss_price=Decimal("51000"))
order.clean()  # OK

# Invalid SELL with stop-loss
order = Order(side="SELL", price=Decimal("50000"), stop_loss_price=Decimal("49000"))
order.clean()  # Raises ValidationError: "Stop-loss must be above entry price for SELL orders"
```

---

### 3.3 Risk Rule Validation

**References**: REQ-CUR-DOMAIN-018

**Validation Method**: `BaseRiskRule.clean()`

**Rules**:
- `risk_percentage >= 0`
- If `risk_percentage < 0`, auto-correct to `0`

**Example**:
```python
rule = BaseRiskRule(risk_percentage=Decimal("-1.00"))
rule.clean()
# risk_percentage: 0.00 (auto-corrected)
```

**Subclass Enforcement**:
- `OnePercentOfCapital.clean()`: Forces `risk_percentage = 1.00`
- `JustBet4percent.clean()`: Forces `risk_percentage = 4.00`

---

## 4. Calculations and Formulas

### 4.1 P&L Calculation Summary

All P&L calculations follow consistent formulas based on side (BUY/SELL).

#### 4.1.1 Order P&L

**Reference**: REQ-CUR-DOMAIN-007

```python
# BUY
pnl = (current_price - fill_price) * quantity

# SELL
pnl = (fill_price - current_price) * quantity
```

#### 4.1.2 Position Unrealized P&L

**Reference**: REQ-CUR-DOMAIN-012

```python
# Long
unrealized_pnl = (current_price - average_price) * quantity

# Short
unrealized_pnl = (average_price - current_price) * quantity
```

#### 4.1.3 Position Realized P&L

**Reference**: REQ-CUR-DOMAIN-013

```python
# Long
realized_pnl = (exit_price - average_price) * quantity

# Short
realized_pnl = (average_price - exit_price) * quantity
```

#### 4.1.4 Trade Net P&L

**Reference**: REQ-CUR-DOMAIN-015

```python
# BUY
gross_pnl = (exit_price - entry_price) * quantity

# SELL
gross_pnl = (entry_price - exit_price) * quantity

# Net P&L (both sides)
net_pnl = gross_pnl - (entry_fee + exit_fee)
```

---

### 4.2 Average Price Calculation

**Reference**: REQ-CUR-DOMAIN-011

#### 4.2.1 Position Average Price

When adding orders to existing position:

```python
new_average_price = (
    (current_avg_price * current_quantity) +
    (order_fill_price * order_quantity)
) / (current_quantity + order_quantity)
```

**Example**:
```
Initial position: 1.0 BTC @ 50,000
Add order: 0.5 BTC @ 51,000

new_avg = (50000 * 1.0 + 51000 * 0.5) / (1.0 + 0.5)
        = (50000 + 25500) / 1.5
        = 50333.33
```

#### 4.2.2 Operation Average Price

Weighted average across multiple orders:

```python
total_value = sum(order.avg_fill_price * order.filled_quantity for order in entry_orders)
total_quantity = sum(order.filled_quantity for order in entry_orders)

average_entry_price = total_value / total_quantity if total_quantity > 0 else 0
```

---

### 4.3 Strategy Performance Metrics

**Reference**: REQ-CUR-DOMAIN-004

#### 4.3.1 Win Rate

```python
win_rate = (winning_trades / total_trades) * 100  if total_trades > 0 else 0.0
```

#### 4.3.2 Average P&L per Trade

```python
average_pnl_per_trade = total_pnl / total_trades  if total_trades > 0 else 0.0
```

**Example**:
```
Strategy: 100 total trades, 60 winners, total P&L: 10,000 USDT

win_rate = (60 / 100) * 100 = 60%
average_pnl_per_trade = 10000 / 100 = 100 USDT
```

---

## 5. Risk Management

**References**: REQ-CUR-DOMAIN-018, REQ-CUR-DOMAIN-019, REQ-CUR-DOMAIN-020

### 5.1 Risk Rule Model

**Base Class**: `BaseRiskRule`

**Purpose**: Define percentage-based capital allocation limits.

**Fields**:
- `risk_percentage`: Decimal (5, 2) - Percentage of capital (e.g., 1.00 for 1%)
- `max_capital_amount`: Decimal (20, 2), optional - Hard cap in base currency

**Enforcement**: **NOT YET IMPLEMENTED** (see [Known Gap](#91-risk-rule-enforcement))

### 5.2 Predefined Risk Rules

#### 5.2.1 One Percent Rule

**Class**: `OnePercentOfCapital`

**Configuration**:
- `risk_percentage`: 1.00 (enforced by clean())
- Default name: "One Percent Of Capital"
- Default description: "Limit each trade exposure to one percent of available capital."

**Usage**: Conservative risk management for retail traders.

**Example**:
```python
rule = OnePercentOfCapital(client=client)
rule.save()
# risk_percentage: 1.00

# Calculation (when implemented):
# capital = 100,000 USDT
# max_position_value = 100,000 * 0.01 = 1,000 USDT
```

#### 5.2.2 Four Percent Rule

**Class**: `JustBet4percent`

**Configuration**:
- `risk_percentage`: 4.00 (enforced by clean())
- Default name: "Just Bet 4 Percent"
- Default description: "Allow up to four percent of capital to be allocated to a single position."

**Usage**: Moderate risk tolerance for experienced traders.

**Example**:
```python
rule = JustBet4percent(client=client)
rule.save()
# risk_percentage: 4.00

# Calculation (when implemented):
# capital = 100,000 USDT
# max_position_value = 100,000 * 0.04 = 4,000 USDT
```

### 5.3 Future Risk Management

**Planned Features** (see [Future Requirements](#83-risk-management-enhancements)):
- Automatic risk rule enforcement at order creation
- Position size validation
- Real-time capital usage tracking
- Risk-adjusted performance metrics (Sharpe ratio, Sortino ratio)

---

## 6. Technical Indicators

**References**: REQ-CUR-DOMAIN-021 to REQ-CUR-DOMAIN-026

### 6.1 Indicator Base Model

**Base Class**: `StatisticalIndicator`

**Common Fields**:
- `symbol`: ForeignKey to Symbol
- `timeframe`: CharField (default "1h") - e.g., "1m", "5m", "15m", "1h", "4h", "1d"
- `created_at`: Auto timestamp
- `updated_at`: Auto timestamp

**Note**: Indicator calculation logic **not yet implemented** (see [Known Gap](#92-indicator-calculation)).

### 6.2 Moving Average

**Model**: `MovingAverage`

**Fields**:
- `period`: IntegerField - Number of candles (e.g., 20, 50, 200)
- `value`: Decimal (20, 8) - Calculated MA value

**Usage**: Trend identification, support/resistance levels.

**Example**:
```python
ma = MovingAverage(
    symbol=btc_symbol,
    timeframe="1h",
    period=50,
    value=Decimal("50123.45678912")
)
```

### 6.3 RSI (Relative Strength Index)

**Model**: `RSIIndicator`

**Fields**:
- `period`: IntegerField - Typically 14
- `value`: Decimal (20, 8) - Range 0-100

**Interpretation**:
- RSI > 70: Overbought
- RSI < 30: Oversold

**Example**:
```python
rsi = RSIIndicator(
    symbol=btc_symbol,
    timeframe="1h",
    period=14,
    value=Decimal("65.50")
)
```

### 6.4 MACD

**Model**: `MACDIndicator`

**Fields**:
- `fast_period`: IntegerField - Typically 12
- `slow_period`: IntegerField - Typically 26
- `signal_period`: IntegerField - Typically 9
- `macd`: Decimal (20, 8) - MACD line
- `signal`: Decimal (20, 8) - Signal line
- `histogram`: Decimal (20, 8) - MACD - Signal

**Signals**:
- MACD crosses above signal: Bullish
- MACD crosses below signal: Bearish

**Example**:
```python
macd = MACDIndicator(
    symbol=btc_symbol,
    timeframe="1h",
    fast_period=12,
    slow_period=26,
    signal_period=9,
    macd=Decimal("123.45"),
    signal=Decimal("110.50"),
    histogram=Decimal("12.95")  # macd - signal
)
```

### 6.5 Bollinger Bands

**Model**: `BollingerBands`

**Fields**:
- `period`: IntegerField - Typically 20
- `standard_deviations`: Decimal (5, 2) - Default 2.00
- `upper_band`: Decimal (20, 8)
- `middle_band`: Decimal (20, 8) - Simple MA
- `lower_band`: Decimal (20, 8)

**Interpretation**:
- Price near upper band: Overbought
- Price near lower band: Oversold
- Narrow bands: Low volatility
- Wide bands: High volatility

**Example**:
```python
bb = BollingerBands(
    symbol=btc_symbol,
    timeframe="1h",
    period=20,
    standard_deviations=Decimal("2.00"),
    upper_band=Decimal("51000.00"),
    middle_band=Decimal("50000.00"),
    lower_band=Decimal("49000.00")
)
```

### 6.6 Stochastic Oscillator

**Model**: `StochasticOscillator`

**Fields**:
- `period`: IntegerField - Default 14
- `k_value`: Decimal (5, 2) - %K line (range 0-100)
- `d_value`: Decimal (5, 2) - %D line (range 0-100)
- `slow_d_value`: Decimal (5, 2), optional - Slow %D

**Interpretation**:
- %K > 80: Overbought
- %K < 20: Oversold

**Example**:
```python
stoch = StochasticOscillator(
    symbol=btc_symbol,
    timeframe="1h",
    period=14,
    k_value=Decimal("75.50"),
    d_value=Decimal("72.30"),
    slow_d_value=Decimal("70.10")
)
```

---

## 7. Pattern Recognition System

**Purpose**: Formal catalog and detection of technical chart patterns with lifecycle tracking.

### 7.1 Pattern Categories

**Enum**: `PatternCategory`

| Category | Description | Examples |
|----------|-------------|----------|
| CHART | Classical chart patterns | Head & Shoulders, Triangles, Wedges |
| CANDLESTICK | Japanese candlestick formations | Doji, Hammer, Engulfing |
| HARMONIC | Harmonic price patterns | Gartley, Butterfly, Bat, Crab |
| ELLIOTT | Elliott Wave structures | Impulse waves, Corrective waves |
| WYCKOFF | Wyckoff methodology patterns | Accumulation, Distribution phases |
| INDICATOR | Indicator-based patterns | Divergences, Crossovers |
| CYCLE | Cyclical/seasonal patterns | Market cycles, Seasonal trends |
| HYBRID | Multi-method combinations | Combined pattern signals |

### 7.2 Pattern Catalog

**Model**: `PatternCatalog`

**Purpose**: Formal definition of all supported technical patterns.

**Fields**:
- `pattern_code`: Unique identifier (e.g., "HS", "GARTLEY_222")
- `name`: Human-readable name
- `category`: PatternCategory enum
- `direction_bias`: Expected market direction (BULLISH, BEARISH, CONTINUATION, NEUTRAL)
- `reliability_score`: Decimal (0-100) - Historical success rate
- `minimum_bars`: Integer - Minimum candles required
- `description`: Detailed pattern description
- `entry_rules`: JSONField - Entry trigger conditions
- `exit_rules`: JSONField - Exit trigger conditions
- `stop_rules`: JSONField - Stop-loss placement rules
- `target_calculation`: JSONField - Price target formula

**Example**:
```python
pattern = PatternCatalog(
    pattern_code="HS",
    name="Head and Shoulders",
    category=PatternCategory.CHART,
    direction_bias=PatternDirectionBias.BEARISH,
    reliability_score=Decimal("72.50"),
    minimum_bars=15,
    description="Reversal pattern indicating trend exhaustion",
    entry_rules={
        "trigger": "neckline_break",
        "confirmation": "close_below_neckline",
        "volume_requirement": "rising"
    },
    exit_rules={
        "target": "neckline_to_head_distance",
        "partial_exit": "50_percent_at_half_target"
    },
    stop_rules={
        "method": StopMethod.STRUCTURE,
        "placement": "above_right_shoulder"
    }
)
```

### 7.3 Pattern Instance Lifecycle

**Base Model**: `PatternInstance` (abstract)

**States**: `PatternStatus` enum

```
FORMING
  │
  ├─> CONFIRMED (pattern fully formed)
  │     │
  │     ├─> TARGET_HIT (price target achieved)
  │     ├─> FAILED (price reversed before target)
  │     └─> INVALIDATED (pattern structure broken)
  │
  ├─> INVALIDATED (formation failed)
  └─> EXPIRED (timeout before confirmation)
```

#### 7.3.1 State Transitions

| From | Event | To | Condition |
|------|-------|-----|-----------|
| FORMING | All pattern points identified | CONFIRMED | Structure meets catalog requirements |
| FORMING | Structure breaks | INVALIDATED | Price violates pattern rules |
| FORMING | Timeout | EXPIRED | Time exceeds max_formation_time |
| CONFIRMED | Target price reached | TARGET_HIT | Price >= target_price (bullish) or <= target_price (bearish) |
| CONFIRMED | Stop hit | FAILED | Price hits stop_loss_price |
| CONFIRMED | Structure invalidated | INVALIDATED | Pattern rules violated post-confirmation |

#### 7.3.2 Common Pattern Instance Fields

**Fields** (all pattern types):
- `catalog`: ForeignKey to PatternCatalog
- `symbol`: ForeignKey to Symbol
- `timeframe`: CharField (e.g., "1h", "4h", "1d")
- `status`: PatternStatus enum
- `detected_at`: DateTimeField - Detection timestamp
- `confirmed_at`: DateTimeField, optional
- `completed_at`: DateTimeField, optional
- `confidence_score`: Decimal (0-100) - Detection confidence
- `target_price`: Decimal, optional
- `stop_loss_price`: Decimal, optional
- `breakout_direction`: BreakoutDirection enum (UP, DOWN, NONE)
- `volume_profile`: VolumeProfile enum (RISING, FALLING, MIXED)

### 7.4 Harmonic Patterns

**Model**: `HarmonicPattern` (extends PatternInstance)

**Additional Fields**:
- `point_x`: Decimal - X point price
- `point_a`: Decimal - A point price
- `point_b`: Decimal - B point price
- `point_c`: Decimal - C point price
- `point_d`: Decimal - D point price (completion point)
- `xab_ratio`: Decimal - Fibonacci ratio XA to AB
- `abc_ratio`: Decimal - Fibonacci ratio AB to BC
- `bcd_ratio`: Decimal - Fibonacci ratio BC to CD
- `xad_ratio`: Decimal - Fibonacci ratio XA to AD

**Common Harmonic Patterns**:
- **Gartley**: XAB=0.618, ABC=0.382-0.886, BCD=1.13-1.618, XAD=0.786
- **Butterfly**: XAB=0.786, ABC=0.382-0.886, BCD=1.618-2.24, XAD=1.27-1.618
- **Bat**: XAB=0.382-0.50, ABC=0.382-0.886, BCD=1.618-2.618, XAD=0.886
- **Crab**: XAB=0.382-0.618, ABC=0.382-0.886, BCD=2.618-3.618, XAD=1.618

**Example**:
```python
gartley = HarmonicPattern(
    catalog=gartley_catalog,
    symbol=btc_symbol,
    timeframe="4h",
    status=PatternStatus.CONFIRMED,
    point_x=Decimal("48000"),
    point_a=Decimal("52000"),
    point_b=Decimal("50000"),
    point_c=Decimal("51200"),
    point_d=Decimal("48800"),
    xab_ratio=Decimal("0.618"),
    abc_ratio=Decimal("0.618"),
    bcd_ratio=Decimal("1.27"),
    xad_ratio=Decimal("0.786"),
    confidence_score=Decimal("85.00"),
    target_price=Decimal("52500"),
    stop_loss_price=Decimal("48500")
)
```

### 7.5 Elliott Wave Patterns

**Model**: `ElliottWavePattern` (extends PatternInstance)

**Additional Fields**:
- `wave_degree`: CharField - Wave degree (Grand Supercycle to Subminuette)
- `wave_structure`: CharField - Structure type (IMPULSE, ZIGZAG, FLAT, TRIANGLE)
- `wave_1_high`: Decimal
- `wave_1_low`: Decimal
- `wave_2_low`: Decimal (for impulse) / wave_2_high (for zigzag)
- `wave_3_high`: Decimal
- `wave_4_low`: Decimal
- `wave_5_high`: Decimal (projected or actual)
- `fibonacci_extensions`: JSONField - Extension levels for targets

**Wave Degrees** (largest to smallest):
1. Grand Supercycle
2. Supercycle
3. Cycle
4. Primary
5. Intermediate
6. Minor
7. Minute
8. Minuette
9. Subminuette

**Validation Rules**:
- Wave 2 never retraces more than 100% of Wave 1
- Wave 3 is never the shortest of waves 1, 3, and 5
- Wave 4 does not overlap Wave 1 price territory (in impulse)

### 7.6 Wyckoff Patterns

**Model**: `WyckoffPattern` (extends PatternInstance)

**Additional Fields**:
- `phase`: CharField - Wyckoff phase (ACCUMULATION, MARKUP, DISTRIBUTION, MARKDOWN)
- `schematic`: CharField - Specific schematic (e.g., "ACCUMULATION_1", "DISTRIBUTION_2")
- `preliminary_support`: Decimal, optional - PS level
- `selling_climax`: Decimal, optional - SC level
- `automatic_rally`: Decimal, optional - AR level
- `secondary_test`: Decimal, optional - ST level
- `spring`: Decimal, optional - Spring level
- `sign_of_strength`: Decimal, optional - SOS level
- `last_point_of_support`: Decimal, optional - LPS level
- `backup_to_edge`: Decimal, optional - BUEC/LPSY level

**Wyckoff Phases**:
1. **Accumulation**: Smart money buying (schematics #1, #2, #3)
2. **Markup**: Uptrend following accumulation
3. **Distribution**: Smart money selling (schematics #1, #2)
4. **Markdown**: Downtrend following distribution

### 7.7 Chart Patterns

**Model**: `ChartPattern` (extends PatternInstance)

**Additional Fields**:
- `pattern_type`: CharField - Specific pattern (HEAD_SHOULDERS, TRIANGLE, WEDGE, etc.)
- `neckline_price`: Decimal, optional
- `left_shoulder_high`: Decimal, optional
- `head_high`: Decimal, optional
- `right_shoulder_high`: Decimal, optional
- `support_line_slope`: Decimal, optional
- `resistance_line_slope`: Decimal, optional
- `apex_date`: DateTimeField, optional - For triangles
- `width`: Decimal, optional - Pattern width in price
- `height`: Decimal, optional - Pattern height in price

**Common Chart Patterns**:
- **Reversal**: Head & Shoulders, Inverse H&S, Double Top/Bottom, Triple Top/Bottom
- **Continuation**: Flags, Pennants, Triangles (Ascending, Descending, Symmetrical)
- **Bilateral**: Rectangles, Expanding formations

**Example - Head & Shoulders**:
```python
hs_pattern = ChartPattern(
    catalog=hs_catalog,
    symbol=btc_symbol,
    timeframe="1d",
    status=PatternStatus.CONFIRMED,
    pattern_type="HEAD_SHOULDERS",
    left_shoulder_high=Decimal("52000"),
    head_high=Decimal("54000"),
    right_shoulder_high=Decimal("51500"),
    neckline_price=Decimal("48000"),
    target_price=Decimal("42000"),  # Head to neckline distance
    stop_loss_price=Decimal("52000"),  # Above right shoulder
    breakout_direction=BreakoutDirection.DOWN,
    confidence_score=Decimal("78.00")
)
```

### 7.8 Candlestick Patterns

**Model**: `CandlestickPattern` (extends PatternInstance)

**Additional Fields**:
- `pattern_type`: CharField - Candlestick formation name
- `candle_count`: IntegerField - Number of candles in pattern
- `body_to_wick_ratio`: Decimal, optional
- `first_candle_open`: Decimal
- `first_candle_close`: Decimal
- `first_candle_high`: Decimal
- `first_candle_low`: Decimal
- `second_candle_open`: Decimal, optional
- `second_candle_close`: Decimal, optional
- `third_candle_open`: Decimal, optional
- `third_candle_close`: Decimal, optional

**Common Patterns**:
- **Single**: Doji, Hammer, Shooting Star, Spinning Top
- **Double**: Engulfing (Bullish/Bearish), Harami, Piercing/Dark Cloud
- **Triple**: Morning/Evening Star, Three White Soldiers, Three Black Crows

**Example - Bullish Engulfing**:
```python
engulfing = CandlestickPattern(
    catalog=engulfing_catalog,
    symbol=btc_symbol,
    timeframe="4h",
    status=PatternStatus.CONFIRMED,
    pattern_type="BULLISH_ENGULFING",
    candle_count=2,
    first_candle_open=Decimal("50000"),
    first_candle_close=Decimal("49500"),  # Bearish
    first_candle_high=Decimal("50200"),
    first_candle_low=Decimal("49400"),
    second_candle_open=Decimal("49400"),
    second_candle_close=Decimal("50500"),  # Bullish, engulfs first
    second_candle_high=Decimal("50600"),
    direction_bias=PatternDirectionBias.BULLISH,
    confidence_score=Decimal("82.00")
)
```

### 7.9 Indicator-Based Patterns

**Model**: `IndicatorPattern` (extends PatternInstance)

**Additional Fields**:
- `indicator_type`: CharField - Indicator name (RSI, MACD, STOCH, etc.)
- `pattern_type`: CharField - Pattern within indicator (DIVERGENCE, CROSSOVER, OVERBOUGHT, etc.)
- `divergence_type`: CharField, optional - REGULAR, HIDDEN, EXAGGERATED
- `indicator_value_1`: Decimal - First reference value
- `indicator_value_2`: Decimal, optional - Second reference value (for divergences)
- `price_at_value_1`: Decimal
- `price_at_value_2`: Decimal, optional

**Common Indicator Patterns**:
- **Divergences**: Regular Bullish/Bearish, Hidden Bullish/Bearish
- **Crossovers**: MA crossover, MACD line/signal cross
- **Extremes**: RSI overbought (>70), RSI oversold (<30)
- **Breakouts**: Bollinger Band squeeze breakout

**Example - RSI Bullish Divergence**:
```python
rsi_div = IndicatorPattern(
    catalog=rsi_div_catalog,
    symbol=btc_symbol,
    timeframe="1h",
    status=PatternStatus.CONFIRMED,
    indicator_type="RSI",
    pattern_type="DIVERGENCE",
    divergence_type="REGULAR_BULLISH",
    indicator_value_1=Decimal("35.00"),  # First low
    indicator_value_2=Decimal("40.00"),  # Higher low
    price_at_value_1=Decimal("48000"),   # Lower price low
    price_at_value_2=Decimal("47500"),   # Lower price low (divergence!)
    direction_bias=PatternDirectionBias.BULLISH,
    confidence_score=Decimal("75.00")
)
```

### 7.10 Pattern-to-Trade Workflow

**Sequence**:
```
1. Pattern Detection
   ├─> PatternInstance created (status: FORMING)
   └─> Confidence score calculated

2. Pattern Confirmation
   ├─> Structure validation passed
   ├─> Status → CONFIRMED
   ├─> Entry trigger activated
   └─> Signal generation (if enabled)

3. Trade Execution
   ├─> Order created (linked to pattern)
   ├─> Entry price set from pattern rules
   ├─> Stop-loss from pattern.stop_loss_price
   └─> Target from pattern.target_price

4. Pattern Resolution
   ├─> TARGET_HIT → Close position (profit)
   ├─> FAILED → Stop-loss hit (loss)
   └─> INVALIDATED → Manual review
```

**Integration Points**:
- `Order.pattern`: ForeignKey to PatternInstance (optional)
- `Strategy.config['patterns']`: List of enabled pattern codes
- `Signal.pattern`: ForeignKey to PatternInstance (future)

---

## 8. Trading Configuration Rules

**Purpose**: Configurable business rules that govern trading behavior.

### 8.1 Base Configuration Model

**Base Class**: `BaseConfigModel`

**Common Fields** (via mixins):
- `name`: CharField - Rule name
- `description`: TextField - Rule description
- `client`: ForeignKey to Client (multi-tenant)
- `created_at`, `updated_at`: Timestamps

### 8.2 Only Trade Reversal Rule

**Model**: `OnlyTradeReversal`

**Purpose**: Enforce trading only when reversal patterns are confirmed.

**Fields**:
- `is_enabled`: BooleanField (default: True)
- `minimum_confirmation`: PositiveIntegerField (default: 1)

**Behavior**:
- When `is_enabled = True`, system blocks trades that are NOT based on reversal patterns
- `minimum_confirmation` specifies how many reversal confirmations required
- Reversal patterns: Head & Shoulders, Double Top/Bottom, Reversal candlesticks

**Rationale**: "Reversals reinforce the trend of the opposing technical event within the chart pattern."

**Example**:
```python
rule = OnlyTradeReversal(
    client=client,
    is_enabled=True,
    minimum_confirmation=2
)

# Before creating order:
if rule.is_enabled:
    reversal_patterns = order.get_supporting_patterns(category="REVERSAL")
    if len(reversal_patterns) < rule.minimum_confirmation:
        raise ValidationError(
            f"Order requires {rule.minimum_confirmation} reversal confirmations, "
            f"found {len(reversal_patterns)}"
        )
```

### 8.3 Max Trades Per Day Rule

**Model**: `MaxTradePerDay`

**Purpose**: Limit trading frequency to prevent overtrading.

**Fields**:
- `max_trades`: PositiveIntegerField (default: 3)

**Behavior**:
- Counts completed trades in rolling 24-hour window
- Blocks new orders if limit reached
- Applies per client (multi-tenant)

**Example**:
```python
rule = MaxTradePerDay(client=client, max_trades=3)

# Before creating order:
recent_trades = Trade.objects.filter(
    client=client,
    created_at__gte=timezone.now() - timedelta(days=1)
).count()

if recent_trades >= rule.max_trades:
    raise ValidationError(
        f"Daily trade limit reached ({rule.max_trades}). "
        f"Try again after {next_allowed_time}"
    )
```

### 8.4 Configuration Rule Lifecycle

**Creation**:
1. Admin/user creates configuration rule
2. `clean()` method sets default name/description if missing
3. Rule saved with client association

**Enforcement**:
- Rules checked at order creation time
- ValidationError raised if rule violated
- User notified of specific rule that blocked trade

**Deactivation**:
- Set `is_enabled = False` (if field exists)
- Or delete rule instance
- Existing orders unaffected (rules apply only at creation)

---

## 9. Technical Analysis Workflow

**Purpose**: Structured workflow for documenting technical analysis reasoning.

### 9.1 Analysis Hierarchy

```
TechnicalAnalysisInterpretation (Catalog)
  │
  └─> TechnicalEvent (Instance)
        │
        └─> Argument (Supporting evidence)
              │
              └─> Reason (Rationale)
```

### 9.2 Technical Analysis Interpretation

**Model**: `TechnicalAnalysisInterpretation`

**Purpose**: Catalog of recognized technical analysis concepts.

**Fields**:
- `name`: CharField - Interpretation name (e.g., "Bullish Breakout", "Trend Reversal")
- `description`: TextField - Detailed explanation
- `experience`: IntegerField (1-5) - Required experience level

**Experience Levels**:
1. **Beginner**: Basic patterns, simple indicators
2. **Intermediate**: Multiple indicator combinations
3. **Advanced**: Complex patterns, divergences
4. **Expert**: Multi-timeframe analysis, harmonic patterns
5. **Master**: Full market structure analysis, Wyckoff, Elliott Wave

**Example**:
```python
interpretation = TechnicalAnalysisInterpretation(
    name="Bullish Harmonic Pattern Completion",
    description="Price completes bullish harmonic pattern at key Fibonacci level with RSI divergence",
    experience=4  # Expert level
)
```

### 9.3 Technical Event

**Model**: `TechnicalEvent`

**Purpose**: Concrete occurrence of a technical interpretation in market data.

**Fields**:
- `interpretation`: ForeignKey to TechnicalAnalysisInterpretation
- `strategy`: ForeignKey to Strategy
- `timeframe`: CharField (e.g., "1h", "4h", "1d")
- `detected_at`: DateTimeField
- `symbol`: ForeignKey to Symbol (via BaseTechnicalModel)

**Relationships**:
- `arguments`: Reverse FK from Argument

**Example**:
```python
event = TechnicalEvent(
    interpretation=bullish_breakout_interpretation,
    strategy=my_strategy,
    symbol=btc_symbol,
    timeframe="4h",
    detected_at=timezone.now()
)
```

### 9.4 Argument

**Model**: `Argument`

**Purpose**: Supporting evidence for a technical event.

**Fields**:
- `technical_event`: ForeignKey to TechnicalEvent
- `name`: CharField - Argument description
- `created_at`: DateTimeField

**Relationships**:
- `reasons`: Reverse FK from Reason

**Example**:
```python
argument1 = Argument(
    technical_event=event,
    name="Price broke above 200-period MA with strong volume"
)

argument2 = Argument(
    technical_event=event,
    name="RSI showing bullish divergence on 1h timeframe"
)
```

### 9.5 Reason

**Model**: `Reason`

**Purpose**: Detailed rationale explaining an argument.

**Fields**:
- `argument`: ForeignKey to Argument
- `name`: CharField - Reasoning text
- `created_at`: DateTimeField

**Example**:
```python
reason = Reason(
    argument=argument1,
    name="Volume spike of 300% confirms institutional buying pressure at breakout level"
)
```

### 9.6 Complete Analysis Workflow

**Sequence Diagram**:
```
Analyst                   System                    Database
   │                         │                          │
   │──1. Detect Pattern────>│                          │
   │                         │──Create Event──────────>│
   │                         │<─Event ID───────────────│
   │                         │                          │
   │──2. Add Arguments────>│                          │
   │   (Evidence)            │──Save Arguments───────>│
   │                         │                          │
   │──3. Add Reasons──────>│                          │
   │   (Rationale)           │──Save Reasons─────────>│
   │                         │                          │
   │──4. Generate Signal──>│                          │
   │                         │──Check Rules──────────>│
   │                         │<─Rules OK───────────────│
   │                         │──Create Order─────────>│
   │<─Order Created─────────│                          │
```

**Full Example**:
```python
# 1. Create interpretation (catalog entry)
interpretation = TechnicalAnalysisInterpretation.objects.create(
    name="Double Bottom Reversal",
    description="Price forms two equal lows with intervening rally",
    experience=2
)

# 2. Detect event
event = TechnicalEvent.objects.create(
    interpretation=interpretation,
    strategy=strategy,
    symbol=symbol,
    timeframe="4h"
)

# 3. Add supporting arguments
arg1 = Argument.objects.create(
    technical_event=event,
    name="Second bottom at 48,000 matches first bottom within 0.5%"
)

arg2 = Argument.objects.create(
    technical_event=event,
    name="Price broke above neckline at 50,000 with volume confirmation"
)

# 4. Add detailed reasoning
Reason.objects.create(
    argument=arg1,
    name="Equal lows indicate strong support level where buyers consistently enter"
)

Reason.objects.create(
    argument=arg2,
    name="Volume on neckline break 2x average, confirming institutional participation"
)

# 5. Use event to justify order
order = Order.objects.create(
    symbol=symbol,
    side="BUY",
    quantity=Decimal("1.0"),
    price=Decimal("50200"),
    technical_event=event,  # Link order to analysis
    stop_loss_price=Decimal("47500"),  # Below second bottom
    target_price=Decimal("52000")  # Neckline to bottom distance projected up
)
```

---

## 10. Domain Principles

**Purpose**: Qualitative knowledge and trading principles encoded in the domain.

### 10.1 Base Principle Model

**Base Class**: `BasePrinciple`

**Common Fields** (via mixins):
- `name`: CharField - Principle name
- `description`: TextField - Detailed explanation
- `experience`: IntegerField - Recommended experience level
- `client`: ForeignKey to Client
- `created_at`, `updated_at`: Timestamps

### 10.2 Odds In Your Favor

**Model**: `OddsYourFavor`

**Purpose**: Principles for stacking probabilities toward successful trades.

**Concept**: Only take trades when multiple factors align in your favor.

**Examples**:
```python
principle = OddsYourFavor.objects.create(
    name="Multi-Timeframe Confluence",
    description="""
    Trade only when:
    1. Daily timeframe shows bullish trend
    2. 4h timeframe shows bullish pattern
    3. 1h timeframe shows entry trigger
    4. Volume confirms direction
    5. RSI not overbought
    All 5 factors must align to stack odds in your favor.
    """,
    experience=3
)
```

**Application**:
- Pre-trade checklist validation
- Score trades based on number of favorable factors
- Reject trades below minimum score threshold

### 10.3 Limit Losses

**Model**: `LimitLosses`

**Purpose**: Loss mitigation strategies and best practices.

**Concept**: Protect capital through strict loss limits and disciplined exits.

**Examples**:
```python
principle = LimitLosses.objects.create(
    name="Never Risk More Than 2% Per Trade",
    description="""
    Calculate position size such that if stop-loss hits,
    total loss = 2% of account balance.

    Formula:
    position_size = (account_balance * 0.02) / (entry_price - stop_loss_price)

    This ensures no single trade can significantly damage account.
    """,
    experience=1
)
```

**Application**:
- Automatic position sizing calculation
- Pre-trade risk validation
- Daily/weekly loss limits

### 10.4 Attributes

**Model**: `Attribute`

**Purpose**: Qualitative attributes describing strategies, events, or market conditions.

**Fields** (methods):
- `context()`: Situational context
- `primary_implication()`: Main takeaway
- `underlying_objective()`: Strategic goal
- `volume()`: Volume characteristics
- `perspective()`: Market viewpoint

**Example**:
```python
attribute = Attribute.objects.create(
    name="High-Probability Setup",
    description="""
    Context: Strong uptrend with pullback to support
    Primary Implication: Likely continuation of trend
    Underlying Objective: Buy dip in established trend
    Volume: Rising on rallies, falling on pullbacks
    Perspective: Bullish with confirmation
    """,
    experience=2
)
```

**Usage**: Tag strategies and events with qualitative attributes for filtering and analysis.

---

## 11. Multi-Tenant Data Isolation

**References**: REQ-CUR-DOMAIN-028, REQ-CUR-CORE-002

### 11.1 Tenant Mixin

**Base Class**: `TenantMixin`

**Field**:
- `client`: ForeignKey to Client (nullable, on_delete=SET_NULL)

**Purpose**: Automatic data scoping by client (tenant).

**Applied To**:
- All trading entities (Symbol, Strategy, Order, Operation, Position, Trade)
- All risk rules
- All indicators

### 11.2 Automatic Filtering

**Manager**: `TenantManager` (custom QuerySet manager)

**Behavior**:
- All queries automatically filter by `client_id`
- Prevents cross-client data leakage

**Example**:
```python
# User A (client_id=1) requests orders
orders = Order.objects.all()
# SQL: SELECT * FROM orders WHERE client_id = 1

# User B (client_id=2) requests same
orders = Order.objects.all()
# SQL: SELECT * FROM orders WHERE client_id = 2
```

### 11.3 Validation

**Method**: `TenantMixin.clean()`

**Behavior**:
- Logs warning if entity created without client
- Does NOT raise error (allows superuser operations)

---

## 12. Future Planned Features

### 12.1 Enhanced Validations

#### 12.1.1 Order Quantity Validation (REQ-FUT-DOMAIN-001)

**Status**: Planned (Priority: High)

**Behavior** (when implemented):
```python
# Order.clean()
def clean(self):
    if not self.symbol.is_quantity_valid(self.quantity):
        raise ValidationError(
            f"Quantity {self.quantity} outside valid range "
            f"[{self.symbol.min_qty}, {self.symbol.max_qty or 'unlimited'}]"
        )
```

#### 12.1.2 Position Size Limits (REQ-FUT-DOMAIN-002)

**Status**: Planned (Priority: High)

**Behavior** (when implemented):
```python
# Position.add_order()
def add_order(self, order):
    # Check risk rule
    new_value = (self.quantity + order.filled_quantity) * order.avg_fill_price
    risk_limit = strategy.risk_rule.calculate_max_position_value(client.capital)

    if new_value > risk_limit:
        raise ValidationError(
            f"Adding order would exceed position size limit "
            f"({risk_limit} {quote_asset})"
        )

    # Proceed with adding order...
```

---

### 12.2 Advanced Calculations

#### 12.2.1 Risk-Adjusted Returns (REQ-FUT-DOMAIN-004)

**Status**: Planned (Priority: Low)

**Metrics** (when implemented):
- **Sharpe Ratio**: `(average_return - risk_free_rate) / volatility`
- **Sortino Ratio**: `(average_return - risk_free_rate) / downside_volatility`
- **Risk-Reward Ratio**: `average_win / average_loss`

**Example**:
```python
# Strategy properties (when implemented)
strategy.sharpe_ratio  # 1.85
strategy.sortino_ratio  # 2.10
strategy.risk_reward_ratio  # 2.5
```

#### 12.2.2 Position Netting (REQ-FUT-DOMAIN-005)

**Status**: Planned (Priority: Medium)

**Behavior** (when implemented):
- Multiple positions in same symbol auto-merge
- Opposite side orders reduce position instead of creating new
- Position flipping (long → short) handled gracefully

**Example**:
```python
# Current: Separate positions
pos1 = Position(symbol="BTCUSDT", side="LONG", quantity=Decimal("1.0"))
pos2 = Position(symbol="BTCUSDT", side="LONG", quantity=Decimal("0.5"))

# Future: Auto-netted
net_pos = Position(symbol="BTCUSDT", side="LONG", quantity=Decimal("1.5"))
```

---

### 12.3 State Machine Enhancements

#### 12.3.1 Order Timeout (REQ-FUT-DOMAIN-007)

**Status**: Planned (Priority: Medium)

**Fields** (when implemented):
- `timeout_seconds`: IntegerField, optional

**Behavior** (when implemented):
- Background job checks for expired orders
- Orders exceeding timeout auto-transition to CANCELLED

#### 12.3.2 Position Auto-Close on Stop-Loss (REQ-FUT-DOMAIN-008)

**Status**: Planned (Priority: High)

**Fields** (when implemented):
- `Position.stop_loss_price`: Decimal, optional

**Behavior** (when implemented):
1. Price monitoring detects stop-loss trigger
2. System auto-creates market exit order
3. Position transitions to CLOSED on fill
4. Event logged for audit trail

**Flow**:
```
Position (OPEN, stop_loss_price=49000)
   |
   | Price drops to 49000
   ▼
Auto-create SELL order (MARKET)
   |
   | Order fills
   ▼
Position (CLOSED)
   |
   ▼
Trade created (realized loss)
```

#### 12.3.3 Operation State Transitions (REQ-FUT-DOMAIN-009)

**Status**: Planned (Priority: Low)

**States** (when implemented):
- PLANNED: Operation configured, no orders yet
- ACTIVE: At least one entry order filled
- CLOSED: All exits filled
- CANCELLED: User cancelled

**Transitions** (when implemented):
```
PLANNED → ACTIVE (first entry fills)
ACTIVE → CLOSED (all exits fill)
PLANNED/ACTIVE → CANCELLED (user cancels)
```

---

## 13. Known Gaps and Unclear Behavior

### 13.1 Risk Rule Enforcement

**Gap**: Risk rules defined but **not enforced** at order creation.

**Impact**: Users can place orders exceeding risk limits.

**Recommendation**:
- Implement order validation hook
- Check order value against active risk rule
- Raise ValidationError if limit exceeded

**Related**: REQ-FUT-DOMAIN-002

---

### 13.2 Indicator Calculation

**Gap**: Indicator models store values but **calculation logic missing**.

**Questions**:
- Where are indicator values calculated?
- What triggers recalculation (real-time, periodic batch)?
- How are historical values stored?

**Recommendation**:
- Document indicator calculation service
- Specify calculation frequency
- Add tests with sample OHLCV data

**Related**: REQ-CUR-DOMAIN-021 to REQ-CUR-DOMAIN-026

---

### 13.3 Order-Position Linkage

**Gap**: Unclear how orders **automatically create/update** positions.

**Questions**:
- Does `Order.mark_as_filled()` trigger position creation?
- Is there a background job that syncs orders to positions?
- When is new position created vs existing position updated?

**Recommendation**:
- Document order fill → position workflow
- Add sequence diagram for order-position synchronization
- Implement and test automatic linkage

**Related**: REQ-CUR-DOMAIN-006, REQ-CUR-DOMAIN-011

---

### 13.4 Strategy-Order Association

**Gap**: Order has optional `strategy` FK but **usage not specified**.

**Questions**:
- Do strategies auto-create orders based on signals?
- Is strategy link just for tagging/grouping?
- How does strategy performance get updated from trades?

**Recommendation**:
- Clarify strategy execution mechanism
- Document how strategy config drives order creation
- Specify when `Strategy.update_performance()` is called

**Related**: REQ-CUR-DOMAIN-003, REQ-CUR-DOMAIN-004

---

### 13.5 Symbol Unique Constraint

**Gap**: Symbol has `unique_together = ["id", "client"]` which is redundant.

**Likely Intent**: `unique_together = ["name", "client"]` (symbol name unique per client)

**Recommendation**:
- Verify intent with team
- Create migration to fix constraint
- Add test for duplicate symbol name prevention

**Related**: REQ-CUR-DOMAIN-001

---

### 13.6 Operation Status Lifecycle

**Gap**: Operation has STATUS_CHOICES but **no state transition logic**.

**Impact**: Unclear when operation moves between states.

**Recommendation**:
- Implement REQ-FUT-DOMAIN-009 (operation state machine)
- Add validation to prevent invalid transitions
- Document triggers for each transition

**Related**: REQ-CUR-DOMAIN-009

---

### 13.7 Trade P&L vs Position P&L

**Gap**: Trade and Position both calculate P&L independently.

**Questions**:
- Is Trade created automatically when Position closes?
- Should `Trade.pnl == Position.realized_pnl`?
- What if fees differ between position and trade?

**Recommendation**:
- Clarify Trade creation trigger
- Ensure P&L consistency
- Add test comparing Trade.pnl to Position.realized_pnl

**Related**: REQ-CUR-DOMAIN-013, REQ-CUR-DOMAIN-015

### 13.8 Pattern Detection Implementation

**Gap**: Pattern models defined but **detection logic not implemented**.

**Questions**:
- How are patterns detected in real-time market data?
- What algorithm identifies harmonic ratios, Elliott waves, etc.?
- Is detection manual, semi-automated, or fully automated?

**Recommendation**:
- Document pattern detection service architecture
- Specify detection algorithms for each pattern type
- Add confidence score calculation methodology
- Implement automated testing with historical data

**Related**: Section 7 (Pattern Recognition System)

---

### 13.9 Analysis Workflow Enforcement

**Gap**: Technical analysis workflow (Event → Argument → Reason) defined but **not enforced**.

**Questions**:
- Is analysis workflow optional or required for trades?
- Can orders be created without linked TechnicalEvent?
- How is analysis quality validated?

**Recommendation**:
- Clarify when analysis workflow is required
- Add validation rules if analysis is mandatory
- Implement analysis completeness scoring
- Link Strategy.config to required analysis depth

**Related**: Section 9 (Technical Analysis Workflow)

---

### 13.10 Configuration Rule Enforcement

**Gap**: Configuration rules defined but **enforcement mechanism unclear**.

**Questions**:
- Where are rules checked (view layer, service layer, model clean)?
- What happens when multiple rules conflict?
- Can users override rules temporarily?

**Recommendation**:
- Implement centralized rule validation service
- Define rule priority/precedence for conflicts
- Add audit log for rule violations
- Document override mechanism (if allowed)

**Related**: Section 8 (Trading Configuration Rules)

---

## 14. Traceability

### Requirements → Specification Sections

| Requirement ID | Specification Section |
|----------------|----------------------|
| REQ-CUR-DOMAIN-001-002 | [3.1 Symbol Quantity Constraints](#31-symbol-quantity-constraints) |
| REQ-CUR-DOMAIN-003-004 | [2.5 Strategy Lifecycle](#25-strategy-lifecycle) |
| REQ-CUR-DOMAIN-005-008 | [2.1 Order Lifecycle](#21-order-lifecycle) |
| REQ-CUR-DOMAIN-009-010 | [2.4 Operation Lifecycle](#24-operation-lifecycle) |
| REQ-CUR-DOMAIN-011-013 | [2.2 Position Lifecycle](#22-position-lifecycle) |
| REQ-CUR-DOMAIN-014-017 | [2.3 Trade Lifecycle](#23-trade-lifecycle) |
| REQ-CUR-DOMAIN-018-020 | [5. Risk Management](#5-risk-management) |
| REQ-CUR-DOMAIN-021-026 | [6. Technical Indicators](#6-technical-indicators) |
| REQ-CUR-DOMAIN-027 | [11. Multi-Tenant Data Isolation](#11-multi-tenant-data-isolation) |
| REQ-CUR-DOMAIN-028 | [11. Multi-Tenant Data Isolation](#11-multi-tenant-data-isolation) |

### New Domain Areas (No Specific REQs Yet)

| Domain Area | Specification Section |
|-------------|----------------------|
| Pattern Recognition | [7. Pattern Recognition System](#7-pattern-recognition-system) |
| Trading Configuration | [8. Trading Configuration Rules](#8-trading-configuration-rules) |
| Analysis Workflow | [9. Technical Analysis Workflow](#9-technical-analysis-workflow) |
| Domain Principles | [10. Domain Principles](#10-domain-principles) |

---

## 15. Document Summary

### Coverage Statistics

**Total Sections**: 14

**Domain Entities Documented**:
- Core Trading Entities: 6 (Symbol, Strategy, Order, Operation, Position, Trade)
- Technical Indicators: 6 (MA, RSI, MACD, Bollinger Bands, Stochastic, Base)
- Pattern Types: 5 (Harmonic, Elliott Wave, Wyckoff, Chart, Candlestick, Indicator-based)
- Risk Rules: 2 (OnePercentOfCapital, JustBet4percent)
- Configuration Rules: 2 (OnlyTradeReversal, MaxTradePerDay)
- Analysis Entities: 4 (Interpretation, Event, Argument, Reason)
- Principles: 3 (OddsYourFavor, LimitLosses, Attribute)

**Total Entity Behaviors Documented**: 50+

**State Machines Defined**: 3 (Order, Position, Pattern)

**Calculation Formulas Documented**: 15+

**Known Gaps Identified**: 10

### Document Completeness

| Aspect | Status | Notes |
|--------|--------|-------|
| Core Trading Lifecycle | ✅ Complete | Orders, Positions, Trades fully specified |
| Business Rules | ✅ Complete | Validations and constraints documented |
| Calculations | ✅ Complete | All P&L and averaging formulas specified |
| Risk Management | 🟡 Partial | Models defined, enforcement pending |
| Technical Indicators | 🟡 Partial | Storage defined, calculation logic missing |
| Pattern Recognition | 🟡 Partial | Comprehensive models, detection logic missing |
| Configuration Rules | 🟡 Partial | Rules defined, enforcement mechanism unclear |
| Analysis Workflow | 🟡 Partial | Structure defined, enforcement unclear |
| Multi-Tenancy | ✅ Complete | Full isolation mechanism documented |
| Future Features | ✅ Complete | All planned enhancements cataloged |

### Next Steps for Implementation

**High Priority**:
1. Implement risk rule enforcement (Section 13.1)
2. Implement order quantity validation (Section 12.1.1)
3. Implement position size limits (Section 12.1.2)
4. Clarify order-position linkage mechanism (Section 13.3)

**Medium Priority**:
1. Implement indicator calculation service (Section 13.2)
2. Implement pattern detection algorithms (Section 13.8)
3. Clarify configuration rule enforcement (Section 13.10)
4. Implement operation state transitions (Section 12.3.3)

**Low Priority**:
1. Implement risk-adjusted returns (Section 12.2.1)
2. Implement position netting (Section 12.2.2)
3. Clarify analysis workflow enforcement (Section 13.9)

### Change History

| Version | Date | Author | Changes |
|---------|------|--------|---------|
| 1.0 | 2025-11-14 | System | Initial comprehensive specification |
| 1.1 | 2025-11-15 | Claude | Added Pattern Recognition, Configuration, Analysis, Principles sections |

---

**End of Document**
