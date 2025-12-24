"""
Django management command to open a margin position.

This command provides a CLI interface for opening Isolated Margin positions
with full risk management (1% per trade, stop-loss mandatory).

Usage:
    # Preview position sizing (dry run)
    python manage.py open_margin_position --client-id 1 --symbol BTCUSDC \
        --side LONG --entry 100000 --stop 98000 --capital 1000 --preview

    # Open real position
    python manage.py open_margin_position --client-id 1 --symbol BTCUSDC \
        --side LONG --entry 100000 --stop 98000 --target 105000 --capital 1000 \
        --leverage 3 --live --acknowledge-risk
"""

import json
import sys
from decimal import Decimal
from django.core.management.base import BaseCommand

from api.application.margin_adapters import BinanceMarginAdapter, MockMarginAdapter
from api.application.adapters import LoggingEventBus, RealClock


def calculate_margin_position_size(
    capital,
    entry_price,
    stop_price,
    max_risk_percent=1.0,
):
    """
    Calculate position size for margin trading using the Golden Rule.
    
    Position Size = (Risk Amount) / (Stop Distance)
    
    Where:
    - Risk Amount = Capital × (max_risk_percent / 100)
    - Stop Distance = |Entry Price - Stop Price|
    """
    capital = Decimal(str(capital))
    entry_price = Decimal(str(entry_price))
    stop_price = Decimal(str(stop_price))
    max_risk_percent = Decimal(str(max_risk_percent))
    
    risk_amount = capital * (max_risk_percent / Decimal('100'))
    stop_distance = abs(entry_price - stop_price)
    
    if stop_distance == 0:
        return Decimal('0')
    
    quantity = risk_amount / stop_distance
    return quantity


class SimpleAuditTrail:
    """Simple audit trail that logs to console."""
    
    def record(self, event_type: str, aggregate_id: str, data: dict, reason: str):
        print(f"[AUDIT] {event_type}: {reason}")
        print(f"        ID: {aggregate_id}")
        for k, v in data.items():
            print(f"        {k}: {v}")


