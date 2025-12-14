"""
Lightweight domain entities.

These are simple value objects and domain entities used by the application layer.
They are framework-agnostic and contain only business logic.

Note: For persistence, we use Django models in api/models/trading.py.
These domain entities are used for in-memory operations and type safety.
"""

from __future__ import annotations
from dataclasses import dataclass


@dataclass(frozen=True)
class Symbol:
    """
    Trading symbol value object.

    Represents a trading pair like BTC/USDT.
    Immutable to ensure consistency.
    """

    base: str
    quote: str

    def as_pair(self) -> str:
        """Return the symbol as a pair string (e.g., 'BTCUSDT')."""
        return f"{self.base}{self.quote}".upper()

    def __str__(self) -> str:
        return self.as_pair()
