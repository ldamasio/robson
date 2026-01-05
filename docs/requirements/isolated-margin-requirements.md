# Isolated Margin Trading Requirements

**Document ID**: REQ-FUT-MARGIN  
**Status**: Draft  
**Created**: 2024-12-23  
**Author**: AI-assisted development  

---

## 1. Overview

This document specifies requirements for **Isolated Margin Trading** functionality in Robson Bot. Isolated Margin allows users to trade with leverage while limiting risk to the margin allocated for each specific position.

### 1.1 Business Context

Robson aims to become a medium-frequency trading robot supporting approximately **50 orders per day**. Margin trading is essential for this use case as it enables:

- Efficient capital utilization
- Ability to profit from small price movements
- Hedging capabilities
- Professional-grade trading operations

### 1.2 Risk Management Philosophy

**Core Principle**: USER decides WHEN to trade, ROBSON calculates HOW MUCH and PROTECTS capital.

| Rule | Description | Implementation |
|------|-------------|----------------|
| 1% Per Trade | Maximum loss per single operation | Position sizing based on stop distance |
| 4% Monthly Drawdown | Maximum cumulative monthly loss | Auto-pause when limit reached |
| Isolated Margin | Risk limited to allocated margin | No cross-contamination between positions |
| Technical Stop | Stop-loss based on chart analysis | User provides stop, system calculates size |

---

## 2. Future Requirements (REQ-FUT-MARGIN-*)

### 2.1 Margin Account Management

#### REQ-FUT-MARGIN-001: Transfer Spot to Isolated Margin

**Description**: System must support transferring assets from Spot wallet to Isolated Margin account for a specific trading pair.

**Rationale**: Users need to allocate capital to margin positions before opening trades.

**Dependencies**:
- REQ-CUR-DOMAIN-001 (Binance API integration)
- Valid Binance API credentials with margin permissions

**Priority**: High (P0)

**Estimated Complexity**: Moderate

**Acceptance Criteria**:
- [ ] User can transfer USDC from Spot to Isolated Margin for BTCUSDC pair
- [ ] Transfer amount is validated against available Spot balance
- [ ] Transfer is recorded in audit trail
- [ ] System returns confirmation with new margin balance
- [ ] Error handling for insufficient balance, invalid pair, API errors

**Binance API Reference**:
```python
client.transfer_spot_to_isolated_margin(
    asset='USDC',
    symbol='BTCUSDC',
    amount='100.00'
)
```

---

#### REQ-FUT-MARGIN-002: Transfer Isolated Margin to Spot

**Description**: System must support transferring assets from Isolated Margin account back to Spot wallet.

**Rationale**: Users need to withdraw profits or reallocate capital.

**Dependencies**:
- REQ-FUT-MARGIN-001

**Priority**: High (P0)

**Estimated Complexity**: Simple

**Acceptance Criteria**:
- [ ] User can transfer available (non-locked) assets back to Spot
- [ ] System validates transfer won't cause margin call
- [ ] Transfer is recorded in audit trail
- [ ] Error handling for locked funds, active positions

**Binance API Reference**:
```python
client.transfer_isolated_margin_to_spot(
    asset='USDC',
    symbol='BTCUSDC',
    amount='100.00'
)
```

---

#### REQ-FUT-MARGIN-003: Query Isolated Margin Account

**Description**: System must retrieve current Isolated Margin account status including balances, positions, and margin levels.

**Rationale**: Required for position sizing calculations and risk monitoring.

**Dependencies**:
- REQ-CUR-DOMAIN-001 (Binance API integration)

**Priority**: High (P0)

**Estimated Complexity**: Simple

**Acceptance Criteria**:
- [ ] Retrieve base asset balance (e.g., BTC)
- [ ] Retrieve quote asset balance (e.g., USDC)
- [ ] Retrieve margin level (margin ratio)
- [ ] Retrieve liquidation price
- [ ] Retrieve borrowed amounts
- [ ] Retrieve interest owed

**Binance API Reference**:
```python
client.get_isolated_margin_account()
```

