"""
Isolated Margin Close Command.

Close an open Isolated Margin position at market with AUTO_REPAY.

Usage:
    # Dry-run (default)
    python manage.py isolated_margin_close --client-id 1 --symbol BTCUSDC

    # Close by position ID (live)
    python manage.py isolated_margin_close --client-id 1 --position-id <uuid> --live --confirm
"""

import logging
from decimal import Decimal

from clients.models import Client
from django.core.management.base import BaseCommand, CommandError
from django.db import transaction

from api.application.adapters import BinanceExecution, BinanceMarketData
from api.application.execution import ExecutionMode
from api.models.margin import MarginPosition
from api.services.audit_service import AuditService

logger = logging.getLogger(__name__)


class Command(BaseCommand):
    help = "Close an Isolated Margin position at market with AUTO_REPAY"

    def add_arguments(self, parser):
        parser.add_argument(
            "--client-id",
            type=int,
            default=1,
            help="Client ID for multi-tenant (default: 1)",
        )
        parser.add_argument(
            "--position-id",
            type=str,
            help="Position ID to close (recommended)",
        )
        parser.add_argument(
            "--symbol",
            type=str,
            help="Trading pair (e.g., BTCUSDC) to select an open position",
        )
        parser.add_argument(
            "--reason",
            type=str,
            default="Manual close",
            help="Reason for closing the position",
        )
        parser.add_argument(
            "--live",
            action="store_true",
            help="Execute REAL order (default is dry-run)",
        )
        parser.add_argument(
            "--confirm",
            action="store_true",
            help="Confirm risk acknowledgement for live execution",
        )

    def handle(self, *args, **options):
        client_id = options["client_id"]
        position_id = options.get("position_id")
        symbol = options.get("symbol")
        reason = options["reason"]
        is_live = options["live"]
        is_confirmed = options["confirm"]

        if is_live and not is_confirmed:
            raise CommandError(
                "LIVE mode requires --confirm flag.\n"
                "Add --confirm to acknowledge you understand this will place REAL orders."
            )

        execution = BinanceExecution()
        market_data = BinanceMarketData(client=execution.client)

        env = "PRODUCTION" if not execution.use_testnet else "TESTNET"
        mode = ExecutionMode.LIVE if is_live else ExecutionMode.DRY_RUN

        self.stdout.write(self.style.HTTP_INFO("=" * 70))
        self.stdout.write(self.style.HTTP_INFO("ROBSON - Isolated Margin CLOSE Position"))
        self.stdout.write(self.style.HTTP_INFO("=" * 70))
        self.stdout.write("")
        self.stdout.write(f"Environment: {env}")
        self.stdout.write(f"Mode: {mode.value}")
        self.stdout.write(f"Client ID: {client_id}")
        if symbol:
            self.stdout.write(f"Symbol Filter: {symbol.upper()}")
        if position_id:
            self.stdout.write(f"Position ID: {position_id}")
        self.stdout.write("")

        try:
            client = Client.objects.get(id=client_id)
        except Client.DoesNotExist:
            raise CommandError(f"Client {client_id} not found")

        position = self._resolve_position(client_id, position_id, symbol)
        if not position.is_open:
            raise CommandError(
                f"Position {position.position_id} is not open (status: {position.status})"
            )

        close_side = "SELL" if position.side == MarginPosition.Side.LONG else "BUY"

        self.stdout.write(self.style.HTTP_INFO("--- Position ---"))
        self.stdout.write(f"Symbol: {position.symbol}")
        self.stdout.write(f"Side: {position.side}")
        self.stdout.write(f"Quantity: {position.quantity}")
        self.stdout.write(f"Entry Price: {position.entry_price}")
        self.stdout.write(f"Close Side: {close_side}")
        self.stdout.write("")

        if mode == ExecutionMode.DRY_RUN:
            self.stdout.write(self.style.WARNING("=== DRY-RUN MODE ==="))
            self.stdout.write("No real orders were placed.")
            self.stdout.write("To execute, add: --live --confirm")
            self.stdout.write("")

            est_price = self._estimate_close_price(market_data, position.symbol, close_side)
            est_pnl = self._calculate_pnl(position, est_price)
            self.stdout.write(self.style.HTTP_INFO("--- Estimated Close ---"))
            self.stdout.write(f"Estimated Price: {est_price}")
            self.stdout.write(f"Estimated P&L: {est_pnl}")
            return

        self.stdout.write(self.style.HTTP_INFO("=== EXECUTING LIVE ==="))
        self.stdout.write("")

        # Step 1: Cancel open stop/target orders
        self.stdout.write(self.style.HTTP_INFO("Step 1: Cancel Open Orders"))
        self._cancel_open_orders(execution, position)
        self.stdout.write("")

        # Step 2: Place margin close order with AUTO_REPAY
        self.stdout.write(self.style.HTTP_INFO("Step 2: Place Margin Close Order"))
        try:
            order_result = execution.client.create_margin_order(
                symbol=position.symbol,
                side=close_side,
                type="MARKET",
                quantity=str(position.quantity),
                isIsolated="TRUE",
                sideEffectType="AUTO_REPAY",
            )
        except Exception as e:
            raise CommandError(f"Close order failed: {e}")

        order_id = str(order_result.get("orderId", "N/A"))
        executed_qty = Decimal(str(order_result.get("executedQty", "0")))
        quote_qty = Decimal(str(order_result.get("cummulativeQuoteQty", "0")))
        fill_price = self._resolve_fill_price(
            position, executed_qty, quote_qty, close_side, market_data
        )

        self.stdout.write(self.style.SUCCESS(f"  Close Order ID: {order_id}"))
        self.stdout.write(self.style.SUCCESS(f"  Fill Price: ${fill_price}"))
        self.stdout.write("")

        # Step 3: Update position record
        self.stdout.write(self.style.HTTP_INFO("Step 3: Update Position"))
        with transaction.atomic():
            position.current_price = fill_price
            position.close(fill_price, reason)
            position.binance_close_order_id = order_id
            position.save()
        self.stdout.write(self.style.SUCCESS(f"  Position Closed: {position.position_id}"))
        self.stdout.write("")

        # Step 4: Record to audit trail
        self.stdout.write(self.style.HTTP_INFO("Step 4: Record to Audit Trail"))
        try:
            audit_service = AuditService(client, execution)
            if close_side == "SELL":
                audit_service.record_margin_sell(
                    symbol=position.symbol,
                    quantity=position.quantity,
                    price=fill_price,
                    binance_order_id=order_id,
                    leverage=position.leverage,
                    stop_price=position.stop_price,
                    risk_amount=position.risk_amount,
                    risk_percent=position.risk_percent,
                    position=position,
                    raw_response=order_result,
                )
            else:
                audit_service.record_margin_buy(
                    symbol=position.symbol,
                    quantity=position.quantity,
                    price=fill_price,
                    binance_order_id=order_id,
                    leverage=position.leverage,
                    stop_price=position.stop_price,
                    risk_amount=position.risk_amount,
                    risk_percent=position.risk_percent,
                    position=position,
                    raw_response=order_result,
                )
            self.stdout.write(self.style.SUCCESS("  Audit recorded"))
        except Exception as e:
            self.stdout.write(self.style.WARNING(f"  Audit failed: {e}"))

    def _resolve_position(self, client_id, position_id, symbol):
        """Resolve a single open position for the client."""
        queryset = MarginPosition.objects.filter(
            client_id=client_id,
            status=MarginPosition.Status.OPEN,
        )

        if position_id:
            queryset = queryset.filter(position_id=position_id)
        if symbol:
            queryset = queryset.filter(symbol=symbol.upper())

        count = queryset.count()
        if count == 0:
            raise CommandError("No open margin positions found for the given filters.")
        if count > 1:
            positions = "; ".join(
                f"{p.position_id} {p.symbol} {p.side} qty={p.quantity}" for p in queryset[:5]
            )
            raise CommandError(
                "Multiple open positions found. Use --position-id to select one.\n"
                f"Sample: {positions}"
            )
        return queryset.first()

    def _cancel_open_orders(self, execution, position):
        """Cancel open stop-loss or take-profit orders if present."""
        if position.binance_stop_order_id:
            try:
                execution.client.cancel_margin_order(
                    symbol=position.symbol,
                    orderId=position.binance_stop_order_id,
                    isIsolated="TRUE",
                )
                self.stdout.write(self.style.SUCCESS("  Stop order cancelled"))
            except Exception as e:
                self.stdout.write(self.style.WARNING(f"  Stop cancel failed: {e}"))

        if position.binance_target_order_id:
            try:
                execution.client.cancel_margin_order(
                    symbol=position.symbol,
                    orderId=position.binance_target_order_id,
                    isIsolated="TRUE",
                )
                self.stdout.write(self.style.SUCCESS("  Target order cancelled"))
            except Exception as e:
                self.stdout.write(self.style.WARNING(f"  Target cancel failed: {e}"))

    def _estimate_close_price(self, market_data, symbol, close_side):
        """Estimate close price using best bid/ask."""
        if close_side == "SELL":
            return market_data.best_bid(symbol)
        return market_data.best_ask(symbol)

    def _resolve_fill_price(self, position, executed_qty, quote_qty, close_side, market_data):
        """Resolve fill price from Binance response or market data fallback."""
        if executed_qty > 0 and quote_qty > 0:
            return (quote_qty / executed_qty).quantize(Decimal("0.01"))
        fallback = position.current_price or self._estimate_close_price(
            market_data, position.symbol, close_side
        )
        return Decimal(str(fallback)).quantize(Decimal("0.01"))

    def _calculate_pnl(self, position, close_price):
        """Calculate estimated P&L for display."""
        if position.side == MarginPosition.Side.LONG:
            return (close_price - position.entry_price) * position.quantity
        return (position.entry_price - close_price) * position.quantity
