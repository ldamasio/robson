# Robson Bot - Domain Requirements

**Purpose**: Domain model requirements defining entity behaviors, validations, invariants, business rules, and calculations.

**Last Updated**: 2025-11-14

---

## 1. Current Implementation Requirements

### 1.1 Symbol Entity

**REQ-CUR-DOMAIN-001**: Symbol Representation

**Description**: Symbols must represent trading pairs with base and quote assets.

**Rationale**: Trading requires precise identification of asset pairs.

**Source**: `apps/backend/monolith/api/models/trading.py:Symbol`

**Constraints**:
- name: CharField (max 255), uppercase, unique per client
- base_asset: CharField (max 32), uppercase
- quote_asset: CharField (max 32), uppercase
- is_active: Boolean (default True)

**Acceptance Criteria**:
- ✓ Symbol name automatically uppercase on save
- ✓ Symbol has base and quote asset fields
- ✓ Symbol can be activated/deactivated
- ✓ Symbol unique within client tenant

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_symbol*`

---

**REQ-CUR-DOMAIN-002**: Symbol Quantity Constraints

**Description**: Symbols must define minimum and optional maximum quantity constraints.

**Rationale**: Exchanges enforce minimum trade sizes and may have maximum limits.

**Source**: `apps/backend/monolith/api/models/trading.py:Symbol.min_qty, max_qty`

**Constraints**:
- min_qty: Decimal (20, 8), default 0.00000001, must be positive
- max_qty: Decimal (20, 8), nullable, must be greater than min_qty if set

**Validation Rules**:
- min_qty must be > 0
- max_qty must be > 0 if set
- max_qty must be > min_qty if set

**Acceptance Criteria**:
- ✓ Symbol validates min_qty > 0 on clean()
- ✓ Symbol validates max_qty > min_qty on clean()
- ✓ Symbol.is_quantity_valid(qty) returns True if qty in range
- ✓ Symbol.is_quantity_valid(qty) returns False if qty out of range

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_symbol_quantity_validation`

---

### 1.2 Strategy Entity

**REQ-CUR-DOMAIN-003**: Strategy Configuration

**Description**: Strategies must support flexible JSON-based configuration.

**Rationale**: Different strategies require different parameters; JSON allows schema flexibility.

**Source**: `apps/backend/monolith/api/models/trading.py:Strategy`

**Constraints**:
- config: JSONField (default empty dict)
- risk_config: JSONField (default empty dict)
- Both fields store arbitrary key-value pairs

**Acceptance Criteria**:
- ✓ Strategy can store configuration as JSON
- ✓ Strategy can store risk configuration separately
- ✓ Strategy.get_config_value(key) retrieves nested values
- ✓ Strategy.get_config_value(key) supports dotted paths (e.g., "rsi.period")
- ✓ Strategy.set_config_value(key, value) updates configuration

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_strategy_config`

---

**REQ-CUR-DOMAIN-004**: Strategy Performance Tracking

**Description**: Strategies must automatically track performance metrics (win rate, total P&L).

**Rationale**: Users need to evaluate strategy effectiveness.

**Source**: `apps/backend/monolith/api/models/trading.py:Strategy.update_performance`

**Constraints**:
- total_trades: IntegerField (default 0)
- winning_trades: IntegerField (default 0)
- total_pnl: Decimal (20, 8) (default 0)

**Calculated Properties**:
- win_rate: (winning_trades / total_trades) * 100
- average_pnl_per_trade: total_pnl / total_trades

**Acceptance Criteria**:
- ✓ Strategy.update_performance(pnl, is_winner) increments total_trades
- ✓ Strategy.update_performance(pnl, is_winner) increments winning_trades if is_winner
- ✓ Strategy.update_performance(pnl, is_winner) adds pnl to total_pnl
- ✓ Strategy.win_rate returns 0.0 if total_trades == 0
- ✓ Strategy.average_pnl_per_trade returns 0 if total_trades == 0

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_strategy_performance`

---

### 1.3 Order Entity

**REQ-CUR-DOMAIN-005**: Order State Machine

**Description**: Orders must follow defined state transitions from creation to completion.

**Rationale**: Order lifecycle must be predictable and auditable.

**Source**: `apps/backend/monolith/api/models/trading.py:Order.STATUS_CHOICES`

**States**:
- PENDING: Order created, not yet filled
- PARTIALLY_FILLED: Order partially executed
- FILLED: Order fully executed
- CANCELLED: Order cancelled before full execution
- REJECTED: Order rejected by exchange

