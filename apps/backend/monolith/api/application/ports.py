"""
Application layer ports (interfaces) for the trading system.

Hexagonal Architecture INSIDE Django:
- These ports define the interfaces that adapters must implement
- They live within the Django monolith but maintain clean boundaries
- Concrete implementations are in adapters.py

ONE system, ONE runtime, ONE source of truth.
"""

from __future__ import annotations

from datetime import datetime
from decimal import Decimal
from typing import Iterable, Optional, Protocol


class OrderRepository(Protocol):
    """Port for order persistence operations."""

    def save(self, order: object) -> None:
        """Save or update an order."""
        ...

    def find_by_id(self, oid: str) -> Optional[object]:
        """Find an order by its ID."""
        ...

    def list_recent(self, since: datetime) -> Iterable[object]:
        """List recent orders since a given datetime."""
        ...


class MarketDataPort(Protocol):
    """Port for fetching market data (prices, order book, etc.)."""

    def best_bid(self, symbol: object) -> Decimal:
        """Get the best bid price for a symbol."""
        ...

    def best_ask(self, symbol: object) -> Decimal:
        """Get the best ask price for a symbol."""
        ...


class ExchangeExecutionPort(Protocol):
    """Port for executing orders on an exchange."""

    def place_limit(self, order: object) -> str:
        """
        Place a limit order on the exchange.
        Returns the external order ID from the exchange.
        """
        ...


class EventBusPort(Protocol):
    """Port for publishing domain events."""

    def publish(self, topic: str, payload: dict) -> None:
        """Publish an event to the specified topic."""
        ...


class ClockPort(Protocol):
    """Port for time operations (enables testing with fixed time)."""

    def now(self) -> datetime:
        """Get the current datetime."""
        ...


class UnitOfWork(Protocol):
    """Port for managing database transactions."""

    def __enter__(self) -> "UnitOfWork":
        """Enter the context manager (begin transaction)."""
        ...

    def __exit__(self, exc_type, exc, tb) -> None:
        """Exit the context manager (rollback on exception)."""
        ...

    def commit(self) -> None:
        """Commit the current transaction."""
        ...


class SymbolRepository(Protocol):
    """Port for symbol data access."""

    def get_by_id(self, symbol_id: int, client_id: int) -> object:
        """Get symbol by ID for a specific client."""
        ...


class StrategyRepository(Protocol):
    """Port for strategy data access."""

    def get_by_id(self, strategy_id: int, client_id: int) -> object:
        """Get strategy by ID for a specific client."""
        ...


class TradingIntentRepository(Protocol):
    """Port for trading intent persistence."""

    def save(self, intent: dict) -> object:
        """Save a trading intent and return the persisted object."""
        ...

    def get_by_intent_id(self, intent_id: str, client_id: int) -> object:
        """Get trading intent by intent_id for a specific client."""
        ...

    def list_by_client(
        self,
        client_id: int,
        status: Optional[str] = None,
        strategy_id: Optional[int] = None,
        symbol_id: Optional[int] = None,
        offset: int = 0,
        limit: int = 100
    ) -> Iterable[object]:
        """List trading intents for a client with optional filters."""
        ...


class AccountBalancePort(Protocol):
    """
    Port for retrieving account balance information.

    Hexagonal Architecture:
    - This port defines the interface for balance retrieval
    - Concrete implementations (Binance, etc.) implement this port
    - Use cases depend on this port, not concrete implementations
    """

    def get_available_quote_balance(
        self,
        client_id: int,
        quote_asset: str,
        account_type: str = "spot",
        symbol: Optional[str] = None,
    ) -> Decimal:
        """
        Get the available (free) balance for a quote asset.

        Args:
            client_id: Client ID for multi-tenant balance retrieval
            quote_asset: Quote asset symbol (e.g., "USDT", "BUSD")
            account_type: "spot" or "isolated_margin"
            symbol: Required for isolated margin (e.g., "BTCUSDC")

        Returns:
            Available (free) balance as Decimal

        Raises:
            ConnectionError: If exchange API is unreachable
            TimeoutError: If request times out
            Exception: For other API errors
        """
        ...