---

### 2.2 Margin Order Execution

#### REQ-FUT-MARGIN-004: Place Isolated Margin Market Order

**Description**: System must support placing MARKET orders on Isolated Margin account.

**Rationale**: Market orders are essential for immediate execution in medium-frequency trading.

**Dependencies**:
- REQ-FUT-MARGIN-001 (Transfer to margin)
- REQ-FUT-MARGIN-003 (Query account)

**Priority**: High (P0)

**Estimated Complexity**: Moderate

**Acceptance Criteria**:
- [ ] Support BUY and SELL sides
- [ ] Validate quantity against position sizing rules (1% max risk)
- [ ] Record order in database with margin flag
- [ ] Capture Binance order ID for tracking
- [ ] Support both opening and closing positions
- [ ] Handle partial fills

**Binance API Reference**:
```python
client.create_margin_order(
    symbol='BTCUSDC',
    side='BUY',
    type='MARKET',
    quantity=0.001,
    isIsolated='TRUE'
)
```

---

#### REQ-FUT-MARGIN-005: Place Isolated Margin Limit Order

**Description**: System must support placing LIMIT orders on Isolated Margin account.

**Rationale**: Limit orders allow precise entry at desired price levels.

**Dependencies**:
- REQ-FUT-MARGIN-001 (Transfer to margin)
- REQ-FUT-MARGIN-003 (Query account)

**Priority**: High (P1)

**Estimated Complexity**: Moderate

**Acceptance Criteria**:
- [ ] Support BUY and SELL sides
- [ ] Support GTC (Good Till Cancelled) time in force
- [ ] Validate price and quantity
- [ ] Record order in database with margin flag
- [ ] Support order modification (price/quantity)
- [ ] Support order cancellation

**Binance API Reference**:
```python
client.create_margin_order(
    symbol='BTCUSDC',
    side='BUY',
    type='LIMIT',
    timeInForce='GTC',
    quantity=0.001,
    price=95000.00,
    isIsolated='TRUE'
)
```

---

#### REQ-FUT-MARGIN-006: Cancel Isolated Margin Order

**Description**: System must support cancelling open Isolated Margin orders.

**Rationale**: Users need to cancel orders that are no longer valid.

**Dependencies**:
- REQ-FUT-MARGIN-004 or REQ-FUT-MARGIN-005

**Priority**: High (P1)

**Estimated Complexity**: Simple

**Acceptance Criteria**:
- [ ] Cancel by order ID
- [ ] Update order status in database
- [ ] Record cancellation in audit trail
- [ ] Handle already-filled orders gracefully

---

#### REQ-FUT-MARGIN-007: Internal Stop Execution (Robson Monitor)

**Description**: System must execute stop-loss for isolated margin positions
via Robson infrastructure, using a **market order** when the stop price is hit.

**Rationale**: For Iron Exit Protocol, stop-loss is not pre-placed on Binance.
Robson monitors price and executes the exit to ensure reliability.

**Dependencies**:
- REQ-FUT-MARGIN-003 (Query account)
- Stop monitor infrastructure (cron/worker)

**Priority**: High (P0)

**Estimated Complexity**: Moderate

**Acceptance Criteria**:
- [ ] Stop price is stored on the Operation (Level 2)
- [ ] No STOP_LOSS_LIMIT order is placed on Binance for Iron Exit Protocol
- [ ] Stop monitor executes a **market** close at trigger
- [ ] Stop execution is recorded in AuditService (`STOP_LOSS_TRIGGERED`)
- [ ] Margin position is closed with AUTO_REPAY where applicable

---

### 2.3 Position Management

#### REQ-FUT-MARGIN-008: Position Sizing for Margin

**Description**: System must calculate optimal position size for margin trades using the 1% risk rule, accounting for leverage.

**Rationale**: Core Robson intelligence - protecting user capital.

**Dependencies**:
- REQ-CUR-DOMAIN-002 (Position sizing calculator)
- REQ-FUT-MARGIN-003 (Query account)

