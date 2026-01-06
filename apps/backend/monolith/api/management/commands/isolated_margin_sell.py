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
6. Record stop-loss for internal monitoring (no exchange order)
7. Stop monitor executes market close when triggered
8. Record everything in AuditService
"""

import logging
import uuid
from decimal import ROUND_DOWN, Decimal, InvalidOperation

from clients.models import Client
from django.core.management.base import BaseCommand, CommandError
from django.utils import timezone

from api.application.adapters import BinanceExecution
from api.application.execution import ExecutionMode, ExecutionStatus
from api.application.execution_framework import ExecutionFramework
from api.application.technical_stop_adapter import BinanceTechnicalStopService
from api.models import Strategy, Symbol, TradingIntent

logger = logging.getLogger(__name__)

DEFAULT_SYMBOL = "BTCUSDT"
DEFAULT_TIMEFRAME = "15m"
DEFAULT_LEVEL_N = 2
DEFAULT_STRATEGY_NAME = "Iron Exit Protocol"
MAX_RISK_PERCENT = Decimal("1.0")
CONFIDENCE_MAP = {
    "HIGH": Decimal("0.8"),
    "MEDIUM": Decimal("0.6"),
    "MED": Decimal("0.6"),
    "LOW": Decimal("0.4"),
}


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
            "--strategy",
            type=str,
            default=DEFAULT_STRATEGY_NAME,
            help=f"Strategy name (default: {DEFAULT_STRATEGY_NAME})",
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
        strategy_name = options["strategy"]
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
        self.stdout.write(f"Strategy: {strategy_name}")
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

        self.stdout.write("--- Stop Execution ---")
        self.stdout.write("Stop Execution: INTERNAL (robson_market)")
        self.stdout.write("Exchange stop order: NOT PLACED")
        self.stdout.write("Stop monitor will execute a market BUY when stop is hit.")
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
        symbol_obj, _ = Symbol.objects.get_or_create(
            client=client,
            name=symbol,
            defaults={
                "base_asset": base_asset,
                "quote_asset": quote_asset,
                "is_active": True,
            },
        )

        strategy = Strategy.objects.filter(name=strategy_name, client=client).first()
        if strategy is None:
            strategy = Strategy.objects.filter(name=strategy_name, client__isnull=True).first()
        if strategy is None:
            strategy = Strategy.objects.create(
                client=client,
                name=strategy_name,
                description="Iron Exit Protocol (CLI-created)",
                config={
                    "account_type": "isolated_margin",
                    "capital_mode": "fixed",
                    "capital_fixed": str(capital),
                    "technical_stop": {
                        "timeframe": DEFAULT_TIMEFRAME,
                        "level": DEFAULT_LEVEL_N,
                        "side": "SELL",
                    },
                    "stop_execution": "robson_market",
                    "risk_percent": float(MAX_RISK_PERCENT),
                },
                risk_config={
                    "max_risk_per_trade": float(MAX_RISK_PERCENT),
                    "use_technical_stop": True,
                    "stop_execution": "robson_market",
                },
                market_bias="BEARISH",
                is_active=True,
            )

        confidence_key = str(result.get("confidence", "LOW")).upper()
        confidence_float = CONFIDENCE_MAP.get(confidence_key, Decimal("0.4"))

        intent = TradingIntent.objects.create(
            intent_id=str(uuid.uuid4()),
            client=client,
            symbol=symbol_obj,
            strategy=strategy,
            side="SELL",
            status="VALIDATED",
            quantity=quantity,
            entry_price=entry_price,
            stop_price=stop_price,
            target_price=None,
            regime="unknown",
            confidence=float(confidence_float),
            reason="Iron Exit Protocol (CLI)",
            capital=capital,
            risk_amount=actual_risk.quantize(Decimal("0.01")),
            risk_percent=risk_percent,
            validation_result={"status": "PASS", "source": "cli"},
            validated_at=timezone.now(),
        )

        self.stdout.write("=== EXECUTING LIVE ===")
        self.stdout.write("")

        framework = ExecutionFramework(client_id=client.id)
        exec_result = framework.execute(intent, mode="live")
        if exec_result.is_success():
            intent.status = "EXECUTED"
            intent.executed_at = timezone.now()
            intent.save(update_fields=["status", "executed_at", "updated_at"])
        elif exec_result.is_blocked():
            intent.status = "FAILED"
            intent.error_message = exec_result.error or "Execution blocked by safety guard"
            intent.save(update_fields=["status", "error_message", "updated_at"])
        elif exec_result.status == ExecutionStatus.FAILED:
            intent.status = "FAILED"
            intent.error_message = exec_result.error or "Execution failed"
            intent.save(update_fields=["status", "error_message", "updated_at"])
        self.stdout.write(exec_result.to_human_readable())
