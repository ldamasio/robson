"""
Domain Layer for Monolith.

This is a copy of core domain entities for use within the Django container.
The canonical source is apps/backend/core/domain/ but this copy is needed
because the container only has the monolith code.
"""

from .technical_stop import (
    TechnicalStopCalculator,
    TechnicalStopResult,
    PriceLevel,
    OHLCV,
    StopMethod,
    Confidence,
    calculate_position_from_technical_stop,
)

__all__ = [
    "TechnicalStopCalculator",
    "TechnicalStopResult",
    "PriceLevel",
    "OHLCV",
    "StopMethod",
    "Confidence",
    "calculate_position_from_technical_stop",
]

