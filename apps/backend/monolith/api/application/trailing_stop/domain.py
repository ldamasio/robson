"""
Domain entities and value objects for Hand-Span Trailing Stop.

NO DJANGO DEPENDENCIES - Pure Python business logic.
"""

from __future__ import annotations
from dataclasses import dataclass
from decimal import Decimal
from datetime import datetime
from enum import Enum
from typing import Optional


class PositionSide(Enum):
    """Position side (LONG or SHORT)."""
    LONG = "LONG"
    SHORT = "SHORT"


class AdjustmentReason(Enum):
    """Reason for stop adjustment."""
    BREAK_EVEN = "BREAK_EVEN"  # First span crossed - move to break-even
    TRAILING = "TRAILING"  # Additional span crossed - trail by one span
    NO_ADJUSTMENT = "NO_ADJUSTMENT"  # No adjustment needed


@dataclass(frozen=True)
class TrailingStopState:
    """
    Immutable state of a trailing stop configuration.

    This represents the current state of a position with trailing stop enabled.
    All values are absolute prices (never percentages).

    Attributes:
        position_id: Unique identifier for the position
        symbol: Trading pair (e.g., BTCUSDT)
        side: Position side (LONG or SHORT)
        entry_price: Price at which position was entered
        initial_stop: Initial technical stop price (fixed at entry)
        current_stop: Current stop price (can be adjusted)
        current_price: Current market price
        quantity: Position size

    Computed properties:
        span: Distance between entry and initial stop (always positive)
        spans_in_profit: How many spans the position has moved in profit
    """

    position_id: str
    symbol: str
    side: PositionSide
    entry_price: Decimal
    initial_stop: Decimal
    current_stop: Decimal
    current_price: Decimal
    quantity: Decimal

    def __post_init__(self):
        """Validate business rules."""
        # All prices must be positive
        if self.entry_price <= 0:
            raise ValueError("Entry price must be positive")
        if self.initial_stop <= 0:
            raise ValueError("Initial stop must be positive")
        if self.current_stop <= 0:
            raise ValueError("Current stop must be positive")
        if self.current_price <= 0:
            raise ValueError("Current price must be positive")
        if self.quantity <= 0:
            raise ValueError("Quantity must be positive")

        # Stop must be on correct side of entry
        if self.side == PositionSide.LONG:
            if self.initial_stop >= self.entry_price:
                raise ValueError("LONG: initial stop must be below entry price")
            if self.current_stop > self.entry_price:
                # Allow current_stop to be above entry (trailing gain)
                pass
        else:  # SHORT
            if self.initial_stop <= self.entry_price:
                raise ValueError("SHORT: initial stop must be above entry price")
            if self.current_stop < self.entry_price:
                # Allow current_stop to be below entry (trailing gain)
                pass

    @property
    def span(self) -> Decimal:
        """
        The "hand-span" distance (always positive).

        This is the distance from entry to the initial technical stop.
        It represents the initial risk taken on the position.
        """
        return abs(self.entry_price - self.initial_stop)

    @property
    def spans_in_profit(self) -> int:
        """
        How many complete spans the position has moved in profit.

        For LONG: (current_price - entry_price) / span
        For SHORT: (entry_price - current_price) / span

        Returns 0 if position is at a loss.
        """
        if self.span == 0:
            return 0

        if self.side == PositionSide.LONG:
            profit_distance = self.current_price - self.entry_price
        else:  # SHORT
            profit_distance = self.entry_price - self.current_price

        if profit_distance <= 0:
            return 0

        # Floor division to get complete spans
        spans = int(profit_distance / self.span)
        return max(0, spans)

    @property
    def is_at_profit(self) -> bool:
        """Check if position is currently profitable."""
        return self.spans_in_profit > 0

    @property
    def stop_distance_from_entry(self) -> Decimal:
        """Current stop distance from entry (can be negative for trailing gains)."""
        if self.side == PositionSide.LONG:
            return self.current_stop - self.entry_price
        else:  # SHORT
            return self.entry_price - self.current_stop


@dataclass(frozen=True)
class StopAdjustment:
    """
    Immutable record of a stop adjustment.

    This represents a decision to adjust (or not adjust) the stop price.

    Attributes:
        position_id: Unique identifier for the position
        old_stop: Previous stop price
        new_stop: New stop price (may equal old_stop if no adjustment)
        reason: Why this adjustment was made
        adjustment_token: Idempotency token (unique per adjustment)
        timestamp: When this adjustment was calculated
        current_price: Market price at time of adjustment
        spans_crossed: Number of spans crossed to trigger this adjustment
        step_index: Which step threshold was crossed (1 for break-even, 2+ for trailing)
        metadata: Additional context (fees, slippage config, etc.)
    """

    position_id: str
    old_stop: Decimal
    new_stop: Decimal
    reason: AdjustmentReason
    adjustment_token: str
    timestamp: datetime
    current_price: Decimal
    spans_crossed: int
    step_index: int
    metadata: dict

    @property
    def is_adjusted(self) -> bool:
        """Check if stop was actually adjusted."""
        return self.old_stop != self.new_stop

    @property
    def adjustment_amount(self) -> Decimal:
        """How much the stop moved (always non-negative for valid adjustments)."""
        return abs(self.new_stop - self.old_stop)

    def to_dict(self) -> dict:
        """Convert to dictionary for serialization."""
        return {
            "position_id": self.position_id,
            "old_stop": str(self.old_stop),
            "new_stop": str(self.new_stop),
            "reason": self.reason.value,
            "adjustment_token": self.adjustment_token,
            "timestamp": self.timestamp.isoformat(),
            "current_price": str(self.current_price),
            "spans_crossed": self.spans_crossed,
            "step_index": self.step_index,
            "metadata": self.metadata,
        }


@dataclass(frozen=True)
class FeeConfig:
    """
    Configuration for fees and slippage when calculating break-even.

    Attributes:
        trading_fee_percent: Trading fee as percentage (e.g., 0.1 for 0.1%)
        slippage_buffer_percent: Additional buffer for slippage (e.g., 0.05 for 0.05%)
    """
    trading_fee_percent: Decimal = Decimal("0.1")  # Default: 0.1%
    slippage_buffer_percent: Decimal = Decimal("0.05")  # Default: 0.05%

    @property
    def total_cost_percent(self) -> Decimal:
        """Total cost (fees + slippage) as percentage."""
        return self.trading_fee_percent + self.slippage_buffer_percent

    def calculate_break_even(self, entry_price: Decimal, side: PositionSide) -> Decimal:
        """
        Calculate break-even price accounting for fees and slippage.

        For LONG: break_even = entry_price * (1 + total_cost%)
        For SHORT: break_even = entry_price * (1 - total_cost%)
        """
        cost_multiplier = Decimal("1") + (self.total_cost_percent / Decimal("100"))

        if side == PositionSide.LONG:
            return entry_price * cost_multiplier
        else:  # SHORT
            return entry_price / cost_multiplier
