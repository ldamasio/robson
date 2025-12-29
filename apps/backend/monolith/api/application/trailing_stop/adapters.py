"""
Django adapters for Hand-Span Trailing Stop.

These implement the ports using Django ORM and existing infrastructure.
"""

from __future__ import annotations
from typing import Optional, List
from decimal import Decimal
from datetime import datetime
import logging

from django.db import transaction
from django.utils import timezone

from .domain import TrailingStopState, StopAdjustment, PositionSide
from .ports import (
    PriceProvider,
    TrailingStopRepository,
    EventPublisher,
    AdjustmentFilter,
    NotificationService,
)

logger = logging.getLogger(__name__)


class BinancePriceProvider:
    """
    Price provider using existing Binance adapter.

    Delegates to the existing BinanceMarketData adapter.
    """

    def __init__(self):
        """Initialize with lazy-loaded market data adapter."""
        self._market_data = None

    @property
    def market_data(self):
        """Lazy load market data adapter."""
        if self._market_data is None:
            from api.application.adapters import BinanceMarketData
            self._market_data = BinanceMarketData()
        return self._market_data

    def get_current_price(self, symbol: str) -> Decimal:
        """Get current price from Binance."""
        return self.market_data.ticker_price(symbol)

    def get_best_bid(self, symbol: str) -> Decimal:
        """Get best bid price."""
        return self.market_data.best_bid(symbol)

    def get_best_ask(self, symbol: str) -> Decimal:
        """Get best ask price."""
        return self.market_data.best_ask(symbol)