**Valid Transitions**:
- PENDING → PARTIALLY_FILLED (first partial fill)
- PENDING → FILLED (full fill)
- PENDING → CANCELLED (user cancels)
- PENDING → REJECTED (exchange rejects)
- PARTIALLY_FILLED → FILLED (remaining quantity filled)
- PARTIALLY_FILLED → CANCELLED (user cancels partial)

**Acceptance Criteria**:
- ✓ Order created with status PENDING
- ✓ Order transitions to PARTIALLY_FILLED when 0 < filled_quantity < quantity
- ✓ Order transitions to FILLED when filled_quantity >= quantity
- ✓ Order transitions to CANCELLED when user cancels
- ✓ Order transitions to REJECTED when exchange rejects

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_order_state_transitions`

---

**REQ-CUR-DOMAIN-006**: Order Fill Tracking

**Description**: Orders must track fill progress (quantity filled, average price, timestamp).

**Rationale**: Partial fills require precise tracking for P&L calculation.

**Source**: `apps/backend/monolith/api/models/trading.py:Order.mark_as_filled`

**Constraints**:
- filled_quantity: Decimal (20, 8) (default 0)
- avg_fill_price: Decimal (20, 8), nullable
- filled_at: DateTimeField, nullable (set when fully filled)

**Calculated Properties**:
- remaining_quantity: quantity - filled_quantity
- fill_percentage: (filled_quantity / quantity) * 100
- is_filled: status == "FILLED"
- is_active: status in {PENDING, PARTIALLY_FILLED} and remaining_quantity > 0

**Acceptance Criteria**:
- ✓ Order.mark_as_filled(avg_price, filled_qty) updates filled_quantity
- ✓ Order.mark_as_filled(avg_price, filled_qty) sets avg_fill_price
- ✓ Order.mark_as_filled(avg_price) fills full quantity if filled_qty not provided
- ✓ Order.mark_as_filled sets status to FILLED if filled_quantity >= quantity
- ✓ Order.mark_as_filled sets status to PARTIALLY_FILLED if filled_quantity < quantity
- ✓ Order.mark_as_filled sets filled_at timestamp when fully filled

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_order_fill_tracking`

---

**REQ-CUR-DOMAIN-007**: Order P&L Calculation

**Description**: Orders must calculate profit and loss based on current market price.

**Rationale**: Real-time P&L is critical for trading decisions.

**Source**: `apps/backend/monolith/api/models/trading.py:Order.calculate_pnl`

**Formula**:
- BUY side: P&L = (current_price - fill_price) * quantity
- SELL side: P&L = (fill_price - current_price) * quantity
- fill_price = avg_fill_price if available, else order price

**Constraints**:
- Returns Decimal type with 8 decimal places
- Uses filled_quantity if > 0, else order quantity
- Positive P&L = profit, negative = loss

**Acceptance Criteria**:
- ✓ BUY order with current_price > fill_price returns positive P&L
- ✓ BUY order with current_price < fill_price returns negative P&L
- ✓ SELL order with current_price < fill_price returns positive P&L
- ✓ SELL order with current_price > fill_price returns negative P&L
- ✓ P&L uses avg_fill_price if available
- ✓ P&L uses order price if avg_fill_price is None

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_order_pnl_calculation`

---

**REQ-CUR-DOMAIN-008**: Order Stop-Loss Validation

**Description**: Orders with stop-loss must validate price relationship.

**Rationale**: Prevent invalid stop-loss configurations.

**Source**: `apps/backend/monolith/api/models/trading.py:Order.clean`

**Validation Rules**:
- BUY order: stop_loss_price must be < order price
- SELL order: stop_loss_price must be > order price

**Acceptance Criteria**:
- ✓ BUY order raises ValidationError if stop_loss_price >= price
- ✓ SELL order raises ValidationError if stop_loss_price <= price
- ✓ Order with no stop_loss_price passes validation

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_order_stop_loss_validation`

---

### 1.4 Operation Entity

**REQ-CUR-DOMAIN-009**: Operation Order Grouping

**Description**: Operations must group related entry and exit orders for unified P&L tracking.

**Rationale**: Trading operations often involve multiple orders (scaling in/out).

**Source**: `apps/backend/monolith/api/models/trading.py:Operation`

**Constraints**:
- entry_orders: ManyToManyField to Order
- exit_orders: ManyToManyField to Order
- stop_gain_percent: Decimal (10, 2), nullable
- stop_loss_percent: Decimal (10, 2), nullable

