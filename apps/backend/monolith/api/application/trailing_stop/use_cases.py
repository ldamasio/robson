"""
Use cases for Hand-Span Trailing Stop.

Business logic orchestration - coordinates domain logic with infrastructure.
"""

from __future__ import annotations
from typing import List, Optional
from decimal import Decimal
from dataclasses import dataclass
import logging

from .domain import TrailingStopState, StopAdjustment, FeeConfig
from .calculator import HandSpanCalculator
from .ports import (
    PriceProvider,
    TrailingStopRepository,
    EventPublisher,
    AdjustmentFilter,
    NotificationService,
)

logger = logging.getLogger(__name__)


@dataclass
class AdjustmentResult:
    """Result of a stop adjustment operation."""
    position_id: str
    adjusted: bool
    adjustment: Optional[StopAdjustment] = None
    error: Optional[str] = None


class AdjustTrailingStopUseCase:
    """
    Use case: Adjust trailing stop for a single position.

    This orchestrates:
    1. Getting current state (price + position data)
    2. Calculating adjustment (pure logic)
    3. Persisting changes (repository)
    4. Publishing events (event store)
    5. Sending notifications (optional)

    Idempotency is guaranteed by:
    - Checking for existing adjustment tokens before saving
    - Using unique tokens per adjustment (position_id:timestamp_ms)
    """

    def __init__(
        self,
        calculator: HandSpanCalculator,
        price_provider: PriceProvider,
        repository: TrailingStopRepository,
        event_publisher: Optional[EventPublisher] = None,
        notification_service: Optional[NotificationService] = None,
    ):
        """
        Initialize use case with dependencies.

        Args:
            calculator: Hand-span calculator
            price_provider: Price data source
            repository: Persistence layer
            event_publisher: Optional event publishing
            notification_service: Optional user notifications
        """
        self.calculator = calculator
        self.price_provider = price_provider
        self.repository = repository
        self.event_publisher = event_publisher
        self.notification_service = notification_service

    def execute(self, position_id: str) -> AdjustmentResult:
        """
        Execute trailing stop adjustment for a position.

        Args:
            position_id: Unique position identifier

        Returns:
            AdjustmentResult with details of what happened
        """
        try:
            # 1. Get current state from repository
            state = self.repository.get_state(position_id)
            if state is None:
                logger.warning(f"Position {position_id} not found or not eligible for trailing stop")
                return AdjustmentResult(
                    position_id=position_id,
                    adjusted=False,
                    error="Position not found or not eligible"
                )

            # 2. Get current market price
            try:
                current_price = self._get_closing_price(state)
            except Exception as e:
                logger.error(f"Failed to get price for {state.symbol}: {e}")
                return AdjustmentResult(
                    position_id=position_id,
                    adjusted=False,
                    error=f"Failed to get price: {e}"
                )

            # 3. Update state with current price
            state = self._update_state_with_price(state, current_price)

            # 4. Validate state
            validation_errors = self.calculator.validate_state(state)
            if validation_errors:
                logger.error(f"State validation failed for {position_id}: {validation_errors}")
                return AdjustmentResult(
                    position_id=position_id,
                    adjusted=False,
                    error=f"Validation failed: {'; '.join(validation_errors)}"
                )

            # 5. Calculate adjustment
            adjustment = self.calculator.calculate_adjustment(state)

            # 6. Check if adjustment is needed
            if not adjustment.is_adjusted:
                logger.debug(f"No adjustment needed for {position_id} (reason: {adjustment.reason.value})")
                return AdjustmentResult(
                    position_id=position_id,
                    adjusted=False,
                    adjustment=adjustment
                )

            # 7. Check idempotency: has this adjustment already been applied?
            if self.repository.has_adjustment_token(adjustment.adjustment_token):
                logger.warning(f"Adjustment {adjustment.adjustment_token} already exists (idempotency)")
                return AdjustmentResult(
                    position_id=position_id,
                    adjusted=False,
                    error="Duplicate adjustment (idempotency)",
                    adjustment=adjustment
                )

            # 8. Persist the adjustment
            self.repository.update_stop(position_id, adjustment.new_stop)
            self.repository.save_adjustment(adjustment)

            logger.info(
                f"✅ Stop adjusted for {position_id}: "
                f"{adjustment.old_stop} → {adjustment.new_stop} "
                f"(reason: {adjustment.reason.value}, step: {adjustment.step_index})"
            )

            # 9. Publish event (if publisher configured)
            if self.event_publisher:
                try:
                    self.event_publisher.publish_adjustment(adjustment)
                except Exception as e:
                    logger.error(f"Failed to publish event for {position_id}: {e}")

            # 10. Send notification (if service configured)
            if self.notification_service:
                try:
                    self.notification_service.notify_stop_adjusted(
                        position_id=position_id,
                        old_stop=adjustment.old_stop,
                        new_stop=adjustment.new_stop,
                        reason=adjustment.reason.value,
                    )
                except Exception as e:
                    logger.error(f"Failed to send notification for {position_id}: {e}")

            return AdjustmentResult(
                position_id=position_id,
                adjusted=True,
                adjustment=adjustment
            )

        except Exception as e:
            logger.error(f"Unexpected error adjusting stop for {position_id}: {e}", exc_info=True)
            return AdjustmentResult(
                position_id=position_id,
                adjusted=False,
                error=f"Unexpected error: {e}"
            )

    def _get_closing_price(self, state: TrailingStopState) -> Decimal:
        """
        Get appropriate closing price for position side.

        For LONG: use bid (we would sell at bid)
        For SHORT: use ask (we would buy at ask)
        """
        from .domain import PositionSide

        if state.side == PositionSide.LONG:
            return self.price_provider.get_best_bid(state.symbol)
        else:
            return self.price_provider.get_best_ask(state.symbol)

    def _update_state_with_price(
        self,
        state: TrailingStopState,
        current_price: Decimal
    ) -> TrailingStopState:
        """
        Create new state with updated current price.

        Since TrailingStopState is immutable, we create a new instance.
        """
        return TrailingStopState(
            position_id=state.position_id,
            symbol=state.symbol,
            side=state.side,
            entry_price=state.entry_price,
            initial_stop=state.initial_stop,
            current_stop=state.current_stop,
            current_price=current_price,
            quantity=state.quantity,
        )


