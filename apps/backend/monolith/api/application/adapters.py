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

Multi-tenant aware:
- System adapters use K8s secrets (admin credentials)
- Client adapters use per-client credentials from database (future)
"""

from __future__ import annotations
from typing import Optional, Iterable
from datetime import datetime
from decimal import Decimal
import logging

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

logger = logging.getLogger(__name__)


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


def _get_binance_client(use_testnet: bool = None) -> Client:
    """
    Create a Binance client with appropriate credentials.
    
    Args:
        use_testnet: Override testnet setting. If None, uses settings.BINANCE_USE_TESTNET
        
    Returns:
        Configured Binance Client instance
    """
    if use_testnet is None:
        use_testnet = getattr(settings, 'BINANCE_USE_TESTNET', True)
    
    if use_testnet:
        api_key = settings.BINANCE_API_KEY_TEST
        secret_key = settings.BINANCE_SECRET_KEY_TEST
    else:
        api_key = settings.BINANCE_API_KEY
        secret_key = settings.BINANCE_SECRET_KEY
    
    if not api_key or not secret_key:
        mode = "testnet" if use_testnet else "production"
        raise RuntimeError(f'Binance API credentials not configured for {mode} mode')
    
    mode_str = "TESTNET" if use_testnet else "PRODUCTION"
    logger.info(f"Creating Binance client in {mode_str} mode")
    
    return Client(api_key, secret_key, testnet=use_testnet)


class BinanceMarketData(MarketDataPort):
    """
    Market data adapter using Binance API.
    
    Respects BINANCE_USE_TESTNET setting for environment selection.
    """

    def __init__(self, client: Client | None = None, use_testnet: bool = None):
        """
        Initialize market data adapter.
        
        Args:
            client: Optional pre-configured Binance client
            use_testnet: Override testnet setting. If None, uses settings.BINANCE_USE_TESTNET
        """
        self.client = client or _get_binance_client(use_testnet)

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

    def get_klines(
        self,
        symbol: object,
        interval: str = "15m",
        limit: int = 200
    ) -> list[dict]:
        """
        Get historical klines (candlestick data) from Binance.

        Args:
            symbol: Trading symbol (can be Symbol object or string)
            interval: Kline interval (1m, 3m, 5m, 15m, 30m, 1h, 4h, 1d, etc.)
            limit: Number of klines to fetch (max 1000, default 200)

        Returns:
            List of klines, each containing:
            [
                open_time,
                open,
                high,
                low,
                close,
                volume,
                close_time,
                quote_asset_volume,
                number_of_trades,
                taker_buy_base_asset_volume,
                taker_buy_quote_asset_volume,
                ignore
            ]
        """
        pair = getattr(symbol, "as_pair", lambda: str(symbol))()
        klines = self.client.get_klines(
            symbol=pair,
            interval=interval,
            limit=limit
        )
        return klines


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
    
    Respects BINANCE_USE_TESTNET setting for environment selection.
    When BINANCE_USE_TESTNET=False, trades with REAL money!
    """

    def __init__(self, client: Client | None = None, use_testnet: bool = None):
        """
        Initialize execution adapter.
        
        Args:
            client: Optional pre-configured Binance client
            use_testnet: Override testnet setting. If None, uses settings.BINANCE_USE_TESTNET
        """
        if use_testnet is None:
            use_testnet = getattr(settings, 'BINANCE_USE_TESTNET', True)
        
        self.use_testnet = use_testnet
        self.client = client or _get_binance_client(use_testnet)
        
        if not use_testnet:
            logger.warning("⚠️ BinanceExecution initialized in PRODUCTION mode - REAL MONEY!")

    def place_limit(self, order: object) -> str:
        """Place a real limit order on Binance."""
        pair = getattr(order["symbol"], "as_pair", lambda: str(order["symbol"]))()
        
        mode = "TESTNET" if self.use_testnet else "PRODUCTION"
        logger.info(f"Placing LIMIT order on {mode}: {order['side']} {order['qty']} {pair} @ {order['price']}")

        # Place limit order via Binance API
        response = self.client.create_order(
            symbol=pair,
            side=order["side"],
            type="LIMIT",
            timeInForce="GTC",
            quantity=str(order["qty"]),
            price=str(order["price"]),
        )
        
        order_id = str(response["orderId"])
        logger.info(f"Order placed successfully: {order_id}")

        # Return Binance order ID
        return order_id
    
    def place_market(self, symbol: str, side: str, quantity: Decimal) -> dict:
        """
        Place a market order on Binance.
        
        Args:
            symbol: Trading pair (e.g., "BTCUSDC")
            side: "BUY" or "SELL"
            quantity: Amount to trade
            
        Returns:
            Full order response from Binance
        """
        mode = "TESTNET" if self.use_testnet else "PRODUCTION"
        logger.info(f"Placing MARKET order on {mode}: {side} {quantity} {symbol}")
        
        response = self.client.create_order(
            symbol=symbol,
            side=side,
            type="MARKET",
            quantity=str(quantity),
        )
        
        order_id = str(response["orderId"])
        logger.info(f"Market order placed successfully: {order_id}")
        
        return response
    
    def get_account_balance(self, asset: str = None) -> dict:
        """
        Get account balance(s).
        
        Args:
            asset: Specific asset to get balance for. If None, returns all.
            
        Returns:
            Balance information
        """
        account = self.client.get_account()
        balances = account.get("balances", [])
        
        if asset:
            for balance in balances:
                if balance["asset"] == asset:
                    return {
                        "asset": asset,
                        "free": Decimal(balance["free"]),
                        "locked": Decimal(balance["locked"]),
                    }
            return {"asset": asset, "free": Decimal("0"), "locked": Decimal("0")}
        
        # Return all non-zero balances
        non_zero = []
        for balance in balances:
            free = Decimal(balance["free"])
            locked = Decimal(balance["locked"])
            if free > 0 or locked > 0:
                non_zero.append({
                    "asset": balance["asset"],
                    "free": free,
                    "locked": locked,
                })
        return {"balances": non_zero}


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