**Priority**: Critical (P0)

**Estimated Complexity**: Moderate

**Acceptance Criteria**:
- [ ] Calculate position size where max loss = 1% of total capital
- [ ] Account for leverage in margin calculations
- [ ] Ensure margin allocated <= available margin
- [ ] Warn if position would approach liquidation
- [ ] Cap position at 50% of available margin (safety buffer)

**Formula**:
```
Capital = Isolated Margin Equity (quote net + base net * price)
Risk Amount = Capital × 0.01 (1% rule)
Stop Distance = |Entry Price - Stop Price|
Base Quantity = Risk Amount / Stop Distance

Margin Required = (Base Quantity × Entry Price) / Leverage
If Margin Required > Available Margin:
    Reduce position proportionally
```

---

#### REQ-FUT-MARGIN-009: Margin Stop-Loss Handling

**Description**: System must support stop-loss handling for margin positions.

**Rationale**: Critical for risk management - prevents losses beyond 1% per trade.

**Dependencies**:
- REQ-FUT-MARGIN-004 or REQ-FUT-MARGIN-005
- REQ-CUR-DOMAIN-003 (Stop monitor)

**Priority**: Critical (P0)

**Estimated Complexity**: Complex

**Acceptance Criteria**:
- [ ] For Robson-monitored strategies, stop is internal (no exchange stop order)
- [ ] For exchange-managed strategies, support STOP_LOSS_LIMIT placement
- [ ] Stop price from user's technical analysis
- [ ] Monitor stop status continuously
- [ ] Handle stop execution events
- [ ] Record P&L when stop triggered
- [ ] Update monthly drawdown tracking

**Binance API Reference**:
```python
client.create_margin_order(
    symbol='BTCUSDC',
    side='SELL',  # Opposite of position
    type='STOP_LOSS_LIMIT',
    timeInForce='GTC',
    quantity=0.001,
    price=88000.00,      # Limit price
    stopPrice=88200.00,  # Trigger price
    isIsolated='TRUE'
)
```

---

#### REQ-FUT-MARGIN-010: Margin Take-Profit Orders

**Description**: System must support take-profit orders to lock in gains.

**Rationale**: Allows systematic profit-taking without manual intervention.

**Dependencies**:
- REQ-FUT-MARGIN-004 or REQ-FUT-MARGIN-005

**Priority**: Medium (P1)

**Estimated Complexity**: Moderate

**Acceptance Criteria**:
- [ ] Support TAKE_PROFIT_LIMIT order type
- [ ] User specifies target price
- [ ] Optional: Trailing take-profit
- [ ] Record profit when executed
- [ ] Update performance metrics

---

### 2.4 Risk Management

#### REQ-FUT-MARGIN-011: Margin Level Monitoring

**Description**: System must continuously monitor margin level to prevent liquidation.

**Rationale**: Protect users from catastrophic losses due to liquidation.

**Dependencies**:
- REQ-FUT-MARGIN-003 (Query account)

**Priority**: Critical (P0)

**Estimated Complexity**: Moderate

**Acceptance Criteria**:
- [ ] Alert when margin level < 1.3 (WARNING)
- [ ] Alert when margin level < 1.1 (CRITICAL)
- [ ] Option to auto-reduce position at WARNING level
- [ ] Notify user via configured channels
- [ ] Log all margin level events

**Margin Level Calculation**:
```
Margin Level = Total Asset Value / (Total Borrowed + Total Interest)

Level >= 2.0   → SAFE (can open new positions)
Level >= 1.3   → CAUTION (no new positions recommended)
Level >= 1.1   → WARNING (consider reducing position)
Level < 1.1    → CRITICAL (approaching liquidation)
Level <= 1.0   → LIQUIDATION
```

---

#### REQ-FUT-MARGIN-012: Monthly Drawdown Tracking (Margin)

**Description**: System must track cumulative monthly P&L including margin trades, enforcing 4% max drawdown.

**Rationale**: Prevent catastrophic monthly losses.