**Calculated Properties**:
- total_entry_quantity: sum of entry order quantities
- total_exit_quantity: sum of exit order quantities
- average_entry_price: weighted average of entry prices
- average_exit_price: weighted average of exit prices

**Acceptance Criteria**:
- ✓ Operation can associate multiple entry orders
- ✓ Operation can associate multiple exit orders
- ✓ Operation calculates total entry quantity correctly
- ✓ Operation calculates weighted average entry price
- ✓ Operation calculates weighted average exit price

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_operation_order_grouping`

---

**REQ-CUR-DOMAIN-010**: Operation Unrealized P&L

**Description**: Operations must calculate unrealized P&L for open positions.

**Rationale**: Track performance of operations not yet closed.

**Source**: `apps/backend/monolith/api/models/trading.py:Operation.calculate_unrealized_pnl`

**Formula**:
- BUY side: (current_price - avg_entry_price) * total_entry_quantity
- SELL side: (avg_entry_price - current_price) * total_entry_quantity

**Acceptance Criteria**:
- ✓ Operation calculates unrealized P&L based on current price
- ✓ BUY operation shows positive P&L when current_price > avg_entry_price
- ✓ SELL operation shows positive P&L when current_price < avg_entry_price
- ✓ Operation returns 0 if no entry orders

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_operation_unrealized_pnl`

---

### 1.5 Position Entity

**REQ-CUR-DOMAIN-011**: Position Average Price Tracking

**Description**: Positions must track average entry price as orders are added.

**Rationale**: Positions can be built incrementally; average price must adjust.

**Source**: `apps/backend/monolith/api/models/trading.py:Position.add_order`

**Formula**:
```
new_avg = (current_avg * current_qty + order_fill_price * order_qty) / (current_qty + order_qty)
```

**Acceptance Criteria**:
- ✓ Position.add_order() updates quantity
- ✓ Position.add_order() recalculates average_price
- ✓ Adding order at higher price increases average_price
- ✓ Adding order at lower price decreases average_price
- ✓ Position.add_order() ignores orders without avg_fill_price

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_position_average_price`

---

**REQ-CUR-DOMAIN-012**: Position Unrealized P&L

**Description**: Positions must calculate and update unrealized P&L based on current price.

**Rationale**: Real-time position value tracking.

**Source**: `apps/backend/monolith/api/models/trading.py:Position.update_unrealized_pnl`

**Formula**:
- Long (BUY): (current_price - average_price) * quantity
- Short (SELL): (average_price - current_price) * quantity

**Acceptance Criteria**:
- ✓ Position.update_unrealized_pnl(current_price) calculates P&L
- ✓ Position.update_unrealized_pnl(current_price) saves unrealized_pnl
- ✓ Long position shows positive P&L when current_price > average_price
- ✓ Short position shows positive P&L when current_price < average_price

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_position_unrealized_pnl`

---

**REQ-CUR-DOMAIN-013**: Position Closure

**Description**: Positions must be closeable with final P&L calculation.

**Rationale**: Track realized P&L when position is exited.

**Source**: `apps/backend/monolith/api/models/trading.py:Position.close_position`

**Behavior**:
- Sets status to CLOSED
- Sets closed_at timestamp
- Returns realized P&L

**Formula**:
- Long: (exit_price - average_price) * quantity
- Short: (average_price - exit_price) * quantity

**Acceptance Criteria**:
- ✓ Position.close_position(exit_price) sets status to CLOSED
- ✓ Position.close_position(exit_price) sets closed_at timestamp
- ✓ Position.close_position(exit_price) returns realized P&L
- ✓ Closed position has is_open == False

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_position_closure`

---

### 1.6 Trade Entity

**REQ-CUR-DOMAIN-014**: Trade Fee Tracking

**Description**: Trades must track entry and exit fees separately.

**Rationale**: Fees impact net P&L; must be accounted for.

**Source**: `apps/backend/monolith/api/models/trading.py:Trade`

**Constraints**:
- entry_fee: Decimal (20, 8) (default 0)
- exit_fee: Decimal (20, 8) (default 0)
- total_fees: calculated property (entry_fee + exit_fee)

**Acceptance Criteria**:
- ✓ Trade stores entry_fee
- ✓ Trade stores exit_fee
- ✓ Trade.total_fees returns sum of entry_fee and exit_fee

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_trade_fees`

