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


class _NullUoW:
    """Null Object pattern for UnitOfWork when none is provided."""

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        return False

    def commit(self) -> None:
        pass

