# Execution Plan: Strategic Operations Phase

**Date**: 2025-12-22
**Status**: In Progress
**Author**: AI Agent + Leandro Damasio

---

## 1. Executive Summary

This plan defines the development roadmap for the **Strategic Operations Phase** of Robson Bot.
The goal is to transform the bot from simple buy/sell execution to a fully strategic trading system
that manages operations with proper risk controls.

**Key Principle**: Never lose more than 1% of capital per operation.

---

## 2. Current State

### 2.1 First Historic Trade (Completed)
- **Order ID**: 7612847320
- **Trade**: BUY 0.00033 BTC @ $88,837.92
- **Total Value**: ~$29.32 USDC
- **Status**: OPEN (awaiting strategic exit)

### 2.2 Existing Infrastructure
| Component | Status | Location |
|-----------|--------|----------|
| Operation model | ✅ Exists | `api/models/trading.py:Operation` |
| Stop Loss/Gain fields | ✅ Exists | `stop_gain_percent`, `stop_loss_percent` |
| Risk Rules (1%, 4%) | ✅ Exists | `api/models/risk.py` |
| Validation Framework | ✅ Exists | `api/application/validation.py` |
| CLI Workflow | ✅ Exists | `plan → validate → execute` |
| P&L Calculation | ✅ Exists | `Trade.pnl`, `Operation.calculate_unrealized_pnl()` |

### 2.3 Missing Components
| Component | Priority | Description |
|-----------|----------|-------------|
| Position Sizing Calculator | P0 | Calculate qty based on 1% risk rule |
| Operation Lifecycle Manager | P0 | Create/monitor/close operations |
| Real-time Price Monitor | P1 | Watch prices for stop triggers |
| Stop Loss Executor | P1 | Auto-execute stops when triggered |
| Portfolio State Tracker | P1 | Track total capital, positions, P&L |
| Margin Integration | P2 | Isolated margin support (future) |

---

## 3. Core Concepts

### 3.1 Operation vs Trade

```
OPERATION = Strategic container for a trading thesis
├── Entry Order(s)     → How we get in
├── Exit Order(s)      → How we get out
├── Stop Loss          → Protection level
├── Take Profit        → Target level
├── Strategy           → Rules governing this operation
└── Status             → PLANNED → ACTIVE → CLOSED

TRADE = Historical record of completed execution
├── Entry Price/Time
├── Exit Price/Time
├── Realized P&L
└── Fees
```

### 3.2 The 1% Rule (Position Sizing)

**Formula:**
```
Risk Per Trade = Capital × 1%
Position Size = Risk Per Trade / (Entry Price - Stop Loss Price)
```

**Example:**
```
Capital:        $1,000 USDC
Risk (1%):      $10 USDC
Entry Price:    $90,000 (BTC)
Stop Loss:      $88,200 (2% below entry)
Distance:       $1,800

Position Size = $10 / $1,800 = 0.00556 BTC
Position Value = 0.00556 × $90,000 = $500 (50% of capital)

If stopped out: Loss = 0.00556 × $1,800 = $10 = 1% of capital ✓
```

### 3.3 Risk Configuration Schema

```json
{
  "risk_config": {
    "max_risk_per_trade_percent": 1.0,
    "stop_loss_percent": 2.0,
    "take_profit_percent": 4.0,
    "max_position_size_percent": 50.0,
    "max_daily_loss_percent": 5.0,
    "max_drawdown_percent": 10.0,
    "max_concurrent_operations": 3
  }
}
```

---

## 4. Development Phases

### Phase 1: Operation Foundation (Week 1)
**Goal**: Create operation from current BTC position

#### 4.1.1 Create Operation for Current Position
```python
# What we need to do:
1. Create Operation record linking to existing Trade
2. Set stop_loss_percent and stop_gain_percent
3. Calculate actual stop loss and take profit prices
4. Update Operation status to ACTIVE
```