---

**REQ-CUR-DOMAIN-015**: Trade P&L Calculation

**Description**: Trades must automatically calculate P&L on save when exit_price is set.

**Rationale**: Consistent P&L calculation across all trades.

**Source**: `apps/backend/monolith/api/models/trading.py:Trade.save`

**Formula**:
- BUY: gross_pnl = (exit_price - entry_price) * quantity
- SELL: gross_pnl = (entry_price - exit_price) * quantity
- net_pnl = gross_pnl - total_fees

**Acceptance Criteria**:
- ✓ Trade.save() calculates pnl if exit_price is set
- ✓ Trade.pnl = gross_pnl - total_fees
- ✓ BUY trade with exit_price > entry_price has positive pnl
- ✓ SELL trade with exit_price < entry_price has positive pnl
- ✓ Trade.save() does not calculate pnl if exit_price is None

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_trade_pnl_calculation`

---

**REQ-CUR-DOMAIN-016**: Trade Duration Calculation

**Description**: Trades must calculate duration between entry and exit.

**Rationale**: Duration is a key performance metric for strategies.

**Source**: `apps/backend/monolith/api/models/trading.py:Trade.duration, duration_hours`

**Calculated Properties**:
- duration: exit_time - entry_time (timedelta)
- duration_hours: duration in hours (float)

**Acceptance Criteria**:
- ✓ Trade.duration returns timedelta if both entry_time and exit_time set
- ✓ Trade.duration returns None if exit_time not set
- ✓ Trade.duration_hours returns float hours
- ✓ Trade.duration_hours returns None if exit_time not set

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_trade_duration`

---

**REQ-CUR-DOMAIN-017**: Trade Winner Identification

**Description**: Trades must identify whether they are winners or losers.

**Rationale**: Strategy win rate calculation depends on this.

**Source**: `apps/backend/monolith/api/models/trading.py:Trade.is_winner`

**Logic**:
- BUY trade: winner if exit_price > entry_price
- SELL trade: winner if exit_price < entry_price
- Not closed: is_winner = False

**Acceptance Criteria**:
- ✓ BUY trade with exit_price > entry_price has is_winner == True
- ✓ BUY trade with exit_price < entry_price has is_winner == False
- ✓ SELL trade with exit_price < entry_price has is_winner == True
- ✓ SELL trade with exit_price > entry_price has is_winner == False
- ✓ Trade without exit_price has is_winner == False

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_trade_winner`

---

### 1.7 Risk Management

**REQ-CUR-DOMAIN-018**: Risk Rule Base

**Description**: Risk rules must define percentage-based capital exposure limits.

**Rationale**: Standardize risk management across all rules.

**Source**: `apps/backend/monolith/api/models/risk.py:BaseRiskRule`

**Constraints**:
- risk_percentage: Decimal (5, 2) (e.g., 1.00 for 1%)
- max_capital_amount: Decimal (20, 2), nullable (optional hard cap)

**Validation**:
- risk_percentage must be >= 0
- risk_percentage < 0 automatically set to 0

**Acceptance Criteria**:
- ✓ Risk rule has risk_percentage field
- ✓ Risk rule has optional max_capital_amount field
- ✓ Risk rule validates risk_percentage >= 0 on clean()
- ✓ Negative risk_percentage auto-corrects to 0

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_risk_rule_validation`

---

**REQ-CUR-DOMAIN-019**: One Percent Rule

**Description**: System must provide 1% of capital risk rule.

**Rationale**: Common risk management practice in trading.

**Source**: `apps/backend/monolith/api/models/risk.py:OnePercentOfCapital`

**Constraints**:
- risk_percentage fixed at 1.00
- name defaults to "One Percent Of Capital"
- description defaults to "Limit each trade exposure to one percent of available capital."

**Acceptance Criteria**:
- ✓ OnePercentOfCapital rule has risk_percentage == 1.00
- ✓ OnePercentOfCapital.clean() sets risk_percentage to 1.00
- ✓ OnePercentOfCapital has default name and description

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_one_percent_rule`

---

**REQ-CUR-DOMAIN-020**: Four Percent Rule

**Description**: System must provide 4% of capital allocation rule.

**Rationale**: Some strategies allocate higher percentage per position.

**Source**: `apps/backend/monolith/api/models/risk.py:JustBet4percent`

**Constraints**:
- risk_percentage fixed at 4.00
- name defaults to "Just Bet 4 Percent"
- description defaults to "Allow up to four percent of capital to be allocated to a single position."

**Acceptance Criteria**:
- ✓ JustBet4percent rule has risk_percentage == 4.00
- ✓ JustBet4percent.clean() sets risk_percentage to 4.00
- ✓ JustBet4percent has default name and description

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_four_percent_rule`

