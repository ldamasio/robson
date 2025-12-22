"""
Stop Loss / Take Profit Monitor and Executor.

Monitors active operations and executes stops when triggered.
Designed to run as a periodic task (Celery, cron, or management command).
"""

from __future__ import annotations
import logging
from decimal import Decimal
from dataclasses import dataclass
from typing import Optional, List
from enum import Enum

from django.utils import timezone
from django.db import transaction

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
        
        # Calculate stop and target prices
        stop_loss_price = None
        take_profit_price = None
        
        if operation.stop_loss_percent:
            if operation.side == "BUY":
                stop_loss_price = entry_price * (1 - operation.stop_loss_percent / 100)
            else:
                stop_loss_price = entry_price * (1 + operation.stop_loss_percent / 100)
        
        if operation.stop_gain_percent:
            if operation.side == "BUY":
                take_profit_price = entry_price * (1 + operation.stop_gain_percent / 100)
            else:
                take_profit_price = entry_price * (1 - operation.stop_gain_percent / 100)
        
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
        
        Returns:
            List of TriggerEvent for any triggered stops/targets
        """
        from api.models import Operation
        
        active_operations = Operation.objects.filter(status="ACTIVE")
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
    
    def execute(self, trigger: TriggerEvent) -> ExecutionResult:
        """
        Execute a stop loss or take profit order.
        
        Args:
            trigger: TriggerEvent from PriceMonitor
            
        Returns:
            ExecutionResult with order details
        """
        from api.models import Operation, Order, Trade
        
        logger.info(f"⚡ Executing {trigger.trigger_type.value} for Operation {trigger.operation_id}")
        
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
                
                # Determine order side (opposite of position)
                close_side = "SELL" if operation.side == "BUY" else "BUY"
                
                # Place market order
                order_response = self.execution.place_market(
                    symbol=trigger.symbol,
                    side=close_side,
                    quantity=trigger.quantity,
                )
                
                # Extract execution details
                order_id = str(order_response.get("orderId"))
                executed_qty = Decimal(order_response.get("executedQty", "0"))
                
                fills = order_response.get("fills", [])
                if fills:
                    total_value = sum(Decimal(f["price"]) * Decimal(f["qty"]) for f in fills)
                    total_qty = sum(Decimal(f["qty"]) for f in fills)
                    avg_price = total_value / total_qty if total_qty > 0 else trigger.current_price
                    total_fee = sum(Decimal(f.get("commission", "0")) for f in fills)
                else:
                    avg_price = trigger.current_price
                    total_fee = Decimal("0")
                
                # Calculate P&L
                if operation.side == "BUY":
                    pnl = (avg_price - trigger.entry_price) * executed_qty - total_fee
                else:
                    pnl = (trigger.entry_price - avg_price) * executed_qty - total_fee
                
                # Create exit order
                exit_order = Order.objects.create(
                    symbol=operation.symbol,
                    side=close_side,
                    order_type="MARKET",
                    quantity=executed_qty,
                    filled_quantity=executed_qty,
                    avg_fill_price=avg_price,
                    status="FILLED",
                    binance_order_id=order_id,
                )
                
                # Add to operation
                operation.exit_orders.add(exit_order)
                operation.status = "CLOSED"
                operation.save()
                
                # Update trade if exists
                trade = Trade.objects.filter(
                    symbol=operation.symbol,
                    exit_price__isnull=True,
                ).order_by("entry_time").first()
                
                if trade:
                    trade.exit_price = avg_price
                    trade.exit_fee = total_fee
                    trade.exit_time = timezone.now()
                    trade.save()
                
                # Update strategy stats
                if operation.strategy:
                    operation.strategy.update_performance(pnl, pnl > 0)
                
                logger.info(f"✅ {trigger.trigger_type.value} executed: Order {order_id}, PnL: {pnl}")
                
                return ExecutionResult(
                    success=True,
                    operation_id=trigger.operation_id,
                    trigger_type=trigger.trigger_type,
                    order_id=order_id,
                    executed_qty=executed_qty,
                    executed_price=avg_price,
                    pnl=pnl,
                )
                
        except Exception as e:
            logger.error(f"❌ Execution failed: {e}", exc_info=True)
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