class Command(BaseCommand):
    help = "Open an Isolated Margin position with risk management"

    def add_arguments(self, parser):
        # Required arguments
        parser.add_argument(
            "--client-id",
            type=int,
            required=True,
            help="Client ID (tenant)",
        )
        parser.add_argument(
            "--symbol",
            type=str,
            required=True,
            help="Trading pair (e.g., BTCUSDC)",
        )
        parser.add_argument(
            "--side",
            type=str,
            required=True,
            choices=["LONG", "SHORT"],
            help="Position side",
        )
        parser.add_argument(
            "--entry",
            type=str,
            required=True,
            help="Entry price",
        )
        parser.add_argument(
            "--stop",
            type=str,
            required=True,
            help="Stop-loss price (MANDATORY)",
        )
        parser.add_argument(
            "--capital",
            type=str,
            required=True,
            help="Total capital for position sizing",
        )
        
        # Optional arguments
        parser.add_argument(
            "--target",
            type=str,
            help="Take-profit price (optional)",
        )
        parser.add_argument(
            "--leverage",
            type=int,
            default=3,
            help="Leverage (1-10, default 3)",
        )
        parser.add_argument(
            "--risk-percent",
            type=str,
            default="1.0",
            help="Max risk per trade in percent (default 1.0)",
        )
        
        # Execution mode
        parser.add_argument(
            "--preview",
            action="store_true",
            help="Preview position sizing only (no execution)",
        )
        parser.add_argument(
            "--live",
            action="store_true",
            help="Execute on live exchange (requires --acknowledge-risk)",
        )
        parser.add_argument(
            "--acknowledge-risk",
            action="store_true",
            help="Acknowledge risk for live execution",
        )
        
        # Output format
        parser.add_argument(
            "--json",
            action="store_true",
            help="Output in JSON format",
        )

    def handle(self, *args, **options):
        """Execute the command."""
        # Parse arguments
        client_id = options["client_id"]
        symbol = options["symbol"].upper()
        side = options["side"].upper()
        entry_price = Decimal(options["entry"])
        stop_price = Decimal(options["stop"])
        total_capital = Decimal(options["capital"])
        target_price = Decimal(options["target"]) if options["target"] else None
        leverage = options["leverage"]
        max_risk_percent = Decimal(options["risk_percent"])
        
        preview_only = options["preview"]
        live_mode = options["live"]
        acknowledged = options["acknowledge_risk"]
        output_json = options["json"]
        
        # Validate inputs
        try:
            self._validate_inputs(side, entry_price, stop_price, target_price, leverage)
        except ValueError as e:
            if output_json:
                self.stdout.write(json.dumps({"error": str(e)}))
            else:
                self.stderr.write(self.style.ERROR(f"Validation error: {e}"))
            sys.exit(1)
        
        # Calculate position size
        try:
            sizing = calculate_margin_position_size(
                capital=total_capital,
                entry_price=entry_price,
                stop_price=stop_price,
                side=side,
                leverage=leverage,
                max_risk_percent=max_risk_percent,
            )
        except ValueError as e:
            if output_json:
                self.stdout.write(json.dumps({"error": str(e)}))
            else:
                self.stderr.write(self.style.ERROR(f"Position sizing error: {e}"))
            sys.exit(1)
        
        # Build result
        result = {
            "mode": "PREVIEW" if preview_only else ("LIVE" if live_mode else "DRY_RUN"),
            "client_id": client_id,
            "symbol": symbol,
            "side": side,
            "entry_price": str(entry_price),
            "stop_price": str(stop_price),
            "target_price": str(target_price) if target_price else None,
            "leverage": leverage,
            "position_sizing": {
                "quantity": str(sizing.quantity),
                "position_value": str(sizing.position_value),
                "margin_required": str(sizing.margin_required),
                "risk_amount": str(sizing.risk_amount),
                "risk_percent": str(sizing.risk_percent),
                "stop_distance": str(sizing.stop_distance),
                "stop_distance_percent": str(sizing.stop_distance_percent),
                "is_capped": sizing.is_capped,
                "cap_reason": sizing.cap_reason,
            },
        }
        
        # Preview mode - just show calculation
        if preview_only:
            if output_json:
                self.stdout.write(json.dumps(result, indent=2))
            else:
                self._print_sizing_report(result)
            sys.exit(0)
        
        # Check for live mode requirements
        if live_mode and not acknowledged:
            if output_json:
                self.stdout.write(json.dumps({
                    "error": "Live mode requires --acknowledge-risk flag",
                    "position_sizing": result["position_sizing"],
                }))
            else:
                self.stderr.write(self.style.ERROR(
                    "Live mode requires --acknowledge-risk flag\n"
                    "This will execute REAL trades with REAL money!"
                ))
            sys.exit(1)
        
        # Execute position opening
        try:
            # Select adapter
            if live_mode:
                adapter = BinanceMarginAdapter(use_testnet=False)
            else:
                adapter = MockMarginAdapter()
            
            clock = RealClock()
            audit = SimpleAuditTrail()
            
            # For now, just simulate the order placement
            # Full use case integration would require repository setup
            
            if not output_json:
                self.stdout.write("\n" + "=" * 60)
                self.stdout.write(self.style.SUCCESS(
                    f"\n{'🔴 LIVE' if live_mode else '🟡 DRY-RUN'} MODE\n"
                ))
            
            # Get margin account
            if live_mode:
                try:
                    account = adapter.get_margin_account(symbol)
                    result["margin_account"] = {
                        "quote_free": str(account.quote_free),
                        "margin_level": str(account.margin_level),
                    }
                except Exception as e:
                    result["margin_account_error"] = str(e)
            
            # Place entry order
            order_side = "BUY" if side == "LONG" else "SELL"
            
            entry_result = adapter.place_margin_order(
                symbol=symbol,
                side=order_side,
                order_type="MARKET",
                quantity=sizing.quantity,
                side_effect_type="MARGIN_BUY" if side == "LONG" else None,
            )
            
            result["entry_order"] = {
                "success": entry_result.success,
                "order_id": entry_result.binance_order_id,
                "filled_quantity": str(entry_result.filled_quantity),
                "avg_fill_price": str(entry_result.avg_fill_price) if entry_result.avg_fill_price else None,
                "status": entry_result.status,
                "error": entry_result.error_message,
            }
            
            if not entry_result.success:
                if output_json:
                    self.stdout.write(json.dumps(result, indent=2))
                else:
                    self.stderr.write(self.style.ERROR(
                        f"Entry order failed: {entry_result.error_message}"
                    ))
                sys.exit(1)
            
            # Place stop-loss order
            stop_side = "SELL" if side == "LONG" else "BUY"
            stop_limit = stop_price * (Decimal("0.999") if side == "LONG" else Decimal("1.001"))
            
            stop_result = adapter.place_margin_order(
                symbol=symbol,
                side=stop_side,
                order_type="STOP_LOSS_LIMIT",
                quantity=sizing.quantity,
                price=stop_limit.quantize(Decimal("0.01")),
                stop_price=stop_price,
                side_effect_type="AUTO_REPAY",
            )
            
            result["stop_order"] = {
                "success": stop_result.success,
                "order_id": stop_result.binance_order_id,
                "stop_price": str(stop_price),
                "limit_price": str(stop_limit.quantize(Decimal("0.01"))),
                "status": stop_result.status,
                "error": stop_result.error_message,
            }
            
            # Place take-profit if specified
            if target_price:
                tp_result = adapter.place_margin_order(
                    symbol=symbol,
                    side=stop_side,
                    order_type="TAKE_PROFIT_LIMIT",
                    quantity=sizing.quantity,
                    price=target_price,
                    stop_price=target_price,
                    side_effect_type="AUTO_REPAY",
                )
                
                result["target_order"] = {
                    "success": tp_result.success,
                    "order_id": tp_result.binance_order_id,
                    "target_price": str(target_price),
                    "status": tp_result.status,
                    "error": tp_result.error_message,
                }
            
            result["status"] = "SUCCESS"
            
            if output_json:
                self.stdout.write(json.dumps(result, indent=2))
            else:
                self._print_execution_report(result)
            
            sys.exit(0)
            
        except Exception as e:
            if output_json:
                self.stdout.write(json.dumps({"error": str(e), **result}))
            else:
                self.stderr.write(self.style.ERROR(f"Execution error: {e}"))
            sys.exit(1)
    
    def _validate_inputs(self, side, entry_price, stop_price, target_price, leverage):
        """Validate trading inputs."""
        if leverage < 1 or leverage > 10:
            raise ValueError("Leverage must be between 1 and 10")
        
        if side == "LONG":
            if stop_price >= entry_price:
                raise ValueError(f"LONG stop must be below entry (stop: {stop_price} >= entry: {entry_price})")
            if target_price and target_price <= entry_price:
                raise ValueError("LONG target must be above entry")
        else:  # SHORT
            if stop_price <= entry_price:
                raise ValueError(f"SHORT stop must be above entry (stop: {stop_price} <= entry: {entry_price})")
            if target_price and target_price >= entry_price:
                raise ValueError("SHORT target must be below entry")
    
    def _print_sizing_report(self, result):
        """Print human-readable sizing report."""
        sizing = result["position_sizing"]
        
        self.stdout.write("\n" + "=" * 60)
        self.stdout.write(self.style.SUCCESS("POSITION SIZING PREVIEW"))
        self.stdout.write("=" * 60)
        self.stdout.write("")
        self.stdout.write(f"Symbol:        {result['symbol']}")
        self.stdout.write(f"Side:          {result['side']}")
        self.stdout.write(f"Entry Price:   ${result['entry_price']}")
        self.stdout.write(f"Stop Price:    ${result['stop_price']}")
        if result['target_price']:
            self.stdout.write(f"Target Price:  ${result['target_price']}")
        self.stdout.write(f"Leverage:      {result['leverage']}x")
        self.stdout.write("")
        self.stdout.write("-" * 40)
        self.stdout.write(self.style.WARNING("CALCULATED VALUES"))
        self.stdout.write("-" * 40)
        self.stdout.write(f"Quantity:      {sizing['quantity']}")
        self.stdout.write(f"Position Value:${sizing['position_value']}")
        self.stdout.write(f"Margin Needed: ${sizing['margin_required']}")
        self.stdout.write("")
        self.stdout.write(f"Risk Amount:   ${sizing['risk_amount']}")
        self.stdout.write(f"Risk Percent:  {sizing['risk_percent']}%")
        self.stdout.write(f"Stop Distance: {sizing['stop_distance_percent']}%")
        self.stdout.write("")
        if sizing['is_capped']:
            self.stdout.write(self.style.WARNING(f"⚠️ Position capped: {sizing['cap_reason']}"))
        self.stdout.write("")
        self.stdout.write("=" * 60)
        self.stdout.write("To execute, add --live --acknowledge-risk")
        self.stdout.write("=" * 60)
    
    def _print_execution_report(self, result):
        """Print human-readable execution report."""
        self.stdout.write("")
        self.stdout.write(self.style.SUCCESS("POSITION OPENED"))
        self.stdout.write("-" * 40)
        
        entry = result.get("entry_order", {})
        self.stdout.write(f"Entry Order:   {entry.get('status', 'N/A')}")
        self.stdout.write(f"  Order ID:    {entry.get('order_id', 'N/A')}")
        self.stdout.write(f"  Filled:      {entry.get('filled_quantity', 'N/A')}")
        self.stdout.write(f"  Avg Price:   {entry.get('avg_fill_price', 'N/A')}")
        
        stop = result.get("stop_order", {})
        self.stdout.write(f"\nStop Order:    {stop.get('status', 'N/A')}")
        self.stdout.write(f"  Order ID:    {stop.get('order_id', 'N/A')}")
        self.stdout.write(f"  Stop Price:  {stop.get('stop_price', 'N/A')}")
        
        if "target_order" in result:
            target = result["target_order"]
            self.stdout.write(f"\nTarget Order:  {target.get('status', 'N/A')}")
            self.stdout.write(f"  Order ID:    {target.get('order_id', 'N/A')}")
            self.stdout.write(f"  Target:      {target.get('target_price', 'N/A')}")
        
        self.stdout.write("")
        self.stdout.write("=" * 60)

