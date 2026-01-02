"""
Use cases for the trading system.

This module contains business logic organized as use cases.
"""

from .order import PlaceOrderUseCase
from .trading_intent import (
    CreateTradingIntentCommand,
    CreateTradingIntentUseCase,
    SymbolRepository,
    StrategyRepository,
    TradingIntentRepository,
)
from .operation import (
    CancelOperationUseCase,
    CancelOperationCommand,
    CancelOperationResult,
)

__all__ = [
    # Order use cases
    "PlaceOrderUseCase",
    # Trading intent use cases
    "CreateTradingIntentCommand",
    "CreateTradingIntentUseCase",
    "SymbolRepository",
    "StrategyRepository",
    "TradingIntentRepository",
    # Operation use cases (Gate 6)
    "CancelOperationUseCase",
    "CancelOperationCommand",
    "CancelOperationResult",
]