---

### 1.8 Technical Indicators

**REQ-CUR-DOMAIN-021**: Indicator Timeframe

**Description**: Indicators must be associated with a specific timeframe.

**Rationale**: Same indicator can have different values for different timeframes.

**Source**: `apps/backend/monolith/api/models/indicators.py:StatisticalIndicator`

**Constraints**:
- timeframe: CharField (max 8) (default "1h")
- Common values: 1m, 5m, 15m, 30m, 1h, 4h, 1d

**Acceptance Criteria**:
- ✓ Indicator has timeframe field
- ✓ Indicator defaults to "1h" timeframe

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_indicator_timeframe`

---

**REQ-CUR-DOMAIN-022**: Moving Average

**Description**: System must store Simple Moving Average indicator values.

**Rationale**: MA is fundamental technical indicator.

**Source**: `apps/backend/monolith/api/models/indicators.py:MovingAverage`

**Constraints**:
- symbol: ForeignKey to Symbol
- period: IntegerField (e.g., 20, 50, 200)
- value: Decimal (20, 8)

**Acceptance Criteria**:
- ✓ MovingAverage linked to Symbol
- ✓ MovingAverage has period field
- ✓ MovingAverage has value field

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_moving_average`

---

**REQ-CUR-DOMAIN-023**: RSI Indicator

**Description**: System must store Relative Strength Index indicator values.

**Rationale**: RSI is key momentum indicator.

**Source**: `apps/backend/monolith/api/models/indicators.py:RSIIndicator`

**Constraints**:
- symbol: ForeignKey to Symbol
- period: IntegerField (typically 14)
- value: Decimal (20, 8) (range 0-100)

**Acceptance Criteria**:
- ✓ RSI linked to Symbol
- ✓ RSI has period field
- ✓ RSI has value field

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_rsi_indicator`

---

**REQ-CUR-DOMAIN-024**: MACD Indicator

**Description**: System must store MACD indicator values (macd, signal, histogram).

**Rationale**: MACD is key trend-following indicator.

**Source**: `apps/backend/monolith/api/models/indicators.py:MACDIndicator`

**Constraints**:
- symbol: ForeignKey to Symbol
- fast_period: IntegerField (typically 12)
- slow_period: IntegerField (typically 26)
- signal_period: IntegerField (typically 9)
- macd: Decimal (20, 8)
- signal: Decimal (20, 8)
- histogram: Decimal (20, 8)

**Acceptance Criteria**:
- ✓ MACD linked to Symbol
- ✓ MACD has fast, slow, signal periods
- ✓ MACD has macd, signal, histogram values

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_macd_indicator`

---

**REQ-CUR-DOMAIN-025**: Bollinger Bands

**Description**: System must store Bollinger Bands indicator values (upper, middle, lower).

**Rationale**: Bollinger Bands indicate volatility and support/resistance.

**Source**: `apps/backend/monolith/api/models/indicators.py:BollingerBands`

**Constraints**:
- symbol: ForeignKey to Symbol
- period: IntegerField (typically 20)
- standard_deviations: Decimal (5, 2) (default 2.00)
- upper_band: Decimal (20, 8)
- middle_band: Decimal (20, 8)
- lower_band: Decimal (20, 8)

**Acceptance Criteria**:
- ✓ BollingerBands linked to Symbol
- ✓ BollingerBands has period and std dev fields
- ✓ BollingerBands has upper, middle, lower band values

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_bollinger_bands`

---

**REQ-CUR-DOMAIN-026**: Stochastic Oscillator

**Description**: System must store Stochastic Oscillator indicator values (%K, %D, slow %D).

**Rationale**: Stochastic is key momentum oscillator.

**Source**: `apps/backend/monolith/api/models/indicators.py:StochasticOscillator`

**Constraints**:
- symbol: ForeignKey to Symbol
- period: IntegerField (default 14)
- k_value: Decimal (5, 2) (range 0-100)
- d_value: Decimal (5, 2) (range 0-100)
- slow_d_value: Decimal (5, 2) (range 0-100), optional

**Acceptance Criteria**:
- ✓ Stochastic linked to Symbol
- ✓ Stochastic has period field
- ✓ Stochastic has %K, %D, slow %D values

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_stochastic_oscillator`