#### 4.1.2 Implement Position Sizing Calculator
```python
# apps/backend/core/domain/risk/position_sizing.py

class PositionSizingCalculator:
    """Calculate position size based on 1% risk rule."""
    
    def calculate(
        self,
        capital: Decimal,
        entry_price: Decimal,
        stop_loss_price: Decimal,
        max_risk_percent: Decimal = Decimal("1.0"),
        max_position_percent: Decimal = Decimal("50.0"),
    ) -> PositionSizeResult:
        """
        Calculate optimal position size.
        
        Returns:
            PositionSizeResult with:
            - quantity: Calculated position size
            - position_value: Total value of position
            - risk_amount: Maximum loss if stopped
            - position_percent: Position as % of capital
            - is_capped: True if limited by max_position_percent
        """
```

#### 4.1.3 Add Operation Endpoints
- `POST /api/operations/` - Create new operation
- `GET /api/operations/` - List operations
- `GET /api/operations/{id}/` - Get operation details
- `PATCH /api/operations/{id}/close/` - Close operation
- `GET /api/operations/active/` - Get active operations

### Phase 2: Monitoring & Alerts (Week 2)
**Goal**: Real-time awareness of operation status

#### 4.2.1 Portfolio State Service
```python
class PortfolioStateService:
    """Track real-time portfolio state."""
    
    def get_state(self, client_id: int) -> PortfolioState:
        """
        Returns:
            PortfolioState with:
            - total_capital: Current total value
            - available_capital: Capital not in positions
            - positions: List of open positions
            - unrealized_pnl: Total unrealized P&L
            - daily_pnl: Today's realized P&L
            - operations: Active operations
        """
```

#### 4.2.2 Price Monitor Service
```python
class PriceMonitorService:
    """Monitor prices for stop/target triggers."""
    
    def check_operations(self) -> List[TriggerEvent]:
        """
        Check all active operations against current prices.
        Returns list of triggered stops/targets.
        """
```

#### 4.2.3 Endpoints
- `GET /api/portfolio/state/` - Current portfolio state
- `GET /api/portfolio/pnl/` - P&L summary
- `GET /api/operations/{id}/status/` - Real-time operation status

### Phase 3: Automated Execution (Week 3)
**Goal**: Automatic stop loss and take profit execution

#### 4.3.1 Operation Executor Service
```python
class OperationExecutorService:
    """Execute operation actions (stops, targets, manual close)."""
    
    def execute_stop_loss(self, operation: Operation) -> ExecutionResult:
        """Execute stop loss order."""
        
    def execute_take_profit(self, operation: Operation) -> ExecutionResult:
        """Execute take profit order."""
        
    def close_operation(self, operation: Operation, reason: str) -> ExecutionResult:
        """Close operation with reason."""
```

#### 4.3.2 Background Monitoring Task
```python
# Celery task or Django management command
@periodic_task(run_every=timedelta(seconds=1))
def monitor_operations():
    """Check all active operations and trigger stops/targets."""
```

### Phase 4: Advanced Features (Week 4+)
**Goal**: Enhanced risk management and margin support

#### 4.4.1 Trailing Stop Loss
- Dynamic stop that follows price
- Lock in profits as price moves favorably

#### 4.4.2 Partial Position Sizing
- Scale into positions
- Take partial profits

#### 4.4.3 Margin Isolated (Future)
- Leverage with isolated margin
- Strict position limits for leveraged trades

---

## 5. Database Schema Updates

### 5.1 Operation Enhancements
```python
class Operation(BaseModel):
    # Existing fields...
    
    # New fields for strategic operations
    entry_price = models.DecimalField(...)  # Actual entry price
    stop_loss_price = models.DecimalField(...)  # Calculated stop price
    take_profit_price = models.DecimalField(...)  # Calculated target price
    
    # Risk tracking
    risk_amount = models.DecimalField(...)  # Max loss in quote currency
    position_value = models.DecimalField(...)  # Total position value
    
    # Timestamps
    activated_at = models.DateTimeField(null=True)
    closed_at = models.DateTimeField(null=True)
    close_reason = models.CharField(...)  # STOP_LOSS, TAKE_PROFIT, MANUAL, etc.
```

