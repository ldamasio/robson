"""
Port definitions (interfaces) for Hand-Span Trailing Stop.

These are Protocol classes that define the contracts for external dependencies.
Adapters will implement these protocols.
"""

from __future__ import annotations
from typing import Protocol, Optional, List
from decimal import Decimal

from .domain import TrailingStopState, StopAdjustment


class PriceProvider(Protocol):
    """
    Interface for getting current market prices.

    Implementations might fetch from:
    - Binance API
    - WebSocket stream
    - Test fixture
    """

    def get_current_price(self, symbol: str) -> Decimal:
        """
        Get current market price for a symbol.

        Args:
            symbol: Trading pair (e.g., BTCUSDT)

        Returns:
            Current market price

        Raises:
            PriceNotAvailableError: If price cannot be fetched
        """
        ...

    def get_best_bid(self, symbol: str) -> Decimal:
        """Get best bid price (for closing LONG positions)."""
        ...

    def get_best_ask(self, symbol: str) -> Decimal:
        """Get best ask price (for closing SHORT positions)."""
        ...


class TrailingStopRepository(Protocol):
    """
    Interface for persisting trailing stop state and adjustments.

    Implementations might use:
    - Django ORM (Operation/MarginPosition models)
    - In-memory store (for testing)
    - Redis cache
    """

    def get_state(self, position_id: str) -> Optional[TrailingStopState]:
        """
        Get current trailing stop state for a position.

        Args:
            position_id: Unique position identifier

        Returns:
            TrailingStopState if position exists and has trailing stop enabled,
            None otherwise
        """
        ...

    def update_stop(self, position_id: str, new_stop: Decimal) -> None:
        """
        Update the stop price for a position.

        Args:
            position_id: Unique position identifier
            new_stop: New stop price

        Raises:
            PositionNotFoundError: If position doesn't exist
        """
        ...

    def save_adjustment(self, adjustment: StopAdjustment) -> None:
        """
        Save a stop adjustment record for audit trail.

        This should be idempotent - saving the same adjustment_token
        multiple times should not create duplicates.

        Args:
            adjustment: StopAdjustment record to save
        """
        ...

    def get_last_adjustment(self, position_id: str) -> Optional[StopAdjustment]:
        """
        Get the most recent adjustment for a position.

        Args:
            position_id: Unique position identifier

        Returns:
            Most recent StopAdjustment, or None if no adjustments exist
        """
        ...

    def has_adjustment_token(self, adjustment_token: str) -> bool:
        """
        Check if an adjustment with this token already exists.

        This is used for idempotency checking.

        Args:
            adjustment_token: Unique adjustment token

        Returns:
            True if adjustment already exists, False otherwise
        """
        ...


class EventPublisher(Protocol):
    """
    Interface for publishing stop adjustment events.

    Implementations might:
    - Emit to RabbitMQ (via Outbox pattern)
    - Log to event store
    - Trigger webhooks
    - Send notifications
    """

    def publish_adjustment(self, adjustment: StopAdjustment) -> None:
        """
        Publish a stop adjustment event.

        Args:
            adjustment: StopAdjustment record to publish
        """
        ...


class AdjustmentFilter(Protocol):
    """
    Interface for filtering which positions should have stops adjusted.

    Implementations might filter by:
    - Position status (only OPEN positions)
    - Strategy type (only certain strategies use trailing stops)
    - Tenant/client ID
    - Symbol (only certain pairs)
    """

    def should_adjust(self, position_id: str) -> bool:
        """
        Check if a position should be considered for stop adjustment.

        Args:
            position_id: Unique position identifier

        Returns:
            True if position should be adjusted, False otherwise
        """
        ...

    def get_eligible_positions(self) -> List[str]:
        """
        Get list of all position IDs eligible for stop adjustment.

        Returns:
            List of position IDs
        """
        ...


class NotificationService(Protocol):
    """
    Interface for sending notifications about stop adjustments.

    Implementations might:
    - Send email
    - Push notification
    - SMS
    - Slack/Discord webhook
    """

    def notify_stop_adjusted(
        self,
        position_id: str,
        old_stop: Decimal,
        new_stop: Decimal,
        reason: str,
    ) -> None:
        """
        Notify user that stop was adjusted.

        Args:
            position_id: Position identifier
            old_stop: Previous stop price
            new_stop: New stop price
            reason: Human-readable reason for adjustment
        """
        ...
