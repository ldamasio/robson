"""
Dependency injection container for assembling use cases with adapters.

This is the composition root where we wire together:
- Use cases (from use_cases.py)
- Adapters (from adapters.py)
- Configuration (from Django settings)

Hexagonal Architecture INSIDE Django:
- This module knows about both use cases and adapters
- It creates concrete instances and injects dependencies
- Use singletons for stateful adapters (e.g., Binance client)

ONE system, ONE runtime, ONE source of truth.
"""

from __future__ import annotations

from .use_cases import PlaceOrderUseCase
from .adapters import (
    DjangoOrderRepository,
    BinanceMarketData,
    StubExecution,
    BinanceExecution,
    NoopEventBus,
    LoggingEventBus,
    RealClock,
)


# Singleton cache for expensive-to-create adapters
_singletons: dict[str, object] = {}


def get_singleton(key: str, factory):
    """Get or create a singleton instance."""
    if key not in _singletons:
        _singletons[key] = factory()
    return _singletons[key]


def get_place_order_uc() -> PlaceOrderUseCase:
    """
    Factory function to create PlaceOrderUseCase with all dependencies.

    This is the primary entry point for getting a configured use case.
    It assembles all the adapters and injects them into the use case.

    Returns:
        Configured PlaceOrderUseCase instance
    """

    def factory():
        repo = DjangoOrderRepository()
        md = BinanceMarketData()
        ex = StubExecution()  # Use stub for safety (change to BinanceExecution for real orders)
        bus = LoggingEventBus()  # Log events (can change to NoopEventBus or custom)
        clock = RealClock()
        return PlaceOrderUseCase(repo, md, ex, bus, clock)

    return get_singleton("place_order_uc", factory)


def clear_singletons():
    """Clear the singleton cache (useful for testing)."""
    _singletons.clear()