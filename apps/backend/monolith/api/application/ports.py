"""
Application layer ports (interfaces) for the trading system.

Hexagonal Architecture INSIDE Django:
- These ports define the interfaces that adapters must implement
- They live within the Django monolith but maintain clean boundaries
- Concrete implementations are in adapters.py

ONE system, ONE runtime, ONE source of truth.
"""

from __future__ import annotations
from typing import Protocol, Optional, Iterable
from decimal import Decimal
from datetime import datetime


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
