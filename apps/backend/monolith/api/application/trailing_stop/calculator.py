"""
Core Hand-Span Trailing Stop calculation algorithm.

Pure functions - no side effects, no I/O, no Django dependencies.
Deterministic and testable.
"""

from __future__ import annotations
from decimal import Decimal
from datetime import datetime
from typing import Optional
import uuid

from .domain import (
    TrailingStopState,
    StopAdjustment,
    PositionSide,
    AdjustmentReason,
    FeeConfig,
)


class HandSpanCalculator:
    """
    Calculator for hand-span trailing stop adjustments.

    This class implements the core algorithm:
    1. Calculate how many spans the price has moved in profit
    2. Determine the appropriate stop level based on thresholds
    3. Never loosen the stop (monotonic property)

    The algorithm:
    - At 0 spans: stop remains at initial_stop
    - At 1 span: stop moves to break-even (entry_price + fees/slippage)
    - At 2+ spans: stop trails by (spans_crossed - 1) * span distance
    """

    def __init__(self, fee_config: Optional[FeeConfig] = None):
        """
        Initialize calculator with optional fee configuration.

        Args:
            fee_config: Configuration for fees and slippage (defaults to 0.1% + 0.05%)
        """
        self.fee_config = fee_config or FeeConfig()

    def calculate_adjustment(
        self,
        state: TrailingStopState,
        adjustment_token: Optional[str] = None,
    ) -> StopAdjustment:
        """
        Calculate stop adjustment based on current state.

        This is the main entry point for the algorithm.

        Args:
            state: Current trailing stop state
            adjustment_token: Optional idempotency token (generated if not provided)

        Returns:
            StopAdjustment record with old_stop, new_stop, and reason
        """
        # Generate idempotency token if not provided
        if adjustment_token is None:
            timestamp_ms = int(datetime.now().timestamp() * 1000)
            adjustment_token = f"{state.position_id}:adjust:{timestamp_ms}"

        # Calculate how many complete spans have been crossed
        spans_crossed = state.spans_in_profit

        # Determine new stop level
        if spans_crossed == 0:
            # No profit yet - keep current stop
            new_stop = state.current_stop
            reason = AdjustmentReason.NO_ADJUSTMENT
            step_index = 0
        elif spans_crossed == 1:
            # First span crossed - move to break-even
            new_stop = self._calculate_break_even(state)
            reason = AdjustmentReason.BREAK_EVEN
            step_index = 1
        else:
            # Multiple spans crossed - trail by (spans - 1)
            new_stop = self._calculate_trailing_stop(state, spans_crossed)
            reason = AdjustmentReason.TRAILING
            step_index = spans_crossed

        # CRITICAL: Never loosen the stop (monotonic property)
        new_stop = self._enforce_monotonic(state, new_stop)

        # Create adjustment record
        return StopAdjustment(
            position_id=state.position_id,
            old_stop=state.current_stop,
            new_stop=new_stop,
            reason=reason,
            adjustment_token=adjustment_token,
            timestamp=datetime.now(),
            current_price=state.current_price,
            spans_crossed=spans_crossed,
            step_index=step_index,
            metadata={
                "symbol": state.symbol,
                "side": state.side.value,
                "entry_price": str(state.entry_price),
                "span": str(state.span),
                "fee_config": {
                    "trading_fee_percent": str(self.fee_config.trading_fee_percent),
                    "slippage_buffer_percent": str(self.fee_config.slippage_buffer_percent),
                },
            },
        )

    def _calculate_break_even(self, state: TrailingStopState) -> Decimal:
        """
        Calculate break-even stop price.

        For LONG: break_even = entry_price * (1 + fees%)
        For SHORT: break_even = entry_price * (1 - fees%)

        This ensures that if stopped at break-even, the trader loses
        approximately zero after fees and slippage.
        """
        return self.fee_config.calculate_break_even(state.entry_price, state.side)

    def _calculate_trailing_stop(self, state: TrailingStopState, spans_crossed: int) -> Decimal:
        """
        Calculate trailing stop price for multiple spans crossed.

        The formula:
        - For LONG: new_stop = entry_price + ((spans_crossed - 1) * span)
        - For SHORT: new_stop = entry_price - ((spans_crossed - 1) * span)

        This means:
        - At 2 spans: stop is 1 span above/below entry (trailing by 1 span)
        - At 3 spans: stop is 2 spans above/below entry (trailing by 2 spans)
        - And so on...
        """
        if spans_crossed < 2:
            # This should not be called for < 2 spans, but handle gracefully
            return self._calculate_break_even(state)

        # Calculate trailing distance: (spans_crossed - 1) * span
        trailing_distance = (spans_crossed - 1) * state.span

        if state.side == PositionSide.LONG:
            # For LONG: move stop UP by trailing_distance from entry
            return state.entry_price + trailing_distance
        else:  # SHORT
            # For SHORT: move stop DOWN by trailing_distance from entry
            return state.entry_price - trailing_distance

    def _enforce_monotonic(self, state: TrailingStopState, proposed_stop: Decimal) -> Decimal:
        """
        Enforce monotonic property: stop never loosens.

        For LONG: new_stop >= current_stop (never move down)
        For SHORT: new_stop <= current_stop (never move up)

        Args:
            state: Current state
            proposed_stop: Proposed new stop price

        Returns:
            Validated stop price (max/min of current and proposed)
        """
        if state.side == PositionSide.LONG:
            # For LONG, stop can only move UP (increase)
            return max(state.current_stop, proposed_stop)
        else:  # SHORT
            # For SHORT, stop can only move DOWN (decrease)
            return min(state.current_stop, proposed_stop)

    def should_adjust(self, state: TrailingStopState) -> bool:
        """
        Check if stop should be adjusted given current state.

        This is a quick check to avoid unnecessary calculations.

        Returns:
            True if adjustment is needed, False otherwise
        """
        # Quick check: if not at profit, no adjustment needed
        if not state.is_at_profit:
            return False

        # Calculate what the new stop would be
        adjustment = self.calculate_adjustment(state)

        # Only adjust if stop actually changes
        return adjustment.is_adjusted

    def validate_state(self, state: TrailingStopState) -> list[str]:
        """
        Validate trailing stop state for business rule violations.

        Returns:
            List of validation errors (empty if valid)
        """
        errors = []

        # Check that current_stop hasn't loosened
        if state.side == PositionSide.LONG:
            if state.current_stop < state.initial_stop:
                errors.append(
                    f"LONG: current_stop ({state.current_stop}) is below initial_stop ({state.initial_stop})"
                )
        else:  # SHORT
            if state.current_stop > state.initial_stop:
                errors.append(
                    f"SHORT: current_stop ({state.current_stop}) is above initial_stop ({state.initial_stop})"
                )

        # Check that stop is still valid for position side
        if state.side == PositionSide.LONG:
            # For LONG, once stop crosses entry, it becomes a stop-gain
            # This is OK - no error
            pass
        else:  # SHORT
            # For SHORT, once stop crosses entry, it becomes a stop-gain
            # This is OK - no error
            pass

        return errors
