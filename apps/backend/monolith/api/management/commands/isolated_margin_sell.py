"""
Isolated Margin Sell Command.

Execute a SHORT position (sell BTC) on Binance Isolated Margin using a
technical stop from the 15m chart (second resistance level).

Flow:
1. Calculate technical stop (15m, level 2) for SELL
2. Calculate position size from stop distance (max 1% risk)
3. Transfer collateral (USDT) from Spot to Isolated Margin
4. Borrow BTC
5. Place SELL entry order
6. Place BUY stop-loss order (STOP_LOSS_LIMIT)
7. Record everything in AuditService
"""

import logging
import uuid
from decimal import ROUND_DOWN, Decimal, InvalidOperation

from clients.models import Client
from django.core.management.base import BaseCommand, CommandError
from django.db import transaction
from django.utils import timezone

from api.application.adapters import BinanceExecution
from api.application.execution import ExecutionMode
from api.application.technical_stop_adapter import BinanceTechnicalStopService
from api.models.margin import MarginPosition, MarginTransfer
from api.services.audit_service import AuditService

logger = logging.getLogger(__name__)

DEFAULT_SYMBOL = "BTCUSDT"
DEFAULT_TIMEFRAME = "15m"
DEFAULT_LEVEL_N = 2
MAX_RISK_PERCENT = Decimal("1.0")


def _split_symbol(symbol: str) -> tuple[str, str]:
    """Split symbol into base and quote assets with common quotes."""
    for quote in ("USDT", "USDC", "BUSD"):
        if symbol.endswith(quote):
            return symbol[: -len(quote)], quote
    return symbol[:-3], symbol[-3:]