---

### 1.9 Base Mixins and Common Behavior

**REQ-CUR-DOMAIN-027**: Automatic Timestamps

**Description**: All domain entities must have automatic creation and update timestamps.

**Rationale**: Audit trail and time-based queries.

**Source**: `apps/backend/monolith/api/models/base.py:TimestampMixin`

**Constraints**:
- created_at: DateTimeField (auto_now_add=True)
- updated_at: DateTimeField (auto_now=True)

**Calculated Properties**:
- age: timezone.now() - created_at
- time_since_last_update: timezone.now() - updated_at

**Acceptance Criteria**:
- ✓ Entity has created_at set on creation
- ✓ Entity has updated_at set on every save
- ✓ Entity.age returns timedelta since creation
- ✓ Entity.time_since_last_update returns timedelta since update

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_timestamp_mixin`

---

**REQ-CUR-DOMAIN-028**: Multi-Tenant Isolation

**Description**: All domain entities must be scoped to a client (tenant).

**Rationale**: Data isolation between different users/organizations.

**Source**: `apps/backend/monolith/api/models/base.py:TenantMixin`

**Constraints**:
- client: ForeignKey to Client (nullable, on_delete=SET_NULL)
- Data access filtered by client_id

**Acceptance Criteria**:
- ✓ Entity has client foreign key
- ✓ Entity.client_name returns client name safely
- ✓ Entity.clean() logs warning if created without client
- ✓ Queries automatically filter by tenant (via manager)

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_tenant_mixin`

---

## 2. Future / Planned Requirements

### 2.1 Enhanced Validations

**REQ-FUT-DOMAIN-001**: Order Quantity Symbol Validation

**Description**: Order creation should validate quantity against symbol constraints.

**Rationale**: Prevent invalid orders at creation time, not just at execution.

**Dependencies**:
- REQ-CUR-DOMAIN-002 (symbol quantity constraints)
- REQ-CUR-DOMAIN-005 (order creation)

**Priority**: High

**Estimated Complexity**: Simple

**Acceptance Criteria** (when implemented):
- [ ] Order.clean() validates quantity >= symbol.min_qty
- [ ] Order.clean() validates quantity <= symbol.max_qty if set
- [ ] ValidationError raised if quantity out of range
- [ ] Error message includes valid range

---

**REQ-FUT-DOMAIN-002**: Position Size Limits

**Description**: Positions should enforce maximum size limits based on risk rules.

**Rationale**: Prevent over-concentration in single position.

**Dependencies**:
- REQ-CUR-DOMAIN-018 (risk rules)
- REQ-CUR-DOMAIN-011 (position tracking)

**Priority**: High

**Estimated Complexity**: Moderate

**Acceptance Criteria** (when implemented):
- [ ] Position.add_order() checks total value against risk limits
- [ ] Position.add_order() raises ValidationError if limit exceeded
- [ ] Risk rule configurable per strategy
- [ ] Override mechanism for manual trades

---

**REQ-FUT-DOMAIN-003**: Strategy Active Constraint

**Description**: Only active strategies should be executable.

**Rationale**: Prevent accidental execution of disabled strategies.

**Dependencies**:
- REQ-CUR-DOMAIN-003 (strategy configuration)

**Priority**: Medium

**Estimated Complexity**: Simple

**Acceptance Criteria** (when implemented):
- [ ] Strategy execution checks is_active flag
- [ ] Inactive strategy execution raises error
- [ ] Strategy can be deactivated mid-execution (graceful stop)

---

### 2.2 Advanced Calculations

**REQ-FUT-DOMAIN-004**: Risk-Adjusted Returns

**Description**: Strategies and trades should calculate Sharpe ratio and other risk-adjusted metrics.

**Rationale**: Evaluate performance relative to risk taken.

**Dependencies**:
- REQ-CUR-DOMAIN-004 (strategy performance)
- Historical volatility data

**Priority**: Low

**Estimated Complexity**: Moderate

**Acceptance Criteria** (when implemented):
- [ ] Strategy.sharpe_ratio calculated from returns and volatility
- [ ] Strategy.sortino_ratio calculated
- [ ] Trade.risk_reward_ratio calculated
- [ ] Metrics updated periodically

---

**REQ-FUT-DOMAIN-005**: Position Netting

**Description**: System should support position netting (combining multiple positions in same symbol).

