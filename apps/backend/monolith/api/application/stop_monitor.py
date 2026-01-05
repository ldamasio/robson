"""
Stop Loss / Take Profit Monitor and Executor.

Monitors active operations and executes stops when triggered.
Designed to run as a periodic task (Celery, cron, or management command).
"""

from __future__ import annotations

import logging
from dataclasses import dataclass
from decimal import Decimal
from enum import Enum
from typing import List, Optional

from django.db import transaction
from django.utils import timezone

logger = logging.getLogger(__name__)


class TriggerType(Enum):
    """Type of price trigger."""

    STOP_LOSS = "STOP_LOSS"
    TAKE_PROFIT = "TAKE_PROFIT"
    NONE = "NONE"


@dataclass
class TriggerEvent:
    """Represents a triggered stop or target."""

    operation_id: int
    trigger_type: TriggerType
    trigger_price: Decimal
    current_price: Decimal
    entry_price: Decimal
    quantity: Decimal
    symbol: str
    expected_pnl: Decimal


@dataclass
class ExecutionResult:
    """Result of stop/target execution."""

    success: bool
    operation_id: int
    trigger_type: TriggerType
    order_id: Optional[str] = None
    executed_qty: Optional[Decimal] = None
    executed_price: Optional[Decimal] = None
    pnl: Optional[Decimal] = None
    error: Optional[str] = None


class PriceMonitor:
    """
    Monitor prices for active operations and detect stop/target triggers.

    Usage:
        monitor = PriceMonitor(market_data_adapter)
        triggers = monitor.check_all_operations()
        for trigger in triggers:
            executor.execute(trigger)
    """

    def __init__(self, market_data_port=None):
        """Initialize with market data adapter."""
        self._market_data = market_data_port

    @property
    def market_data(self):
        """Lazy load market data adapter."""
        if self._market_data is None:
            from api.application.adapters import BinanceMarketData

            self._market_data = BinanceMarketData()
        return self._market_data

    def check_operation(self, operation) -> Optional[TriggerEvent]:
        """
        Check if an operation's stop or target has been triggered.

        Args:
            operation: Operation model instance

        Returns:
            TriggerEvent if triggered, None otherwise
        """
        # Get symbol pair
        symbol = operation.symbol.name

        # Get current price (bid for sells, ask for buys)
        if operation.side == "BUY":
            # To close a long, we sell at bid
            current_price = self.market_data.best_bid(symbol)
        else:
            # To close a short, we buy at ask
            current_price = self.market_data.best_ask(symbol)

        # Calculate entry price from orders
        entry_price = operation.average_entry_price
        if entry_price is None:
            logger.warning(f"Operation {operation.id} has no entry price")
            return None

        # ⭐ ADR-0012: Use absolute stop/target prices (NEVER recalculate from percentage)
        # Get stop and target prices (absolute levels, FIXED when operation was created)
        stop_loss_price = operation.stop_price  # May be None
        take_profit_price = operation.target_price  # May be None

        # Skip if no stop or target configured
        if stop_loss_price is None and take_profit_price is None:
            logger.debug(f"Operation {operation.id} has no stop_price or target_price configured")
            return None

        # Get quantity
        quantity = operation.total_entry_quantity

        # Check stop loss
        if stop_loss_price:
            if operation.side == "BUY" and current_price <= stop_loss_price:
                pnl = (current_price - entry_price) * quantity
                return TriggerEvent(
                    operation_id=operation.id,
                    trigger_type=TriggerType.STOP_LOSS,
                    trigger_price=stop_loss_price,
                    current_price=current_price,
                    entry_price=entry_price,
                    quantity=quantity,
                    symbol=symbol,
                    expected_pnl=pnl,
                )
            elif operation.side == "SELL" and current_price >= stop_loss_price:
                pnl = (entry_price - current_price) * quantity
                return TriggerEvent(
                    operation_id=operation.id,
                    trigger_type=TriggerType.STOP_LOSS,
                    trigger_price=stop_loss_price,
                    current_price=current_price,
                    entry_price=entry_price,
                    quantity=quantity,
                    symbol=symbol,
                    expected_pnl=pnl,
                )

        # Check take profit
        if take_profit_price:
            if operation.side == "BUY" and current_price >= take_profit_price:
                pnl = (current_price - entry_price) * quantity
                return TriggerEvent(
                    operation_id=operation.id,
                    trigger_type=TriggerType.TAKE_PROFIT,
                    trigger_price=take_profit_price,
                    current_price=current_price,
                    entry_price=entry_price,
                    quantity=quantity,
                    symbol=symbol,
                    expected_pnl=pnl,
                )
            elif operation.side == "SELL" and current_price <= take_profit_price:
                pnl = (entry_price - current_price) * quantity
                return TriggerEvent(
                    operation_id=operation.id,
                    trigger_type=TriggerType.TAKE_PROFIT,
                    trigger_price=take_profit_price,
                    current_price=current_price,
                    entry_price=entry_price,
                    quantity=quantity,
                    symbol=symbol,
                    expected_pnl=pnl,
                )

        return None

    def check_all_operations(self) -> List[TriggerEvent]:
        """
        Check all active operations for triggers.

        ADR-0012: Only check operations with stop_price or target_price configured.

        Returns:
            List of TriggerEvent for any triggered stops/targets
        """
        from django.db.models import Q

        from api.models import Operation

        # Only check operations with stop_price or target_price configured
        active_operations = Operation.objects.filter(status="ACTIVE").filter(
            Q(stop_price__isnull=False) | Q(target_price__isnull=False)
        )
        triggers = []

        for op in active_operations:
            try:
                trigger = self.check_operation(op)
                if trigger:
                    logger.info(f"🚨 {trigger.trigger_type.value} triggered for Operation {op.id}")
                    triggers.append(trigger)
            except Exception as e:
                logger.error(f"Error checking operation {op.id}: {e}")

        return triggers