**Dependencies**:
- REQ-CUR-DOMAIN-004 (PolicyState)
- REQ-FUT-MARGIN-004 (Margin orders)

**Priority**: Critical (P0)

**Estimated Complexity**: Moderate

**Acceptance Criteria**:
- [ ] Include margin trade P&L in monthly tracking
- [ ] Include unrealized margin P&L in calculations
- [ ] Auto-pause ALL trading when 4% drawdown reached
- [ ] Require manual override to resume after pause
- [ ] Alert at 2% and 3% drawdown levels

---

#### REQ-FUT-MARGIN-013: Daily Trade Limit

**Description**: System must enforce maximum 50 trades per day for medium-frequency operation.

**Rationale**: Prevent overtrading and ensure quality over quantity.

**Dependencies**:
- REQ-FUT-MARGIN-004 (Margin orders)

**Priority**: Medium (P1)

**Estimated Complexity**: Simple

**Acceptance Criteria**:
- [ ] Count all trades (Spot + Margin) per calendar day
- [ ] Block new trades when limit reached
- [ ] Warn user at 40 trades (80% of limit)
- [ ] Reset counter at 00:00 UTC
- [ ] Allow override for closing positions only

---

### 2.5 Borrowing and Interest

#### REQ-FUT-MARGIN-014: Automatic Borrow on Trade

**Description**: System must handle automatic borrowing when placing margin orders.

**Rationale**: Binance Isolated Margin can auto-borrow when placing orders.

**Dependencies**:
- REQ-FUT-MARGIN-004 or REQ-FUT-MARGIN-005

**Priority**: Medium (P1)

**Estimated Complexity**: Moderate

**Acceptance Criteria**:
- [ ] Support `sideEffectType=MARGIN_BUY` for auto-borrow on buy
- [ ] Track borrowed amounts per position
- [ ] Calculate and display interest accrued
- [ ] Include interest in P&L calculations

**Binance API Reference**:
```python
client.create_margin_order(
    symbol='BTCUSDC',
    side='BUY',
    type='MARKET',
    quantity=0.001,
    isIsolated='TRUE',
    sideEffectType='MARGIN_BUY'  # Auto-borrow
)
```

---

#### REQ-FUT-MARGIN-015: Repay Borrowed Assets

**Description**: System must support repaying borrowed assets.

**Rationale**: Reduce interest charges and close margin positions cleanly.

**Dependencies**:
- REQ-FUT-MARGIN-014

**Priority**: Medium (P1)

**Estimated Complexity**: Simple

**Acceptance Criteria**:
- [ ] Repay specific amount or all borrowed
- [ ] Handle interest repayment
- [ ] Support `AUTO_REPAY` on close
- [ ] Update position status after repayment

---

## 3. Domain Model Extensions

### 3.1 New Entities

```python
@dataclass
class MarginPosition:
    """Represents an Isolated Margin position."""
    position_id: str
    client_id: int
    operation_id: Optional[int]  # Link to Operation (Level 2)
    symbol: str
    side: str  # "LONG" or "SHORT"
    
    # Position details
    entry_price: Decimal
    quantity: Decimal
    leverage: int
    
    # Risk parameters
    stop_price: Decimal
    target_price: Optional[Decimal]
    
    # Margin details
    margin_allocated: Decimal
    borrowed_amount: Decimal
    interest_accrued: Decimal
    
    # P&L
    unrealized_pnl: Decimal
    realized_pnl: Decimal
    
    # Status
    status: MarginPositionStatus  # OPEN, CLOSED, LIQUIDATED
    margin_level: Decimal
    
    # Timestamps
    opened_at: datetime
    closed_at: Optional[datetime]


class MarginPositionStatus(str, Enum):
    OPEN = "OPEN"
    CLOSED = "CLOSED"
    LIQUIDATED = "LIQUIDATED"
    STOPPED_OUT = "STOPPED_OUT"
```

### 3.2 Extended Ports