**Rationale**: Simplify position management and P&L tracking.

**Dependencies**:
- REQ-CUR-DOMAIN-011 (position average price)

**Priority**: Medium

**Estimated Complexity**: Moderate

**Acceptance Criteria** (when implemented):
- [ ] Multiple positions in same symbol auto-merge
- [ ] Net position tracks combined quantity and avg price
- [ ] Opposite side orders reduce position (not create new)
- [ ] Position flipping (long → short) handled correctly

---

**REQ-FUT-DOMAIN-006**: Trade Commission Calculation

**Description**: System should automatically calculate exchange commissions.

**Rationale**: Manual fee entry is error-prone.

**Dependencies**:
- REQ-CUR-DOMAIN-014 (fee tracking)
- Exchange commission rate configuration

**Priority**: Medium

**Estimated Complexity**: Simple

**Acceptance Criteria** (when implemented):
- [ ] System stores commission rate per exchange
- [ ] System auto-calculates entry_fee on order fill
- [ ] System auto-calculates exit_fee on position close
- [ ] Commission rate configurable per symbol/tier

---

### 2.3 State Machine Enhancements

**REQ-FUT-DOMAIN-007**: Order Timeout

**Description**: Orders should auto-cancel if not filled within timeout period.

**Rationale**: Prevent stale pending orders.

**Dependencies**:
- REQ-CUR-DOMAIN-005 (order state machine)
- Background job scheduler

**Priority**: Medium

**Estimated Complexity**: Moderate

**Acceptance Criteria** (when implemented):
- [ ] Order has optional timeout_seconds field
- [ ] Background job checks for expired orders
- [ ] Expired orders auto-transition to CANCELLED
- [ ] Timeout configurable per strategy

---

**REQ-FUT-DOMAIN-008**: Position Auto-Close on Stop-Loss

**Description**: Positions should auto-close when price hits stop-loss level.

**Rationale**: Automated risk management.

**Dependencies**:
- REQ-CUR-DOMAIN-008 (stop-loss validation)
- REQ-CUR-DOMAIN-013 (position closure)
- Real-time price monitoring

**Priority**: High

**Estimated Complexity**: Complex

**Acceptance Criteria** (when implemented):
- [ ] Position has stop_loss_price field
- [ ] Price monitoring detects stop-loss trigger
- [ ] System auto-creates exit order at market
- [ ] Position transitions to CLOSED on fill
- [ ] Stop-loss trigger logged for audit

---

**REQ-FUT-DOMAIN-009**: Operation State Transitions

**Description**: Operations should enforce state machine (PLANNED → ACTIVE → CLOSED).

**Rationale**: Structured operation lifecycle.

**Dependencies**:
- REQ-CUR-DOMAIN-009 (operation grouping)

**Priority**: Low

**Estimated Complexity**: Simple

**Acceptance Criteria** (when implemented):
- [ ] Operation starts in PLANNED state
- [ ] First entry order transitions to ACTIVE
- [ ] All exits filled transitions to CLOSED
- [ ] User cancel transitions to CANCELLED
- [ ] Invalid transitions raise error

---

## 3. Known Gaps or Unclear Behavior

### 3.1 Risk Rule Enforcement

**Gap**: Risk rules defined (REQ-CUR-DOMAIN-018-20) but **not enforced** at order creation or position update.

**Impact**: Users can violate risk rules without system prevention.

**Current State**: Risk rules exist as data models only; no validation logic.

**Recommendation**:
- Implement REQ-FUT-DOMAIN-002 (position size limits)
- Add order creation hook to check risk rules
- Create comprehensive tests for risk enforcement

---

### 3.2 Indicator Calculation

**Gap**: Indicator models exist (REQ-CUR-DOMAIN-021-26) but **calculation logic not implemented**.

**Impact**: Unclear how indicator values are populated.

**Current State**: Models store values but calculation service missing.

**Recommendation**:
- Document indicator calculation service location
- Specify calculation frequency (real-time, periodic batch)
- Add tests for indicator calculations using sample data

---

### 3.3 Order-Position Linkage

**Gap**: Unclear how orders **automatically create/update** positions.

**Impact**: Position management may be manual or external.

**Current State**: Order.mark_as_filled() exists but position creation unclear.

**Recommendation**:
- Document order fill → position creation/update flow
- Specify when new position created vs existing position updated
- Add tests for order-position synchronization

---

### 3.4 Strategy-Order Association

**Gap**: Order has optional strategy FK but **usage not specified**.