### 5.2 Portfolio Snapshot
```python
class PortfolioSnapshot(BaseModel):
    """Point-in-time portfolio state for historical analysis."""
    
    timestamp = models.DateTimeField()
    total_value = models.DecimalField(...)
    positions_value = models.DecimalField(...)
    available_capital = models.DecimalField(...)
    unrealized_pnl = models.DecimalField(...)
    realized_pnl_day = models.DecimalField(...)
    realized_pnl_month = models.DecimalField(...)
    realized_pnl_year = models.DecimalField(...)
```

---

## 6. Immediate Next Steps

### Step 1: Create Operation for Current Position (TODAY)
```bash
# In production pod, create operation:
python manage.py shell

from api.models import Operation, Trade, Strategy, Symbol
from decimal import Decimal

# Get the current open trade
trade = Trade.objects.get(id=1)
symbol = trade.symbol

# Create or get a default strategy
strategy, _ = Strategy.objects.get_or_create(
    name="BTC Spot Manual",
    defaults={
        "description": "Manual BTC spot trading strategy",
        "config": {},
        "risk_config": {
            "max_risk_per_trade_percent": 1,
            "stop_loss_percent": 2,
            "take_profit_percent": 4,
            "max_position_size_percent": 50,
            "max_daily_loss_percent": 5,
        },
        "is_active": True,
    }
)

# Create operation
operation = Operation.objects.create(
    symbol=symbol,
    strategy=strategy,
    side="BUY",
    status="ACTIVE",
    stop_gain_percent=Decimal("4.0"),  # Target +4%
    stop_loss_percent=Decimal("2.0"),  # Stop -2%
)

# Link entry order
order = Order.objects.get(binance_order_id="7612847320")
operation.entry_orders.add(order)

print(f"Operation created: {operation.id}")
print(f"Stop Loss: {trade.entry_price * Decimal('0.98')}")  # -2%
print(f"Take Profit: {trade.entry_price * Decimal('1.04')}")  # +4%
```

### Step 2: Implement Position Sizing Calculator (This Week)
- Create `apps/backend/core/domain/risk/position_sizing.py`
- Add unit tests
- Integrate with Operation creation

### Step 3: Create Portfolio State Endpoint (This Week)
- Implement `GET /api/portfolio/state/`
- Return real-time portfolio snapshot
- Include active operations

---

## 7. Success Metrics

| Metric | Target | Measurement |
|--------|--------|-------------|
| Max loss per operation | ≤ 1% of capital | Track actual vs expected |
| Stop loss execution | < 5 seconds from trigger | Monitor latency |
| P&L accuracy | 100% match with Binance | Daily reconciliation |
| Operation completion rate | 100% closed properly | No orphaned operations |

---

## 8. Risk Considerations

### 8.1 Market Risks
- **Gap risk**: Price may gap past stop loss
- **Slippage**: Market orders may execute at worse price
- **Liquidity**: Large positions may impact price

### 8.2 Technical Risks
- **Downtime**: Bot may be offline when stop triggers
- **API limits**: Binance rate limits
- **Network latency**: Delay in order execution

### 8.3 Mitigations
- Use OCO orders on Binance (One-Cancels-Other)
- Implement heartbeat monitoring
- Add redundant price sources
- Create emergency stop procedures

---

## 9. References

- `docs/specs/robson-domain-spec.md` - Domain specification
- `docs/requirements/robson-domain-requirements.md` - Requirements
- `docs/STRATEGY.md` - Strategy guidelines
- `apps/backend/monolith/api/models/trading.py` - Trading models
- `apps/backend/monolith/api/models/risk.py` - Risk models
- `apps/backend/monolith/api/application/validation.py` - Validators

---

**Next Action**: Create Operation for current BTC position and implement position sizing calculator.