class StopExecutor:
    """
    Execute stop loss and take profit orders.

    Usage:
        executor = StopExecutor(execution_adapter)
        result = executor.execute(trigger_event)
    """

    def __init__(self, execution_port=None):
        """Initialize with execution adapter."""
        self._execution = execution_port

    @property
    def execution(self):
        """Lazy load execution adapter."""
        if self._execution is None:
            from api.application.adapters import BinanceExecution

            self._execution = BinanceExecution()
        return self._execution

    def execute(self, trigger: TriggerEvent, source: str = "cron") -> ExecutionResult:
        """
        Execute a stop loss or take profit order with Event Sourcing and idempotency.

        ADR-0012: Event-Sourced Stop-Loss Monitor
        - Emits immutable events to stop_events (append-only log)
        - Uses execution_token for idempotency (prevents duplicate executions)
        - Updates stop_executions projection (materialized view)

        Args:
            trigger: TriggerEvent from PriceMonitor
            source: Execution source ('ws', 'cron', 'manual')

        Returns:
            ExecutionResult with order details
        """
        import uuid

        from django.db import IntegrityError

        from api.models import Operation, Order, Trade
        from api.models.event_sourcing import (
            ExecutionSource,
            ExecutionStatus,
            StopEvent,
            StopEventType,
            StopExecution,
        )

        logger.info(
            f"⚡ Executing {trigger.trigger_type.value} for Operation {trigger.operation_id}"
        )

        # Generate idempotency token: {operation_id}:{stop_price}:{timestamp_ms}
        timestamp_ms = int(timezone.now().timestamp() * 1000)
        execution_token = f"{trigger.operation_id}:{trigger.trigger_price}:{timestamp_ms}"

        try:
            with transaction.atomic():
                # Get operation
                operation = Operation.objects.select_for_update().get(id=trigger.operation_id)

                if operation.status != "ACTIVE":
                    return ExecutionResult(
                        success=False,
                        operation_id=trigger.operation_id,
                        trigger_type=trigger.trigger_type,
                        error="Operation is not active",
                    )

                account_type = "spot"
                if operation.strategy:
                    account_type = operation.strategy.get_config_value("account_type", "spot")

                # ⭐ IDEMPOTENCY CHECK: Check if execution already in progress/completed
                # Uses StopExecution projection to prevent duplicate executions
                existing_execution = StopExecution.objects.filter(
                    operation=operation,
                    status__in=[
                        ExecutionStatus.SUBMITTED,
                        ExecutionStatus.EXECUTED,
                        ExecutionStatus.FAILED,
                    ],
                ).first()

                if existing_execution:
                    logger.warning(
                        f"⚠️  Execution already exists for Operation {trigger.operation_id}: "
                        f"status={existing_execution.status}, token={existing_execution.execution_token}"
                    )
                    return ExecutionResult(
                        success=False,
                        operation_id=trigger.operation_id,
                        trigger_type=trigger.trigger_type,
                        error="Duplicate execution prevented (idempotency)",
                    )

                # Create TRIGGERED event
                trigger_event = StopEvent.objects.create(
                    operation=operation,
                    client=operation.client,
                    symbol=trigger.symbol,
                    event_type=StopEventType.STOP_TRIGGERED,
                    trigger_price=trigger.current_price,
                    stop_price=trigger.trigger_price,
                    quantity=trigger.quantity,
                    side="SELL" if operation.side == "BUY" else "BUY",  # Closing direction
                    execution_token=execution_token,
                    source=source,
                    payload_json={
                        "trigger_type": trigger.trigger_type.value,
                        "entry_price": str(trigger.entry_price),
                        "expected_pnl": str(trigger.expected_pnl),
                    },
                )

                # Create/update execution projection
                execution, created = StopExecution.objects.update_or_create(
                    operation=operation,
                    defaults={
                        "client": operation.client,
                        "execution_token": execution_token,
                        "status": ExecutionStatus.PENDING,
                        "stop_price": trigger.trigger_price,
                        "trigger_price": trigger.current_price,
                        "quantity": trigger.quantity,
                        "side": "SELL" if operation.side == "BUY" else "BUY",
                        "source": source,
                        "triggered_at": timezone.now(),
                    },
                )

                # Update operation tracking
                operation.stop_execution_token = execution_token
                operation.last_stop_check_at = timezone.now()
                operation.stop_check_count = (operation.stop_check_count or 0) + 1
                operation.save()

                # Determine order side (opposite of position)
                close_side = "SELL" if operation.side == "BUY" else "BUY"

                # ⭐ EMIT EXECUTION_SUBMITTED EVENT
                StopEvent.objects.create(
                    operation=operation,
                    client=operation.client,
                    symbol=trigger.symbol,
                    event_type=StopEventType.EXECUTION_SUBMITTED,
                    trigger_price=trigger.current_price,
                    stop_price=trigger.trigger_price,
                    quantity=trigger.quantity,
                    side=close_side,
                    execution_token=execution_token,
                    source=source,
                    payload_json={
                        "trigger_type": trigger.trigger_type.value,
                        "entry_price": str(trigger.entry_price),
                    },
                )

                # Update execution status
                execution.status = ExecutionStatus.SUBMITTED
                execution.submitted_at = timezone.now()
                execution.save()

                # Place market order
                if account_type == "isolated_margin":
                    order_response = self.execution.client.create_margin_order(
                        symbol=trigger.symbol,
                        side=close_side,
                        type="MARKET",
                        quantity=str(trigger.quantity),
                        isIsolated="TRUE",
                        sideEffectType="AUTO_REPAY",
                    )
                else:
                    order_response = self.execution.place_market(
                        symbol=trigger.symbol,
                        side=close_side,
                        quantity=trigger.quantity,
                    )

                # Extract execution details
                order_id = str(order_response.get("orderId"))
                executed_qty = Decimal(order_response.get("executedQty", "0"))
                fill_qty = executed_qty if executed_qty > 0 else trigger.quantity

                fills = order_response.get("fills", [])
                if fills:
                    total_value = sum(Decimal(f["price"]) * Decimal(f["qty"]) for f in fills)
                    total_qty = sum(Decimal(f["qty"]) for f in fills)
                    avg_price = total_value / total_qty if total_qty > 0 else trigger.current_price
                    total_fee = sum(Decimal(f.get("commission", "0")) for f in fills)
                    fee_asset = fills[0].get("commissionAsset", operation.symbol.quote_asset)
                else:
                    avg_price = trigger.current_price
                    total_fee = Decimal("0")
                    fee_asset = operation.symbol.quote_asset

                # Calculate slippage
                expected_price = trigger.trigger_price
                slippage_pct = (
                    abs((avg_price - expected_price) / expected_price * 100)
                    if expected_price
                    else Decimal("0")
                )

                # Calculate P&L
                if operation.side == "BUY":
                    pnl = (avg_price - trigger.entry_price) * fill_qty - total_fee
                else:
                    pnl = (trigger.entry_price - avg_price) * fill_qty - total_fee

                # ⭐ EMIT EXECUTED EVENT
                StopEvent.objects.create(
                    operation=operation,
                    client=operation.client,
                    symbol=trigger.symbol,
                    event_type=StopEventType.EXECUTED,
                    trigger_price=trigger.current_price,
                    stop_price=trigger.trigger_price,
                    quantity=fill_qty,
                    side=close_side,
                    execution_token=execution_token,
                    source=source,
                    exchange_order_id=order_id,
                    fill_price=avg_price,
                    slippage_pct=slippage_pct,
                    payload_json={
                        "trigger_type": trigger.trigger_type.value,
                        "entry_price": str(trigger.entry_price),
                        "pnl": str(pnl),
                        "fee": str(total_fee),
                    },
                )

                # Update execution projection
                execution.status = ExecutionStatus.EXECUTED
                execution.executed_at = timezone.now()
                execution.exchange_order_id = order_id
                execution.fill_price = avg_price
                execution.slippage_pct = slippage_pct
                execution.save()

                # Create exit order
                exit_order = Order.objects.create(
                    symbol=operation.symbol,
                    side=close_side,
                    order_type="MARKET",
                    quantity=fill_qty,
                    filled_quantity=fill_qty,
                    avg_fill_price=avg_price,
                    status="FILLED",
                    binance_order_id=order_id,
                )

                # Link exit order and update status (atomic)
                with transaction.atomic():
                    operation.exit_orders.add(exit_order)
                    operation.set_status("CLOSED")
                    operation.save()

                # Update margin position status if applicable
                margin_position = None
                if account_type == "isolated_margin":
                    from api.models.margin import MarginPosition

                    margin_position = getattr(operation, "margin_position", None)
                    if margin_position:
                        margin_position.status = MarginPosition.Status.STOPPED_OUT
                        margin_position.close_price = avg_price
                        margin_position.closed_at = timezone.now()
                        margin_position.close_reason = "Stop-loss triggered"
                        if margin_position.side == MarginPosition.Side.LONG:
                            margin_position.realized_pnl = (
                                avg_price - margin_position.entry_price
                            ) * fill_qty
                        else:
                            margin_position.realized_pnl = (
                                margin_position.entry_price - avg_price
                            ) * fill_qty
                        margin_position.unrealized_pnl = Decimal("0")
                        margin_position.current_price = avg_price
                        margin_position.save(
                            update_fields=[
                                "status",
                                "close_price",
                                "closed_at",
                                "close_reason",
                                "realized_pnl",
                                "unrealized_pnl",
                                "current_price",
                                "updated_at",
                            ]
                        )

                # Record audit trail for stop execution
                from api.services.audit_service import AuditService

                audit_service = AuditService(client=operation.client, execution=self.execution)
                if account_type == "isolated_margin":
                    if close_side == "BUY":
                        movement = audit_service.record_margin_buy(
                            symbol=trigger.symbol,
                            quantity=fill_qty,
                            price=avg_price,
                            binance_order_id=order_id,
                            position=margin_position,
                            raw_response=order_response,
                        )
                    else:
                        movement = audit_service.record_margin_sell(
                            symbol=trigger.symbol,
                            quantity=fill_qty,
                            price=avg_price,
                            binance_order_id=order_id,
                            position=margin_position,
                            raw_response=order_response,
                        )
                else:
                    if close_side == "BUY":
                        movement = audit_service.record_spot_buy(
                            symbol=trigger.symbol,
                            quantity=fill_qty,
                            price=avg_price,
                            binance_order_id=order_id,
                            fee=total_fee,
                            fee_asset=fee_asset,
                            raw_response=order_response,
                        )
                    else:
                        movement = audit_service.record_spot_sell(
                            symbol=trigger.symbol,
                            quantity=fill_qty,
                            price=avg_price,
                            binance_order_id=order_id,
                            fee=total_fee,
                            fee_asset=fee_asset,
                            raw_response=order_response,
                        )

                movement.related_operation = operation
                movement.save(update_fields=["related_operation"])

                if trigger.trigger_type == TriggerType.STOP_LOSS:
                    stop_tx = audit_service.record_stop_loss_triggered(
                        symbol=trigger.symbol,
                        quantity=fill_qty,
                        price=avg_price,
                        stop_price=trigger.trigger_price,
                        binance_order_id=order_id,
                        is_margin=(account_type == "isolated_margin"),
                        position=margin_position,
                        side=close_side,
                        raw_response={"trigger": trigger.trigger_type.value},
                    )
                    stop_tx.related_operation = operation
                    stop_tx.save(update_fields=["related_operation"])

                # Update trade if exists
                trade = (
                    Trade.objects.filter(
                        symbol=operation.symbol,
                        exit_price__isnull=True,
                    )
                    .order_by("entry_time")
                    .first()
                )

                if trade:
                    trade.exit_price = avg_price
                    trade.exit_fee = total_fee
                    trade.exit_time = timezone.now()
                    trade.save()

                # Update strategy stats
                if operation.strategy:
                    operation.strategy.update_performance(pnl, pnl > 0)

                logger.info(
                    f"✅ {trigger.trigger_type.value} executed: Order {order_id}, PnL: {pnl}, Slippage: {slippage_pct}%"
                )

                return ExecutionResult(
                    success=True,
                    operation_id=trigger.operation_id,
                    trigger_type=trigger.trigger_type,
                    order_id=order_id,
                    executed_qty=fill_qty,
                    executed_price=avg_price,
                    pnl=pnl,
                )

        except Exception as e:
            logger.error(f"❌ Execution failed: {e}", exc_info=True)

            # ⭐ EMIT FAILED EVENT (even if outside transaction)
            try:
                from api.models import Operation
                from api.models.event_sourcing import (
                    ExecutionStatus,
                    StopEvent,
                    StopEventType,
                    StopExecution,
                )

                operation = Operation.objects.get(id=trigger.operation_id)

                StopEvent.objects.create(
                    operation=operation,
                    client=operation.client,
                    symbol=trigger.symbol,
                    event_type=StopEventType.FAILED,
                    trigger_price=trigger.current_price,
                    stop_price=trigger.trigger_price,
                    quantity=trigger.quantity,
                    side="SELL" if operation.side == "BUY" else "BUY",
                    execution_token=execution_token,
                    source=source,
                    error_message=str(e),
                    payload_json={
                        "trigger_type": trigger.trigger_type.value,
                        "entry_price": str(trigger.entry_price),
                    },
                )

                # Update execution projection (if it exists)
                StopExecution.objects.filter(operation=operation).update(
                    status=ExecutionStatus.FAILED,
                    failed_at=timezone.now(),
                    error_message=str(e),
                )

            except Exception as event_error:
                logger.error(f"Failed to emit FAILED event: {event_error}")

            return ExecutionResult(
                success=False,
                operation_id=trigger.operation_id,
                trigger_type=trigger.trigger_type,
                error=str(e),
            )


def run_stop_monitor() -> List[ExecutionResult]:
    """
    Main function to run the stop monitor.

    Checks all active operations and executes any triggered stops.

    Returns:
        List of ExecutionResult for any executed orders
    """
    monitor = PriceMonitor()
    executor = StopExecutor()

    results = []
    triggers = monitor.check_all_operations()

    for trigger in triggers:
        result = executor.execute(trigger)
        results.append(result)

    return results