**Impact**: Unclear if strategies auto-create orders or just for tagging.

**Current State**: Strategy has config and performance tracking but no execution logic.

**Recommendation**:
- Clarify strategy execution mechanism
- Document how strategy config drives order creation
- Specify strategy performance update trigger

---

### 3.5 Symbol Unique Constraint

**Gap**: Symbol has `unique_together = ["id", "client"]` which is **redundant** (id is already unique).

**Impact**: Likely intended `unique_together = ["name", "client"]`.

**Current State**: Possible bug in model definition.

**Recommendation**:
- Verify intent: symbol name unique per client?
- If yes, change to `unique_together = ["name", "client"]`
- Add migration to enforce correct constraint
- Add test for duplicate symbol name per client

---

### 3.6 Operation Status Lifecycle

**Gap**: Operation has STATUS_CHOICES but **no state transition logic**.

**Impact**: Unclear when operation moves from PLANNED → ACTIVE → CLOSED.

**Current State**: Status field exists but transitions not enforced.

**Recommendation**:
- Implement REQ-FUT-DOMAIN-009 (operation state machine)
- Document when each transition occurs
- Add validation to prevent invalid transitions

---

### 3.7 Trade P&L vs Position P&L

**Gap**: Trade.pnl calculated on save (REQ-CUR-DOMAIN-015) but **relationship to Position P&L unclear**.

**Impact**: Possible double-counting or inconsistency.

**Current State**: Trade and Position both calculate P&L independently.

**Recommendation**:
- Clarify when Trade created from Position
- Ensure Trade.pnl == Position.realized_pnl on close
- Add test for Trade-Position P&L consistency

---

## 4. Traceability

### Current Requirements → Models

| Requirement ID       | Model / Property                              |
|----------------------|-----------------------------------------------|
| REQ-CUR-DOMAIN-001   | `Symbol` model                                |
| REQ-CUR-DOMAIN-002   | `Symbol.min_qty`, `Symbol.max_qty`, `is_quantity_valid()` |
| REQ-CUR-DOMAIN-003   | `Strategy.config`, `Strategy.risk_config`     |
| REQ-CUR-DOMAIN-004   | `Strategy.total_trades`, `win_rate`, `average_pnl_per_trade` |
| REQ-CUR-DOMAIN-005   | `Order.STATUS_CHOICES`, state transitions     |
| REQ-CUR-DOMAIN-006   | `Order.filled_quantity`, `mark_as_filled()`   |
| REQ-CUR-DOMAIN-007   | `Order.calculate_pnl()`                       |
| REQ-CUR-DOMAIN-008   | `Order.clean()`, stop-loss validation        |
| REQ-CUR-DOMAIN-009   | `Operation.entry_orders`, `exit_orders`       |
| REQ-CUR-DOMAIN-010   | `Operation.calculate_unrealized_pnl()`        |
| REQ-CUR-DOMAIN-011   | `Position.average_price`, `add_order()`       |
| REQ-CUR-DOMAIN-012   | `Position.update_unrealized_pnl()`            |
| REQ-CUR-DOMAIN-013   | `Position.close_position()`                   |
| REQ-CUR-DOMAIN-014   | `Trade.entry_fee`, `exit_fee`, `total_fees`   |
| REQ-CUR-DOMAIN-015   | `Trade.save()`, P&L auto-calculation          |
| REQ-CUR-DOMAIN-016   | `Trade.duration`, `duration_hours`            |
| REQ-CUR-DOMAIN-017   | `Trade.is_winner`                             |
| REQ-CUR-DOMAIN-018   | `BaseRiskRule.risk_percentage`                |
| REQ-CUR-DOMAIN-019   | `OnePercentOfCapital`                         |
| REQ-CUR-DOMAIN-020   | `JustBet4percent`                             |
| REQ-CUR-DOMAIN-021-26| Indicator models (MA, RSI, MACD, BB, Stochastic) |
| REQ-CUR-DOMAIN-027   | `TimestampMixin.created_at`, `updated_at`     |
| REQ-CUR-DOMAIN-028   | `TenantMixin.client`                          |

### Current Requirements → Tests

| Requirement ID       | Test Reference                                |
|----------------------|-----------------------------------------------|
| REQ-CUR-DOMAIN-001-28| `apps/backend/monolith/api/tests/test_models.py` |

*Note: Many requirements lack explicit tests - see "Known Gaps" for areas needing test coverage.*

---

**End of Document**
