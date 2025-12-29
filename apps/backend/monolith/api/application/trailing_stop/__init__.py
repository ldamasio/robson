"""
Hand-Span Trailing Stop Module.

A discrete trailing-stop system derived from the initial technical stop distance at entry.

The "hand-span" is the distance between entry price and the initial technical stop.
As the price moves in favor of the position, the stop is adjusted in discrete steps:
- At +1 span: move stop to break-even
- At +2 spans: move stop by +1 span
- At +3 spans: move stop by +2 spans
- And so on...

Key features:
- Deterministic: same inputs always produce the same output
- Idempotent: multiple applications with same inputs don't create duplicates
- Auditable: every adjustment is logged with full context
- Monotonic: stop never loosens (only tightens or stays the same)
"""

from .domain import (
    TrailingStopState,
    StopAdjustment,
    PositionSide,
    AdjustmentReason,
)
from .calculator import HandSpanCalculator
from .use_cases import AdjustTrailingStopUseCase

__all__ = [
    "TrailingStopState",
    "StopAdjustment",
    "PositionSide",
    "AdjustmentReason",
    "HandSpanCalculator",
    "AdjustTrailingStopUseCase",
]
