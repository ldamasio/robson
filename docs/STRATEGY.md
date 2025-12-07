# Strategy Engine Guide

## Overview

The Strategy Engine is the core component responsible for analyzing market data and generating trading signals. In Robson Bot, a Strategy is a pluggable component that encapsulates trading logic, risk parameters, and execution rules.

## Strategy Lifecycle

1.  **Development**: Implementation of the trading logic using the Strategy Interface (Core Domain).
2.  **Backtesting**: Validation against historical data (out-of-sample testing).
3.  **Paper Trading**: Real-time execution with virtual funds.
4.  **Live Trading**: Real-time execution with real assets.

## Architecture

### The `Strategy` Entity

Located at: `apps/backend/monolith/api/models/trading.py`

Key attributes:
-   **`config`** (JSON): Dynamic parameters (e.g., `{"rsi_period": 14, "threshold": 30}`).
-   **`risk_config`** (JSON): Risk management rules (e.g., `{"max_drawdown": 0.05}`).
-   **`is_active`**: Global switch for the strategy.
-   **Performance Metrics**: `win_rate`, `total_pnl`, `average_pnl_per_trade`.

### Hexagonal Flow

Strategies follow the Ports & Adapters architecture:

1.  **Driver**: Scheduled Job or Real-time Event triggers analysis.
2.  **Input Port**: `MarketDataPort` provides normalized OHLCV data.
3.  **Domain**: Pure Python strategy logic processes data (No Django imports).
4.  **Output Port**: `OrderExecutionPort` or `SignalService` receives the recommendation.

## Implementation Guide

### 1. Strategy Protocol

All core strategies must implement the protocol defined in the domain layer.

```python
# apps/backend/core/domain/strategy.py (Conceptual)
from typing import Protocol, List
from .trade import Signal, MarketData

class StrategyProtocol(Protocol):
    def analyze(self, market_data: List[MarketData]) -> Signal:
        """
        Analyze market data and return a trading signal.
        
        Args:
            market_data: List of OHLCV candles
            
        Returns:
            Signal: BUY, SELL, or HOLD recommendation with strength
        """
        ...
```

### 2. Configuration Schema

Strategies utilize a JSON configuration (`config` field) to allow runtime tuning without code changes.

**Example Config:**
```json
{
  "timeframe": "1h",
  "indicators": {
    "rsi": { "period": 14, "overbought": 70, "oversold": 30 },
    "macd": { "fast": 12, "slow": 26, "signal": 9 }
  },
  "lookback_periods": 100
}
```

### 3. Risk Management

Before generating a signal, the strategy logic must adhere to risk constraints defined in `risk_config`.

**Risk Rules:**
-   **Position Sizing**: Never calculate size in the strategy; return a signal strength (0.0 - 1.0). The `PortfolioService` determines size.
-   **Stop Loss / Take Profit**: Must be suggested by the strategy based on volatility (e.g., ATR) or fixed percentages.

## Best Practices

1.  **Idempotency**: Strategy execution must be idempotent. Analyzing the same data twice should yield the same signal.
2.  **Statelessness**: Ideally, strategies should not store state in memory. Use the database or cache if state between runs is required.
3.  **Isolation**: Strategies should not be aware of other running strategies or global account state (unless implementing a portfolio strategy).
4.  **Error Handling**: Fail gracefully. If data is missing (e.g., `None` or empty list), return a `HOLD` signal or raise a domain specific exception, do not crash the engine.

## Adding a New Strategy

1.  **Define Logic**: Create the strategy logic in `apps/backend/core/domain/strategies/`.
2.  **Create Schema**: Define the expected JSON structure for `config`.
3.  **Register**: Add the strategy identifier to the strategy factory/registry.
4.  **Backtest**: Run historical data through the `analyze` method.
5.  **Deploy**: Create a `Strategy` record in the database with `is_active=False` initially.