class AdjustAllTrailingStopsUseCase:
    """
    Use case: Adjust trailing stops for all eligible positions.

    This is typically run by:
    - Periodic cron job (every minute)
    - WebSocket price update handler (real-time)
    - Manual trigger (management command)
    """

    def __init__(
        self,
        adjust_use_case: AdjustTrailingStopUseCase,
        adjustment_filter: AdjustmentFilter,
    ):
        """
        Initialize use case.

        Args:
            adjust_use_case: Single-position adjustment use case
            adjustment_filter: Filter for eligible positions
        """
        self.adjust_use_case = adjust_use_case
        self.adjustment_filter = adjustment_filter

    def execute(self) -> List[AdjustmentResult]:
        """
        Adjust stops for all eligible positions.

        Returns:
            List of AdjustmentResult for each position processed
        """
        # 1. Get all eligible positions
        position_ids = self.adjustment_filter.get_eligible_positions()

        if not position_ids:
            logger.debug("No eligible positions for trailing stop adjustment")
            return []

        logger.info(f"Checking trailing stops for {len(position_ids)} positions")

        # 2. Adjust each position
        results = []
        for position_id in position_ids:
            result = self.adjust_use_case.execute(position_id)
            results.append(result)

        # 3. Log summary
        adjusted_count = sum(1 for r in results if r.adjusted)
        error_count = sum(1 for r in results if r.error)

        logger.info(
            f"Trailing stop adjustment complete: "
            f"{adjusted_count} adjusted, {error_count} errors, "
            f"{len(results) - adjusted_count - error_count} no change"
        )

        return results
