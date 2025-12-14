"""
Application layer package - Hexagonal Architecture INSIDE Django.

This package contains:
- ports.py: Interface definitions (protocols)
- use_cases.py: Business logic orchestration
- adapters.py: Concrete implementations
- wiring.py: Dependency injection container

Usage:
    from api.application.wiring import get_place_order_uc

    use_case = get_place_order_uc()
    result = use_case.execute(symbol, side, qty, limit_price)

ONE system, ONE runtime, ONE source of truth.
"""

from .ports import (
    OrderRepository,
    MarketDataPort,
    ExchangeExecutionPort,
    EventBusPort,
    ClockPort,
    UnitOfWork,
)

from .use_cases import PlaceOrderUseCase

from .adapters import (
    DjangoOrderRepository,
    BinanceMarketData,
    StubExecution,
    BinanceExecution,
    NoopEventBus,
    LoggingEventBus,
    InMemoryEventBus,
    RealClock,
    FixedClock,
)

from .wiring import get_place_order_uc, clear_singletons

from .domain import Symbol


__all__ = [
    # Ports
    "OrderRepository",
    "MarketDataPort",
    "ExchangeExecutionPort",
    "EventBusPort",
    "ClockPort",
    "UnitOfWork",
    # Use Cases
    "PlaceOrderUseCase",
    # Adapters
    "DjangoOrderRepository",
    "BinanceMarketData",
    "StubExecution",
    "BinanceExecution",
    "NoopEventBus",
    "LoggingEventBus",
    "InMemoryEventBus",
    "RealClock",
    "FixedClock",
    # Wiring
    "get_place_order_uc",
    "clear_singletons",
    # Domain
    "Symbol",
]