```python
class MarginExecutionPort(Protocol):
    """Port for Isolated Margin operations."""
    
    def transfer_to_margin(
        self, symbol: str, asset: str, amount: Decimal
    ) -> TransferResult: ...
    
    def transfer_from_margin(
        self, symbol: str, asset: str, amount: Decimal
    ) -> TransferResult: ...
    
    def get_margin_account(self, symbol: str) -> MarginAccountInfo: ...
    
    def place_margin_order(
        self,
        symbol: str,
        side: str,
        order_type: str,
        quantity: Decimal,
        price: Optional[Decimal] = None,
        stop_price: Optional[Decimal] = None,
    ) -> MarginOrderResult: ...
    
    def cancel_margin_order(
        self, symbol: str, order_id: str
    ) -> bool: ...
    
    def get_margin_level(self, symbol: str) -> Decimal: ...
```

---

## 4. API Endpoints (Planned)

| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/api/margin/transfer/to-margin/` | Transfer Spot → Isolated Margin |
| POST | `/api/margin/transfer/to-spot/` | Transfer Isolated Margin → Spot |
| GET | `/api/margin/account/{symbol}/` | Get margin account status |
| POST | `/api/margin/order/` | Place margin order |
| DELETE | `/api/margin/order/{orderId}/` | Cancel margin order |
| GET | `/api/margin/positions/` | List open margin positions |
| GET | `/api/margin/positions/{positionId}/` | Get position details |
| POST | `/api/margin/positions/{positionId}/close/` | Close position |

---

## 5. Traceability

### 5.1 Specifications (to be created)

- `SPEC-MARGIN-TRANSFER-001`: Transfer operations spec
- `SPEC-MARGIN-ORDER-001`: Order execution spec
- `SPEC-MARGIN-RISK-001`: Risk management spec

### 5.2 Code Locations (after implementation)

- Domain: `apps/backend/core/domain/margin.py`
- Ports: `apps/backend/core/application/ports.py` (extended)
- Adapters: `apps/backend/monolith/api/application/margin_adapters.py`
- Use Cases: `apps/backend/core/application/margin_use_cases.py`
- Views: `apps/backend/monolith/api/views/margin_views.py`
- Models: `apps/backend/monolith/api/models/margin.py`

### 5.3 Related ADRs

- ADR-0001: Binance Service Singleton (extends to margin)
- ADR-0007: Robson is Risk Assistant (margin follows same philosophy)
- ADR-NEW: Isolated Margin Architecture (to be created)

---

## 6. Implementation Priority

### Phase 1: Foundation (Critical)
1. REQ-FUT-MARGIN-001: Transfer to margin
2. REQ-FUT-MARGIN-002: Transfer from margin
3. REQ-FUT-MARGIN-003: Query account
4. REQ-FUT-MARGIN-007: Internal stop execution
5. REQ-FUT-MARGIN-008: Position sizing

### Phase 2: Trading (Critical)
6. REQ-FUT-MARGIN-004: Market orders
7. REQ-FUT-MARGIN-009: Stop-loss handling
8. REQ-FUT-MARGIN-011: Margin monitoring
9. REQ-FUT-MARGIN-012: Drawdown tracking

### Phase 3: Advanced (High)
10. REQ-FUT-MARGIN-005: Limit orders
11. REQ-FUT-MARGIN-006: Cancel orders
12. REQ-FUT-MARGIN-010: Take-profit orders

### Phase 4: Polish (Medium)
13. REQ-FUT-MARGIN-013: Daily trade limit
14. REQ-FUT-MARGIN-014: Auto-borrow
15. REQ-FUT-MARGIN-015: Repay borrowed

---

## 7. Open Questions

1. **Leverage Selection**: Should leverage be user-configurable or fixed (e.g., 3x)?
2. **Multiple Pairs**: Support BTCUSDC only initially, or multiple pairs?
3. **Cross-Margin Fallback**: If Isolated Margin fails, should we support Cross Margin?
4. **Interest Calculation**: How frequently to update interest in P&L?

---

**Last Updated**: 2024-12-23  
**Version**: 1.0 (Draft)
