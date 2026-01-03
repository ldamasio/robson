"""
Application layer use cases for the trading system.

Use cases implement business logic and orchestrate domain entities,
repositories, and external services through ports.

Hexagonal Architecture INSIDE Django:
- Use cases are framework-agnostic (no Django-specific code here)
- Dependencies are injected via ports
- Django adapters implement these ports

ONE system, ONE runtime, ONE source of truth.
"""

from __future__ import annotations
from decimal import Decimal
from .ports import (
    OrderRepository,
    MarketDataPort,
    ExchangeExecutionPort,
    EventBusPort,
    ClockPort,
    UnitOfWork,
)


class PlaceOrderUseCase:
    """
    Use case for placing a trading order.

    This orchestrates:
    1. Fetching market price (if no limit specified)
    2. Placing the order on the exchange
    3. Persisting the order to the database
    4. Publishing an event

    The use case is framework-agnostic - it doesn't know about Django.
    All external dependencies are injected as ports.
    """

    def __init__(
        self,
        repo: OrderRepository,
        md: MarketDataPort,
        ex: ExchangeExecutionPort,
        bus: EventBusPort,
        clock: ClockPort,
        uow: UnitOfWork | None = None,
    ):
        self.repo = repo
        self.md = md
        self.ex = ex
        self.bus = bus
        self.clock = clock
        self.uow = uow

    def execute(
        self,
        symbol: object,
        side: str,
        qty: Decimal,
        limit_price: Decimal | None = None,
    ) -> object:
        """
        Execute a place order operation.

        Args:
            symbol: Trading symbol (Symbol domain object or string)
            side: "BUY" or "SELL"
            qty: Quantity to trade
            limit_price: Optional limit price (uses market price if None)

        Returns:
            The persisted order as a dict

        Raises:
            ValueError: If side is invalid or parameters are malformed
        """
        if side not in {"BUY", "SELL"}:
            raise ValueError("invalid side")

        # Determine price
        px = limit_price
        if px is None:
            px = self.md.best_ask(symbol) if side == "BUY" else self.md.best_bid(symbol)

        # Create order dict (lightweight, no domain coupling)
        order = {
            "id": f"ord_{int(self.clock.now().timestamp()*1000)}",
            "symbol": symbol,
            "side": side,
            "qty": qty,
            "price": px,
            "created_at": self.clock.now(),
        }

        # Execute within transaction context
        ctx = self.uow if self.uow is not None else _NullUoW()
        with ctx:
            # Place order on exchange
            ext_id = self.ex.place_limit(order)

            # Update order with external ID
            persisted = dict(order, id=ext_id)

            # Persist to database
            self.repo.save(persisted)

            # Publish domain event
            self.bus.publish(
                "orders.placed",
                {
                    "id": persisted["id"],
                    "symbol": getattr(symbol, "as_pair", lambda: str(symbol))(),
                    "side": side,
                    "qty": str(qty),
                    "price": str(px),
                },
            )

            # Commit transaction
            ctx.commit()

        return persisted


class AutoCalculateTradingParametersUseCase:
    """
    Use case for auto-calculating trading parameters from strategy configuration.

    This orchestrates:
    1. Determining trade side from strategy's market_bias or config
    2. Determining capital allocation from strategy config
    3. Calculating technical stop-loss using market data
    4. Calculating position size based on 1% risk rule

    Used by both create_trading_intent and auto_calculate_parameters endpoints
    to avoid logic duplication.
    """

    def __init__(self, tech_stop_service, timeout: float = 5.0):
        """
        Initialize the use case.

        Args:
            tech_stop_service: BinanceTechnicalStopService instance
            timeout: Timeout for API calls in seconds
        """
        self.tech_stop_service = tech_stop_service
        self.timeout = timeout

    def execute(self, symbol_obj, strategy_obj) -> dict:
        """
        Calculate trading parameters from symbol and strategy.

        Args:
            symbol_obj: Symbol model instance (has .name attribute)
            strategy_obj: Strategy model instance (has .market_bias, .get_config_value())

        Returns:
            Dictionary with calculated parameters:
            {
                "side": "BUY" | "SELL",
                "entry_price": Decimal,
                "stop_price": Decimal,
                "capital": Decimal,
                "quantity": Decimal,
                "risk_amount": Decimal,
                "position_value": Decimal,
                "timeframe": str,
                "method_used": str,
                "confidence": str,
                "side_source": str,
                "capital_source": str,
                "stop_result": TechnicalStopResult
            }

        Raises:
            TimeoutError: If Binance API calls exceed timeout
            Exception: If calculation fails
        """
        from decimal import Decimal

        # Determine side from Strategy.market_bias or config.default_side
        if hasattr(strategy_obj, 'market_bias') and strategy_obj.market_bias:
            if strategy_obj.market_bias == "BULLISH":
                side = "BUY"
            elif strategy_obj.market_bias == "BEARISH":
                side = "SELL"
            else:  # NEUTRAL
                side = strategy_obj.get_config_value("default_side", "BUY")
            side_source = "strategy.market_bias"
        else:
            side = strategy_obj.get_config_value("default_side", "BUY")
            side_source = "strategy.config.default_side"

        # Determine capital from Strategy.config
        capital_mode = strategy_obj.get_config_value("capital_mode", "fixed")
        if capital_mode == "fixed":
            capital = Decimal(strategy_obj.get_config_value("capital_fixed", "1000.00"))
            capital_source = "strategy.config.capital_fixed"
        else:
            # TODO: Implement balance mode (fetch from Binance)
            capital = Decimal("1000.00")
            capital_source = "fallback (balance mode not implemented)"

        # Get timeframe from strategy
        timeframe = strategy_obj.get_config_value("timeframe", "15m")

        # Calculate technical stop and position size
        result = self.tech_stop_service.calculate_position_with_technical_stop(
            symbol=symbol_obj.name,
            side=side,
            capital=capital,
            entry_price=None,  # Will fetch current price
            timeframe=timeframe,
            max_risk_percent=Decimal("1.0")
        )

        # Extract and enrich result
        stop_result = result["stop_result"]

        return {
            "side": side,
            "entry_price": stop_result.entry_price,
            "stop_price": stop_result.stop_price,
            "capital": capital,
            "quantity": result["quantity"],
            "risk_amount": result["risk_amount"],
            "position_value": result["position_value"],
            "timeframe": timeframe,
            "method_used": result["method_used"],
            "confidence": result["confidence"],
            "side_source": side_source,
            "capital_source": capital_source,
            "stop_result": stop_result,
        }


class _NullUoW:
    """Null Object pattern for UnitOfWork when none is provided."""

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        return False

    def commit(self) -> None:
        pass
