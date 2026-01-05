"""
Execution Framework wrapper for TradingIntent execution.

This provides a simple API for executing trading intents in dry-run or live mode.

Causality Chain (LIVE mode):
    User Execute LIVE
    → Exchange accepts (returns order_id)
    → Create Operation (Level 2)
    → Create AuditTransaction (Level 3)
    → Update TradingIntent.execution_result
"""

import logging
from decimal import Decimal

from django.db import transaction
from django.utils import timezone

from api.application.adapters import BinanceExecution
from api.application.execution import (
    ExecutionGuard,
    ExecutionMode,
    ExecutionResult,
    ExecutionStatus,
)
from api.models import Operation, Symbol, TradingIntent
from api.services.audit_service import AuditService

logger = logging.getLogger(__name__)


class ExecutionFramework:
    """
    Framework for executing trading intents.

    This is a thin wrapper that provides a simple execute() method
    for TradingIntent objects in dry-run or live mode.
    """

    def __init__(self, client_id: int | None = None):
        """
        Initialize execution framework.

        Args:
            client_id: Client ID for audit service (required for LIVE execution)
        """
        self.client_id = client_id

    def execute(self, intent: TradingIntent, mode: str = "dry-run") -> ExecutionResult:
        """
        Execute a trading intent.

        Args:
            intent: TradingIntent to execute
            mode: "dry-run" (default) or "live"

        Returns:
            ExecutionResult with status and actions
        """
        exec_mode = ExecutionMode.LIVE if mode == "live" else ExecutionMode.DRY_RUN

        result = ExecutionResult(
            status=ExecutionStatus.SUCCESS,
            mode=exec_mode,
        )

        if not intent.validation_result or intent.validation_result.get("status") != "PASS":
            result.status = ExecutionStatus.BLOCKED
            result.add_guard(
                ExecutionGuard(
                    name="Validation Required",
                    passed=False,
                    message="Intent must be validated before execution",
                    details={"intent_id": intent.intent_id},
                )
            )
            return result

        result.add_guard(
            ExecutionGuard(
                name="Validation Check", passed=True, message="Intent validation passed", details={}
            )
        )

        # Execute the order (simulated in dry-run)
        if exec_mode == ExecutionMode.DRY_RUN:
            result.add_action(
                {
                    "type": "SIMULATED_ORDER",
                    "side": intent.side,
                    "symbol": intent.symbol.name,
                    "quantity": str(intent.quantity),
                    "price": str(intent.entry_price),
                    "status": "SIMULATED",
                }
            )
            result.add_action(
                {
                    "type": "SIMULATED_STOP",
                    "symbol": intent.symbol.name,
                    "stop_price": str(intent.stop_price),
                    "status": "SIMULATED",
                }
            )
        else:
            # LIVE EXECUTION: Create Operation (L2) + AuditTransaction (L3)
            try:
                account_type = "spot"
                if intent.strategy:
                    account_type = intent.strategy.get_config_value("account_type", "spot")

                if account_type == "isolated_margin":
                    operation_id, movement_id, order_id = self._execute_live_isolated_margin(intent)
                else:
                    operation_id, movement_id, order_id = self._execute_live_spot(intent)

                result.add_action(
                    {
                        "type": "LIVE_ORDER",
                        "side": intent.side,
                        "symbol": intent.symbol.name,
                        "quantity": str(intent.quantity),
                        "price": str(intent.entry_price),
                        "status": "PLACED",
                        "exchange_order_id": order_id,
                        "operation_id": str(operation_id),
                        "movement_id": str(movement_id),
                    }
                )

            except Exception as e:
                logger.error(f"LIVE execution failed for {intent.intent_id}: {e}", exc_info=True)
                result.status = ExecutionStatus.BLOCKED
                result.error = str(e)
                result.add_action(
                    {
                        "type": "LIVE_ORDER",
                        "side": intent.side,
                        "symbol": intent.symbol.name,
                        "quantity": str(intent.quantity),
                        "price": str(intent.entry_price),
                        "status": "FAILED",
                        "error": str(e),
                    }
                )

        return result

    def _execute_live_spot(self, intent: TradingIntent) -> tuple[int, int | None, str]:
        """
        Execute LIVE SPOT order and create Operation + AuditTransaction.

        Causality chain (CRITICAL):
        1. Check LIVE safety gate (settings + credentials)
        2. Place order on exchange
        3. Get exchange_order_id (proof of commitment)
        4. Create Operation (Level 2)
        5. Create AuditTransaction (Level 3)
        6. Update TradingIntent.execution_result
        7. Link all entities

        Args:
            intent: TradingIntent to execute

        Returns:
            Tuple of (operation_id, movement_id, exchange_order_id)
            Note: movement_id can be None if no movement exists yet

        Raises:
            RuntimeError: If LIVE trading not enabled or credentials missing
            Exception: If exchange fails or Operation creation fails
        """
        # LIVE SAFETY GATE: Check if real trading is explicitly enabled
        from django.conf import settings

        allow_live = getattr(settings, "BINANCE_ALLOW_LIVE_TRADING", False)
        if not allow_live:
            error_msg = (
                "LIVE trading is not enabled. "
                "Set BINANCE_ALLOW_LIVE_TRADING=True in settings to execute real orders. "
                "This prevents accidental real trades."
            )
            logger.error(f"LIVE execution blocked for {intent.intent_id}: {error_msg}")
            raise RuntimeError(error_msg)

        # Verify credentials are configured
        api_key = getattr(settings, "BINANCE_API_KEY", None) or getattr(
            settings, "BINANCE_API_KEY_TEST", None
        )
        secret_key = getattr(settings, "BINANCE_SECRET_KEY", None) or getattr(
            settings, "BINANCE_SECRET_KEY_TEST", None
        )
        if not api_key or not secret_key:
            error_msg = "Binance API credentials not configured. Cannot execute LIVE orders."
            logger.error(f"LIVE execution blocked for {intent.intent_id}: {error_msg}")
            raise RuntimeError(error_msg)

        logger.info(f"LIVE safety gate passed for intent {intent.intent_id}")

        # Idempotency check: Has this intent already been executed?
        if intent.execution_result and intent.execution_result.get("operation_id"):
            existing_op_id = intent.execution_result["operation_id"]
            logger.info(
                f"Intent {intent.intent_id} already executed, operation_id={existing_op_id}"
            )

            # Retrieve existing Operation
            operation = Operation.objects.get(id=existing_op_id)
            # Retrieve associated movement
            movement = operation.movements.first()

            return (
                operation.id,
                movement.id if movement else None,
                movement.binance_order_id if movement else "unknown",
            )

        # Step 1: Place order on exchange
        logger.info(
            f"Placing LIVE order for intent {intent.intent_id}: {intent.side} {intent.quantity} {intent.symbol.name} @ {intent.entry_price}"
        )

        binance = BinanceExecution()
        response = binance.place_market(
            symbol=intent.symbol.name, side=intent.side, quantity=intent.quantity
        )

        # Step 2: Extract exchange_order_id (proof of commitment)
        exchange_order_id = str(response["orderId"])
        fills = response.get("fills", [])

        # Calculate actual fill price/quantity
        if fills:
            total_qty = Decimal("0")
            total_cost = Decimal("0")
            total_fee = Decimal("0")
            for fill in fills:
                qty = Decimal(fill["qty"])
                price = Decimal(fill["price"])
                fee = Decimal(fill["commission"])
                total_qty += qty
                total_cost += qty * price
                total_fee += fee
            avg_fill_price = total_cost / total_qty if total_qty > 0 else intent.entry_price
            fill_quantity = total_qty
            fee = total_fee
        else:
            avg_fill_price = intent.entry_price
            fill_quantity = intent.quantity
            fee = Decimal("0")

        logger.info(
            f"Exchange accepted order: order_id={exchange_order_id}, fill_price={avg_fill_price}, fill_qty={fill_quantity}"
        )

        # Step 3-6: Create Operation + AuditTransaction + Update Intent (transactional)
        with transaction.atomic():
            # Create Operation (Level 2)
            operation = Operation.objects.create(
                trading_intent=intent,
                client=intent.client,
                strategy=intent.strategy,
                symbol=intent.symbol,
                side=intent.side,
                status="ACTIVE",  # Immediately ACTIVE (no pending state in Gate 4)
                stop_price=intent.stop_price,
                target_price=intent.target_price,
            )

            logger.info(f"Created Operation {operation.id} for intent {intent.intent_id}")

            # Create AuditTransaction (Level 3) via AuditService public API
            audit_service = AuditService(client=intent.client, execution=binance)

            # Use public API methods (not private _create_transaction)
            if intent.side == "BUY":
                movement = audit_service.record_spot_buy(
                    symbol=intent.symbol.name,
                    quantity=fill_quantity,
                    price=avg_fill_price,
                    binance_order_id=exchange_order_id,
                    fee=fee,
                    fee_asset=intent.symbol.quote_asset,
                    stop_price=intent.stop_price,
                    risk_amount=intent.risk_amount,
                    risk_percent=intent.risk_percent,
                    raw_response=response,
                )
            else:
                movement = audit_service.record_spot_sell(
                    symbol=intent.symbol.name,
                    quantity=fill_quantity,
                    price=avg_fill_price,
                    binance_order_id=exchange_order_id,
                    fee=fee,
                    fee_asset=intent.symbol.quote_asset,
                    raw_response=response,
                )

            # Link movement to operation
            movement.related_operation = operation
            movement.save(update_fields=["related_operation"])

            logger.info(
                f"Created AuditTransaction {movement.id} linked to Operation {operation.id}"
            )

            # Update TradingIntent.execution_result (CRITICAL: inside same transaction)
            intent.execution_result = {
                "operation_id": operation.id,
                "exchange_order_id": exchange_order_id,
                "movement_id": movement.id,
                "avg_fill_price": str(avg_fill_price),
                "fill_quantity": str(fill_quantity),
                "fee": str(fee),
            }
            intent.save(update_fields=["execution_result", "updated_at"])

            logger.info(
                f"Updated TradingIntent {intent.intent_id} execution_result in atomic transaction"
            )

            return (operation.id, movement.id, exchange_order_id)

    def _execute_live_isolated_margin(self, intent: TradingIntent) -> tuple[int, int | None, str]:
        """
        Execute LIVE ISOLATED MARGIN order and create Operation + AuditTransaction.

        This path is used for strategies with account_type=isolated_margin.
        Stop-loss is handled internally by Robson's stop monitor.
        """
        import uuid

        from django.conf import settings

        from api.models import Operation, Order
        from api.models.margin import MarginPosition, MarginTransfer

        allow_live = getattr(settings, "BINANCE_ALLOW_LIVE_TRADING", False)
        if not allow_live:
            error_msg = (
                "LIVE trading is not enabled. "
                "Set BINANCE_ALLOW_LIVE_TRADING=True in settings to execute real orders. "
                "This prevents accidental real trades."
            )
            logger.error(f"LIVE execution blocked for {intent.intent_id}: {error_msg}")
            raise RuntimeError(error_msg)

        api_key = getattr(settings, "BINANCE_API_KEY", None) or getattr(
            settings, "BINANCE_API_KEY_TEST", None
        )
        secret_key = getattr(settings, "BINANCE_SECRET_KEY", None) or getattr(
            settings, "BINANCE_SECRET_KEY_TEST", None
        )
        if not api_key or not secret_key:
            error_msg = "Binance API credentials not configured. Cannot execute LIVE orders."
            logger.error(f"LIVE execution blocked for {intent.intent_id}: {error_msg}")
            raise RuntimeError(error_msg)

        # Idempotency check
        if intent.execution_result and intent.execution_result.get("operation_id"):
            existing_op_id = intent.execution_result["operation_id"]
            operation = Operation.objects.get(id=existing_op_id)
            movement = operation.movements.first()
            return (
                operation.id,
                movement.id if movement else None,
                movement.binance_order_id if movement else "unknown",
            )

        binance = BinanceExecution()
        audit_service = AuditService(client=intent.client, execution=binance)

        symbol = intent.symbol.name
        base_asset = intent.symbol.base_asset
        quote_asset = intent.symbol.quote_asset

        # Step 1: Check isolated margin balances
        margin_info = binance.client.get_isolated_margin_account(symbols=symbol)
        assets = margin_info.get("assets", [])
        base_info = assets[0].get("baseAsset", {}) if assets else {}
        quote_info = assets[0].get("quoteAsset", {}) if assets else {}

        quote_free = Decimal(str(quote_info.get("free", "0")))

        transfer_amount = Decimal("0")
        transfer_tran_id = None

        # Step 2: Transfer collateral from Spot if needed
        if intent.capital > 0 and quote_free < intent.capital:
            spot_quote = binance.get_account_balance(quote_asset)
            spot_free = Decimal(str(spot_quote.get("free", "0")))
            transfer_amount = min(intent.capital - quote_free, spot_free)

            if transfer_amount > 0:
                transfer_res = binance.client.transfer_spot_to_isolated_margin(
                    asset=quote_asset,
                    symbol=symbol,
                    amount=str(transfer_amount),
                )
                transfer_tran_id = transfer_res.get("tranId", None)
                if transfer_tran_id:
                    MarginTransfer.objects.create(
                        transaction_id=str(transfer_tran_id),
                        client=intent.client,
                        symbol=symbol,
                        asset=quote_asset,
                        amount=transfer_amount,
                        direction=MarginTransfer.Direction.TO_MARGIN,
                        success=True,
                    )

                audit_service.record_transfer_to_margin(
                    symbol=symbol,
                    asset=quote_asset,
                    amount=transfer_amount,
                    binance_transaction_id=str(transfer_tran_id),
                    raw_response=transfer_res,
                )

        # Step 3: Borrow base asset for short
        borrow_tran_id = None
        if intent.side == "SELL":
            borrow_res = binance.client.create_margin_loan(
                asset=base_asset,
                amount=str(intent.quantity),
                isIsolated="TRUE",
                symbol=symbol,
            )
            borrow_tran_id = borrow_res.get("tranId", None)

            audit_service.record_margin_borrow(
                symbol=symbol,
                asset=base_asset,
                amount=intent.quantity,
                binance_transaction_id=str(borrow_tran_id),
                raw_response=borrow_res,
            )

        # Step 4: Place margin entry order (market)
        order_params = {
            "symbol": symbol,
            "side": intent.side,
            "type": "MARKET",
            "quantity": str(intent.quantity),
            "isIsolated": "TRUE",
        }
        if intent.side == "BUY":
            order_params["sideEffectType"] = "MARGIN_BUY"

        response = binance.client.create_margin_order(**order_params)
        exchange_order_id = str(response.get("orderId"))

        executed_qty = Decimal(str(response.get("executedQty", "0")))
        quote_qty = Decimal(str(response.get("cummulativeQuoteQty", "0")))
        avg_fill_price = quote_qty / executed_qty if executed_qty > 0 else intent.entry_price
        fill_qty = executed_qty if executed_qty > 0 else intent.quantity

        fills = response.get("fills", [])
        if fills:
            total_fee = sum(Decimal(f.get("commission", "0")) for f in fills)
        else:
            total_fee = Decimal("0")

        # Step 5: Create Operation + Order + MarginPosition (atomic)
        with transaction.atomic():
            operation = Operation.objects.create(
                trading_intent=intent,
                client=intent.client,
                strategy=intent.strategy,
                symbol=intent.symbol,
                side=intent.side,
                status="ACTIVE",
                stop_price=intent.stop_price,
                target_price=intent.target_price,
            )

            entry_order = Order.objects.create(
                client=intent.client,
                symbol=intent.symbol,
                strategy=intent.strategy,
                side=intent.side,
                order_type="MARKET",
                quantity=fill_qty,
                price=avg_fill_price,
                status="FILLED",
                filled_quantity=fill_qty,
                avg_fill_price=avg_fill_price,
                filled_at=timezone.now(),
                stop_loss_price=intent.stop_price,
            )

            operation.entry_orders.add(entry_order)

            margin_position = MarginPosition.objects.create(
                position_id=str(uuid.uuid4()),
                client=intent.client,
                operation=operation,
                symbol=symbol,
                side=MarginPosition.Side.SHORT
                if intent.side == "SELL"
                else MarginPosition.Side.LONG,
                status=MarginPosition.Status.OPEN,
                leverage=1,
                entry_price=avg_fill_price,
                stop_price=intent.stop_price,
                target_price=intent.target_price,
                current_price=avg_fill_price,
                quantity=fill_qty,
                position_value=fill_qty * avg_fill_price,
                margin_allocated=intent.capital,
                borrowed_amount=fill_qty if intent.side == "SELL" else Decimal("0"),
                risk_amount=intent.risk_amount,
                risk_percent=intent.risk_percent,
                binance_entry_order_id=str(exchange_order_id),
                opened_at=timezone.now(),
            )

            # Record entry movement
            if intent.side == "SELL":
                movement = audit_service.record_margin_sell(
                    symbol=symbol,
                    quantity=fill_qty,
                    price=avg_fill_price,
                    binance_order_id=exchange_order_id,
                    stop_price=intent.stop_price,
                    risk_amount=intent.risk_amount,
                    risk_percent=intent.risk_percent,
                    position=margin_position,
                    raw_response=response,
                )
            else:
                movement = audit_service.record_margin_buy(
                    symbol=symbol,
                    quantity=fill_qty,
                    price=avg_fill_price,
                    binance_order_id=exchange_order_id,
                    stop_price=intent.stop_price,
                    risk_amount=intent.risk_amount,
                    risk_percent=intent.risk_percent,
                    position=margin_position,
                    raw_response=response,
                )

            movement.related_operation = operation
            movement.save(update_fields=["related_operation"])

            # Record stop-loss as internal monitor (no exchange order)
            audit_service.record_stop_loss_placed(
                symbol=symbol,
                quantity=fill_qty,
                stop_price=intent.stop_price,
                binance_order_id="internal",
                is_margin=True,
                position=margin_position,
                side="BUY" if intent.side == "SELL" else "SELL",
                raw_response={"stop_execution": "robson_market"},
            )

            # Update TradingIntent execution result
            intent.execution_result = {
                "operation_id": operation.id,
                "exchange_order_id": exchange_order_id,
                "movement_id": movement.id,
                "avg_fill_price": str(avg_fill_price),
                "fill_quantity": str(fill_qty),
                "fee": str(total_fee),
                "margin_transfer_id": str(transfer_tran_id) if transfer_tran_id else None,
                "margin_borrow_id": str(borrow_tran_id) if borrow_tran_id else None,
            }
            intent.save(update_fields=["execution_result", "updated_at"])

            logger.info(
                f"Updated TradingIntent {intent.intent_id} execution_result in atomic transaction"
            )

            return (operation.id, movement.id, exchange_order_id)
