"""
Concrete adapter implementations for application ports.

These adapters implement the ports defined in ports.py using:
- Django ORM for persistence
- Binance API for market data and execution
- In-memory bus for events (can be replaced with message queue)
- System clock for time

Hexagonal Architecture INSIDE Django:
- Adapters ARE allowed to use Django (ORM, settings, etc.)
- They implement the port interfaces
- Use cases depend on ports, not adapters

ONE system, ONE runtime, ONE source of truth.
"""

from __future__ import annotations
from typing import Optional, Iterable
from datetime import datetime
from decimal import Decimal

from django.conf import settings
from django.utils import timezone
from binance.client import Client

from .ports import (
    OrderRepository,
    MarketDataPort,
    ExchangeExecutionPort,
    EventBusPort,
    ClockPort,
)


# ==========================================
# PERSISTENCE ADAPTERS (Django ORM)
# ==========================================


class DjangoOrderRepository(OrderRepository):
    """
    Order repository backed by Django ORM.

    Expects a dict-like order with keys:
    - id: order identifier
    - symbol: domain object with as_pair() or string
    - side: "BUY" or "SELL"
    - qty: Decimal quantity
    - price: Decimal price
    - created_at: datetime
    """

    def __init__(self):
        # Lazy import to avoid circular dependencies
        from api.models import Order as DjangoOrder, Symbol as DjangoSymbol

        self._Order = DjangoOrder
        self._Symbol = DjangoSymbol

    def _get_symbol(self, pair: str):
        """Get or create a Symbol model instance."""
        # Parse pair (assumes format like BTCUSDT)
        base = pair[:-4] if len(pair) > 4 else pair
        quote = pair[-4:] if len(pair) > 4 else "USDT"

        sym, _ = self._Symbol.objects.get_or_create(
            name=pair,
            defaults={
                "description": f"Auto-created for {pair}",
                "base_asset": base,
                "quote_asset": quote,
            },
        )
        return sym

    def save(self, order: dict) -> None:
        """Save order to Django database."""
        # Extract symbol pair
        symbol_pair = getattr(order["symbol"], "as_pair", lambda: str(order["symbol"]))()
        sym = self._get_symbol(symbol_pair)

        # Create or update Order model
        self._Order.objects.update_or_create(
            # Note: Django model doesn't have external_id field yet
            # Using symbol+side+quantity as heuristic for now
            symbol=sym,
            side=order["side"],
            quantity=Decimal(order["qty"]),
            defaults={
                "price": Decimal(order["price"]),
                "order_type": "LIMIT" if order.get("price") else "MARKET",
            },
        )

    def find_by_id(self, oid: str) -> Optional[dict]:
        """Find order by ID (not implemented - model lacks external_id field)."""
        # TODO: Add external_id field to Order model
        return None

    def list_recent(self, since: datetime) -> Iterable[dict]:
        """List orders created since the given datetime."""
        qs = self._Order.objects.filter(created_at__gte=since).order_by("-created_at")
        for m in qs:
            yield {
                "id": str(m.pk),
                "symbol": m.symbol.name,
                "side": m.side,
                "qty": str(m.quantity),
                "price": str(m.price or "0"),
                "created_at": m.created_at,
            }


# ==========================================
# EXTERNAL SERVICE ADAPTERS (Binance)
# ==========================================


class BinanceMarketData(MarketDataPort):
    """Market data adapter using Binance API."""

    def __init__(self, client: Client | None = None):
        self.client = client or Client(
            settings.BINANCE_API_KEY_TEST,
            settings.BINANCE_SECRET_KEY_TEST,
            testnet=True,
        )

    def best_bid(self, symbol: object) -> Decimal:
        """Get best bid price from Binance order book."""
        pair = getattr(symbol, "as_pair", lambda: str(symbol))()
        ob = self.client.get_order_book(symbol=pair, limit=5)
        bid = ob["bids"][0][0]
        return Decimal(str(bid))

    def best_ask(self, symbol: object) -> Decimal:
        """Get best ask price from Binance order book."""
        pair = getattr(symbol, "as_pair", lambda: str(symbol))()
        ob = self.client.get_order_book(symbol=pair, limit=5)
        ask = ob["asks"][0][0]
        return Decimal(str(ask))


class StubExecution(ExchangeExecutionPort):
    """
    Stub execution adapter that doesn't place real orders.

    Used for testing and development.
    Returns synthetic order IDs without hitting the exchange.
    """

    def place_limit(self, order: object) -> str:
        """Return synthetic order ID without external call."""
        return f"ext_{order['id']}"


class BinanceExecution(ExchangeExecutionPort):
    """
    Real Binance execution adapter.

    WARNING: This places REAL orders on Binance.
    Use with caution and proper risk management.
    """

    def __init__(self, client: Client | None = None):
        self.client = client or Client(
            settings.BINANCE_API_KEY_TEST,
            settings.BINANCE_SECRET_KEY_TEST,
            testnet=True,
        )

    def place_limit(self, order: object) -> str:
        """Place a real limit order on Binance."""
        pair = getattr(order["symbol"], "as_pair", lambda: str(order["symbol"]))()

        # Place limit order via Binance API
        response = self.client.create_order(
            symbol=pair,
            side=order["side"],
            type="LIMIT",
            timeInForce="GTC",
            quantity=str(order["qty"]),
            price=str(order["price"]),
        )

        # Return Binance order ID
        return str(response["orderId"])


# ==========================================
# EVENT BUS ADAPTERS
# ==========================================


class NoopEventBus(EventBusPort):
    """
    No-op event bus that discards events.

    Used when event publishing is not needed.
    """

    def publish(self, topic: str, payload: dict) -> None:
        """Discard the event (no-op)."""
        pass


class LoggingEventBus(EventBusPort):
    """
    Event bus that logs events to Django logger.

    Useful for debugging and development.
    """

    def __init__(self):
        import logging

        self.logger = logging.getLogger("robson.events")

    def publish(self, topic: str, payload: dict) -> None:
        """Log the event."""
        self.logger.info(f"Event published: {topic} - {payload}")


class InMemoryEventBus(EventBusPort):
    """
    In-memory event bus for testing.

    Stores events in a list for later inspection.
    """

    def __init__(self):
        self.events: list[tuple[str, dict]] = []

    def publish(self, topic: str, payload: dict) -> None:
        """Store the event in memory."""
        self.events.append((topic, payload))

    def clear(self) -> None:
        """Clear all stored events."""
        self.events.clear()


# ==========================================
# TIME ADAPTERS
# ==========================================


class RealClock(ClockPort):
    """Real clock using Django's timezone-aware datetime."""

    def now(self) -> datetime:
        """Return current timezone-aware datetime."""
        return timezone.now()


class FixedClock(ClockPort):
    """Fixed clock for testing (always returns the same time)."""

    def __init__(self, fixed_time: datetime):
        self._fixed = fixed_time

    def now(self) -> datetime:
        """Return the fixed datetime."""
        return self._fixed