class DjangoTrailingStopRepository:
    """
    Repository using Django ORM.

    Stores trailing stop state in Operation/MarginPosition models.
    Stores adjustments in AuditTransaction (with new transaction type).
    """

    def __init__(self):
        """Initialize repository."""
        pass

    def get_state(self, position_id: str) -> Optional[TrailingStopState]:
        """
        Get trailing stop state from Operation or MarginPosition.

        Tries Operation first, then MarginPosition.
        Returns None if position not found or doesn't have required fields.
        """
        from api.models import Operation, MarginPosition

        # Try Operation model first
        try:
            operation = Operation.objects.get(id=int(position_id))
            return self._operation_to_state(operation)
        except (Operation.DoesNotExist, ValueError):
            pass

        # Try MarginPosition model
        try:
            position = MarginPosition.objects.get(position_id=position_id)
            return self._margin_position_to_state(position)
        except MarginPosition.DoesNotExist:
            pass

        return None

    def _operation_to_state(self, operation) -> Optional[TrailingStopState]:
        """Convert Operation model to TrailingStopState."""
        # Check if operation has required fields
        if not operation.stop_price:
            return None
        if operation.status != "ACTIVE":
            return None

        # Get entry price
        entry_price = operation.average_entry_price
        if entry_price is None:
            return None

        # Determine position side
        side = PositionSide.LONG if operation.side == "BUY" else PositionSide.SHORT

        # Get quantity
        quantity = operation.total_entry_quantity
        if quantity == 0:
            return None

        # For initial_stop, we use the original stop_price
        # (This assumes stop_price is never modified except by trailing stop logic)
        # TODO: Consider adding initial_stop field to Operation model for clarity
        initial_stop = operation.stop_price

        return TrailingStopState(
            position_id=str(operation.id),
            symbol=operation.symbol.name,
            side=side,
            entry_price=entry_price,
            initial_stop=initial_stop,
            current_stop=operation.stop_price,
            current_price=Decimal("0"),  # Will be updated before calculation
            quantity=quantity,
        )

    def _margin_position_to_state(self, position) -> Optional[TrailingStopState]:
        """Convert MarginPosition model to TrailingStopState."""
        # Check if position has required fields
        if not position.stop_price:
            return None
        if not position.is_open:
            return None

        # Determine position side
        side = PositionSide.LONG if position.side == position.Side.LONG else PositionSide.SHORT

        # For initial_stop, we use the original stop_price
        # TODO: Consider adding initial_stop field to MarginPosition model
        initial_stop = position.stop_price

        return TrailingStopState(
            position_id=position.position_id,
            symbol=position.symbol,
            side=side,
            entry_price=position.entry_price,
            initial_stop=initial_stop,
            current_stop=position.stop_price,
            current_price=position.current_price or Decimal("0"),
            quantity=position.quantity,
        )

    def update_stop(self, position_id: str, new_stop: Decimal) -> None:
        """
        Update stop price in database.

        Uses atomic transaction to ensure consistency.
        """
        from api.models import Operation, MarginPosition

        with transaction.atomic():
            # Try Operation first
            try:
                operation = Operation.objects.select_for_update().get(id=int(position_id))
                operation.stop_price = new_stop
                operation.save(update_fields=['stop_price', 'updated_at'])
                logger.debug(f"Updated Operation {position_id} stop to {new_stop}")
                return
            except (Operation.DoesNotExist, ValueError):
                pass

            # Try MarginPosition
            try:
                position = MarginPosition.objects.select_for_update().get(position_id=position_id)
                position.stop_price = new_stop
                position.save(update_fields=['stop_price', 'updated_at'])
                logger.debug(f"Updated MarginPosition {position_id} stop to {new_stop}")
                return
            except MarginPosition.DoesNotExist:
                pass

            raise ValueError(f"Position {position_id} not found")

    def save_adjustment(self, adjustment: StopAdjustment) -> None:
        """
        Save adjustment to audit trail.

        Uses AuditTransaction with a new transaction type.
        Implements idempotency via adjustment_token uniqueness.
        """
        from api.models.audit import AuditTransaction, TransactionType, TransactionStatus, AccountType
        from api.models import Operation, MarginPosition
        import uuid

        # Check if already exists (idempotency)
        if self.has_adjustment_token(adjustment.adjustment_token):
            logger.debug(f"Adjustment {adjustment.adjustment_token} already saved")
            return

        # Get client from position
        client = None
        symbol = adjustment.metadata.get("symbol", "UNKNOWN")

        try:
            operation = Operation.objects.get(id=int(adjustment.position_id))
            client = operation.client
            symbol = operation.symbol.name
        except (Operation.DoesNotExist, ValueError):
            try:
                position = MarginPosition.objects.get(position_id=adjustment.position_id)
                client = position.client
                symbol = position.symbol
            except MarginPosition.DoesNotExist:
                logger.error(f"Cannot save adjustment: position {adjustment.position_id} not found")
                return

        # Create audit transaction
        AuditTransaction.objects.create(
            transaction_id=str(uuid.uuid4()),
            client=client,
            transaction_type=TransactionType.STOP_LOSS_PLACED,  # Reuse existing type
            status=TransactionStatus.FILLED,
            symbol=symbol,
            asset=symbol.split("USDT")[0] if "USDT" in symbol else symbol[:3],
            quantity=Decimal("0"),  # Not applicable for adjustment
            price=adjustment.new_stop,
            total_value=Decimal("0"),
            description=self._adjustment_description(adjustment),
            source="trailing_stop",
            executed_at=adjustment.timestamp,
            # Store adjustment token in raw_response for idempotency
            raw_response={
                "adjustment_token": adjustment.adjustment_token,
                "old_stop": str(adjustment.old_stop),
                "new_stop": str(adjustment.new_stop),
                "reason": adjustment.reason.value,
                "spans_crossed": adjustment.spans_crossed,
                "step_index": adjustment.step_index,
                "metadata": adjustment.metadata,
            },
        )

        logger.debug(f"Saved adjustment {adjustment.adjustment_token}")

    def _adjustment_description(self, adjustment: StopAdjustment) -> str:
        """Generate human-readable description for audit trail."""
        side = adjustment.metadata.get("side", "UNKNOWN")
        entry_price = adjustment.metadata.get("entry_price", "0")
        span = adjustment.metadata.get("span", "0")

        return (
            f"Trailing stop adjusted ({adjustment.reason.value}): "
            f"{adjustment.old_stop} → {adjustment.new_stop} "
            f"[{side}, entry={entry_price}, span={span}, step={adjustment.step_index}]"
        )

    def has_adjustment_token(self, adjustment_token: str) -> bool:
        """Check if adjustment token exists in raw_response."""
        from api.models.audit import AuditTransaction

        return AuditTransaction.objects.filter(
            raw_response__adjustment_token=adjustment_token
        ).exists()

    def get_last_adjustment(self, position_id: str) -> Optional[StopAdjustment]:
        """
        Get most recent adjustment for a position.

        This is not implemented yet - would require parsing AuditTransaction.
        """
        # TODO: Implement if needed
        return None