class Command(BaseCommand):
    help = "Execute a SHORT position on Isolated Margin using technical stop (15m)"

    def add_arguments(self, parser):
        parser.add_argument(
            "--capital",
            type=str,
            required=True,
            help="Total capital for risk calculation (in USDT)",
        )
        parser.add_argument(
            "--symbol",
            type=str,
            default=DEFAULT_SYMBOL,
            help=f"Trading pair (default: {DEFAULT_SYMBOL})",
        )
        parser.add_argument(
            "--client-id",
            type=int,
            default=1,
            help="Client ID for multi-tenant (default: 1)",
        )
        parser.add_argument(
            "--live",
            action="store_true",
            help="Execute REAL orders (default is dry-run)",
        )
        parser.add_argument(
            "--confirm",
            action="store_true",
            help="Confirm risk acknowledgement for live execution",
        )

    def handle(self, *args, **options):
        self.stdout.write("=" * 70)
        self.stdout.write("ROBSON - Isolated Margin SHORT (Technical Stop)")
        self.stdout.write("=" * 70)
        self.stdout.write("")

        try:
            capital = Decimal(options["capital"])
        except (InvalidOperation, ValueError):
            raise CommandError(f"Invalid capital: {options['capital']}")

        symbol = options["symbol"].upper()
        client_id = options["client_id"]
        is_live = options["live"]
        is_confirmed = options["confirm"]

        if is_live and not is_confirmed:
            raise CommandError(
                "LIVE mode requires --confirm flag.\n"
                "Add --confirm to acknowledge you understand this will place REAL orders."
            )

        mode = ExecutionMode.LIVE if is_live else ExecutionMode.DRY_RUN

        execution = BinanceExecution()
        tech_service = BinanceTechnicalStopService(
            level_n=DEFAULT_LEVEL_N,
            default_timeframe=DEFAULT_TIMEFRAME,
        )

        env = "PRODUCTION" if not execution.use_testnet else "TESTNET"
        self.stdout.write(f"Environment: {env}")
        self.stdout.write(f"Mode: {mode.value}")
        self.stdout.write(f"Symbol: {symbol}")
        self.stdout.write(f"Timeframe: {DEFAULT_TIMEFRAME} (level {DEFAULT_LEVEL_N})")
        self.stdout.write("")

        base_asset, quote_asset = _split_symbol(symbol)

        self.stdout.write("--- Account ---")
        try:
            spot_quote = execution.get_account_balance(quote_asset)
            spot_base = execution.get_account_balance(base_asset)
        except Exception as e:
            raise CommandError(f"Failed to get balances: {e}")

        self.stdout.write(f"Spot {quote_asset}: {spot_quote.get('free')}")
        self.stdout.write(f"Spot {base_asset}: {spot_base.get('free')}")
        self.stdout.write("")

        self.stdout.write("--- Isolated Margin Balance ---")
        margin_base_asset = base_asset
        margin_quote_asset = quote_asset
        try:
            margin_info = execution.client.get_isolated_margin_account(symbols=symbol)
            assets = margin_info.get("assets", [])
            if assets:
                base_asset_info = assets[0].get("baseAsset", {})
                quote_asset_info = assets[0].get("quoteAsset", {})
                margin_base_asset = base_asset_info.get("asset", base_asset)
                margin_quote_asset = quote_asset_info.get("asset", quote_asset)
                margin_level = assets[0].get("marginLevel", "999")
                self.stdout.write(
                    f"{margin_base_asset}: Free={base_asset_info.get('free')} "
                    f"Borrowed={base_asset_info.get('borrowed')}"
                )
                self.stdout.write(
                    f"{margin_quote_asset}: Free={quote_asset_info.get('free')} "
                    f"Borrowed={quote_asset_info.get('borrowed')}"
                )
                self.stdout.write(f"Margin Level: {margin_level}")
            else:
                self.stdout.write("Isolated margin not enabled for this pair")
        except Exception as e:
            self.stdout.write(f"Could not get margin info: {e}")
        self.stdout.write("")

        self.stdout.write("--- Technical Stop (SELL) ---")
        try:
            result = tech_service.calculate_position_with_technical_stop(
                symbol=symbol,
                side="SELL",
                capital=capital,
                timeframe=DEFAULT_TIMEFRAME,
                max_risk_percent=MAX_RISK_PERCENT,
            )
        except Exception as e:
            raise CommandError(f"Technical analysis failed: {e}")

        stop_result = result["stop_result"]
        entry_price = result["entry_price"]
        stop_price = result["stop_price"]
        stop_distance = abs(entry_price - stop_price)
        stop_distance_pct = (stop_distance / entry_price) * Decimal("100")

        self.stdout.write(f"Entry Price: ${entry_price}")
        self.stdout.write(f"Technical Stop: ${stop_price}")
        self.stdout.write(f"Stop Distance: ${stop_distance} ({stop_distance_pct:.2f}%)")
        self.stdout.write(f"Method: {result['method_used']}")
        self.stdout.write(f"Confidence: {result['confidence']}")
        self.stdout.write(f"Levels Found: {result['levels_found']}")
        if stop_result.warnings:
            for warning in stop_result.warnings:
                self.stdout.write(f"Warning: {warning}")
        self.stdout.write("")

        if not stop_result.is_valid():
            self.stdout.write("INVALID: Stop price is on wrong side of entry!")
            return

        if stop_distance <= 0:
            self.stdout.write("INVALID: Stop distance is zero")
            return

        max_risk_amount = capital * (MAX_RISK_PERCENT / Decimal("100"))
        quantity = result["quantity"]
        actual_risk = stop_distance * quantity

        if actual_risk > max_risk_amount:
            quantity = (max_risk_amount / stop_distance).quantize(
                Decimal("0.00001"),
                rounding=ROUND_DOWN,
            )
            actual_risk = stop_distance * quantity

        if quantity <= 0:
            self.stdout.write("INVALID: Quantity is zero after sizing")
            return

        position_value = (quantity * entry_price).quantize(Decimal("0.01"))
        risk_percent = (actual_risk / capital) * Decimal("100")
        risk_percent = risk_percent.quantize(Decimal("0.01"))

        self.stdout.write("--- Position Sizing (1% Risk Rule) ---")
        self.stdout.write(f"Capital: ${capital}")
        self.stdout.write(f"Max Risk (1%): ${max_risk_amount}")
        self.stdout.write(f"Quantity: {quantity} {base_asset}")
        self.stdout.write(f"Position Value: ${position_value}")
        self.stdout.write(f"Risk Amount: ${actual_risk.quantize(Decimal('0.01'))}")
        self.stdout.write(f"Risk Percent: {risk_percent.quantize(Decimal('0.01'))}%")
        self.stdout.write("")

        if risk_percent > MAX_RISK_PERCENT:
            self.stdout.write("BLOCKED: Risk exceeds 1% limit")
            return

        if mode == ExecutionMode.DRY_RUN:
            self.stdout.write("DRY-RUN: No real orders were placed.")
            self.stdout.write("To execute, add: --live --confirm")
            return

        try:
            client = Client.objects.get(id=client_id)
        except Client.DoesNotExist:
            raise CommandError(f"Client {client_id} not found")

        audit_service = AuditService(client, execution)
        position_id = str(uuid.uuid4())

        self.stdout.write("=== EXECUTING LIVE ===")
        self.stdout.write("")

        # Step 1: Transfer collateral to isolated margin
        self.stdout.write("Step 1: Transfer to Isolated Margin")
        try:
            spot_quote_free = Decimal(spot_quote.get("free", "0"))
            transfer_amount = min(capital, spot_quote_free)
            if transfer_amount < capital:
                self.stdout.write(
                    f"Insufficient {margin_quote_asset}. Have: {transfer_amount}, Need: {capital}"
                )
                self.stdout.write("Will use existing margin collateral if available...")

            if transfer_amount > 0:
                transfer_result = execution.client.transfer_spot_to_isolated_margin(
                    asset=margin_quote_asset,
                    symbol=symbol,
                    amount=str(transfer_amount),
                )
                tran_id = transfer_result.get("tranId", "N/A")
                self.stdout.write(
                    f"Transferred {transfer_amount} {margin_quote_asset} (ID: {tran_id})"
                )

                with transaction.atomic():
                    MarginTransfer.objects.create(
                        transaction_id=str(tran_id),
                        client=client,
                        symbol=symbol,
                        asset=margin_quote_asset,
                        amount=transfer_amount,
                        direction=MarginTransfer.Direction.TO_MARGIN,
                        success=True,
                    )

                audit_service.record_transfer_to_margin(
                    symbol=symbol,
                    asset=margin_quote_asset,
                    amount=transfer_amount,
                    binance_transaction_id=str(tran_id),
                    raw_response=transfer_result,
                )
        except Exception as e:
            self.stdout.write(f"Transfer failed: {e}")

        # Step 2: Borrow BTC for short
        self.stdout.write("Step 2: Borrow BTC")
        borrow_tran_id = None
        borrow_amount = quantity.quantize(Decimal("0.00001"))
        try:
            borrow_result = execution.client.create_margin_loan(
                asset=margin_base_asset,
                amount=str(borrow_amount),
                isIsolated="TRUE",
                symbol=symbol,
            )
            borrow_tran_id = borrow_result.get("tranId", "N/A")
            self.stdout.write(
                f"Borrowed {borrow_amount} {margin_base_asset} (ID: {borrow_tran_id})"
            )

            with transaction.atomic():
                MarginTransfer.objects.create(
                    transaction_id=str(borrow_tran_id),
                    client=client,
                    symbol=symbol,
                    asset=margin_base_asset,
                    amount=borrow_amount,
                    direction=MarginTransfer.Direction.TO_MARGIN,
                    success=True,
                )

            audit_service.record_margin_borrow(
                symbol=symbol,
                asset=margin_base_asset,
                amount=borrow_amount,
                binance_transaction_id=str(borrow_tran_id),
                raw_response=borrow_result,
            )
        except Exception as e:
            raise CommandError(f"Cannot proceed without borrowing {margin_base_asset}: {e}")

        # Step 3: Place entry SELL order
        self.stdout.write("Step 3: Place Margin Entry Order")
        try:
            order_result = execution.client.create_margin_order(
                symbol=symbol,
                side="SELL",
                type="MARKET",
                quantity=str(quantity),
                isIsolated="TRUE",
            )
            order_id = order_result.get("orderId", "N/A")
            executed_qty = Decimal(str(order_result.get("executedQty", "0")))
            quote_qty = Decimal(str(order_result.get("cummulativeQuoteQty", "0")))
            fill_price = quote_qty / executed_qty if executed_qty else entry_price
            self.stdout.write(f"Entry Order ID: {order_id}")
            self.stdout.write(f"Fill Price: ${fill_price:.2f}")
        except Exception as e:
            raise CommandError(f"Entry order failed: {e}")

        # Step 4: Place stop-loss BUY order
        self.stdout.write("Step 4: Place Margin Stop-Loss")
        stop_order_id = None
        stop_order_result = None
        try:
            stop_limit = (stop_price * Decimal("1.001")).quantize(Decimal("0.01"))
            stop_order_result = execution.client.create_margin_order(
                symbol=symbol,
                side="BUY",
                type="STOP_LOSS_LIMIT",
                quantity=str(quantity),
                price=str(stop_limit),
                stopPrice=str(stop_price.quantize(Decimal("0.01"))),
                timeInForce="GTC",
                isIsolated="TRUE",
                sideEffectType="AUTO_REPAY",
            )
            stop_order_id = stop_order_result.get("orderId", "N/A")
            self.stdout.write(f"Stop Order ID: {stop_order_id}")
        except Exception as e:
            self.stdout.write(f"Stop order failed: {e}")
            self.stdout.write("MANUAL STOP-LOSS REQUIRED")

        # Step 5: Record position in database
        self.stdout.write("Step 5: Record Position (Audit)")
        position = None
        try:
            with transaction.atomic():
                position = MarginPosition.objects.create(
                    position_id=position_id,
                    client=client,
                    symbol=symbol,
                    side=MarginPosition.Side.SHORT,
                    status=MarginPosition.Status.OPEN,
                    leverage=1,
                    entry_price=fill_price,
                    stop_price=stop_price,
                    current_price=fill_price,
                    quantity=quantity,
                    position_value=quantity * fill_price,
                    margin_allocated=capital,
                    borrowed_amount=borrow_amount,
                    risk_amount=actual_risk.quantize(Decimal("0.01")),
                    risk_percent=risk_percent,
                    binance_entry_order_id=str(order_id),
                    binance_stop_order_id=str(stop_order_id) if stop_order_id else None,
                    opened_at=timezone.now(),
                )
                self.stdout.write(f"Position ID: {position.id}")
                self.stdout.write(f"DB Record: {position.position_id}")
        except Exception as e:
            self.stdout.write(f"Failed to save position: {e}")
            logger.error("Failed to save margin position", exc_info=True)

        # Step 6: Record to audit trail
        self.stdout.write("Step 6: Record to Audit Trail")
        try:
            audit_service.record_margin_sell(
                symbol=symbol,
                quantity=quantity,
                price=fill_price,
                binance_order_id=str(order_id),
                leverage=1,
                stop_price=stop_price,
                risk_amount=actual_risk,
                risk_percent=risk_percent,
                position=position,
                raw_response=order_result,
            )
            self.stdout.write("Recorded: Margin Sell")

            if stop_order_id:
                audit_service.record_stop_loss_placed(
                    symbol=symbol,
                    quantity=quantity,
                    stop_price=stop_price,
                    binance_order_id=str(stop_order_id),
                    is_margin=True,
                    position=position,
                    side="BUY",
                    raw_response=stop_order_result,
                )
                self.stdout.write("Recorded: Stop-Loss Order")
        except Exception as e:
            self.stdout.write(f"Audit recording failed: {e}")
            logger.warning("Failed to record audit trail", exc_info=True)

        self.stdout.write("")
        self.stdout.write("=" * 70)
        self.stdout.write("POSITION OPENED SUCCESSFULLY")
        self.stdout.write("")
        self.stdout.write(f"Position: SHORT {quantity} {base_asset} @ ${fill_price:.2f}")
        self.stdout.write(f"Stop-Loss: ${stop_price:.2f}")
        self.stdout.write(
            f"Risk: ${actual_risk.quantize(Decimal('0.01'))} ({risk_percent.quantize(Decimal('0.01'))}%)"
        )
        self.stdout.write("")
        self.stdout.write("=" * 70)