class ActivePositionFilter:
    """
    Filter for positions eligible for trailing stop adjustment.

    Returns all ACTIVE operations and OPEN margin positions with stop_price set.
    """

    def __init__(self, client_id: Optional[int] = None):
        """
        Initialize filter.

        Args:
            client_id: Optional client ID to filter by (for multi-tenant)
        """
        self.client_id = client_id

    def should_adjust(self, position_id: str) -> bool:
        """Check if a position should be adjusted."""
        from api.models import Operation, MarginPosition

        # Try Operation
        try:
            operation = Operation.objects.get(id=int(position_id))
            if self.client_id and operation.client_id != self.client_id:
                return False
            return operation.status == "ACTIVE" and operation.stop_price is not None
        except (Operation.DoesNotExist, ValueError):
            pass

        # Try MarginPosition
        try:
            position = MarginPosition.objects.get(position_id=position_id)
            if self.client_id and position.client_id != self.client_id:
                return False
            return position.is_open and position.stop_price is not None
        except MarginPosition.DoesNotExist:
            pass

        return False

    def get_eligible_positions(self) -> List[str]:
        """Get all eligible position IDs."""
        from api.models import Operation, MarginPosition
        from django.db.models import Q

        position_ids = []

        # Get active operations with stop_price
        operations = Operation.objects.filter(
            status="ACTIVE",
            stop_price__isnull=False
        )
        if self.client_id:
            operations = operations.filter(client_id=self.client_id)

        for op in operations:
            position_ids.append(str(op.id))

        # Get open margin positions with stop_price
        positions = MarginPosition.objects.filter(
            status=MarginPosition.Status.OPEN,
            stop_price__isnull=False
        )
        if self.client_id:
            positions = positions.filter(client_id=self.client_id)

        for pos in positions:
            position_ids.append(pos.position_id)

        return position_ids


class StopAdjustmentEventPublisher:
    """
    Event publisher using existing event sourcing infrastructure.

    Publishes stop adjustment events to the Outbox for async processing.
    """

    def __init__(self):
        """Initialize publisher."""
        pass

    def publish_adjustment(self, adjustment: StopAdjustment) -> None:
        """
        Publish adjustment event to outbox.

        This would integrate with the existing Outbox pattern if needed.
        For now, we just log (events are already in AuditTransaction).
        """
        logger.info(
            f"Stop adjustment event: {adjustment.position_id} "
            f"{adjustment.old_stop} → {adjustment.new_stop} "
            f"({adjustment.reason.value})"
        )


class LoggingNotificationService:
    """
    Simple notification service that just logs.

    In production, this could send emails, push notifications, etc.
    """

    def __init__(self):
        """Initialize service."""
        pass

    def notify_stop_adjusted(
        self,
        position_id: str,
        old_stop: Decimal,
        new_stop: Decimal,
        reason: str,
    ) -> None:
        """Log notification."""
        logger.info(
            f"📢 Notification: Stop adjusted for position {position_id}: "
            f"{old_stop} → {new_stop} (reason: {reason})"
        )
