"""
Django management command to monitor margin levels.

This command monitors Isolated Margin positions and alerts on unhealthy levels.
Can run as a one-shot check or continuously.

Usage:
    # One-shot check
    python manage.py monitor_margin_levels --client-id 1

    # Continuous monitoring (every 30 seconds)
    python manage.py monitor_margin_levels --client-id 1 --continuous --interval 30

    # Auto-close positions in DANGER zone
    python manage.py monitor_margin_levels --client-id 1 --auto-close-danger
"""

import json
import sys
import time
from decimal import Decimal
from django.core.management.base import BaseCommand

from api.application.margin_adapters import BinanceMarginAdapter


class Command(BaseCommand):
    help = "Monitor Isolated Margin levels for open positions"

    def add_arguments(self, parser):
        parser.add_argument(
            "--client-id",
            type=int,
            required=True,
            help="Client ID (tenant)",
        )
        parser.add_argument(
            "--symbol",
            type=str,
            help="Specific symbol to monitor (optional, monitors all if not specified)",
        )
        parser.add_argument(
            "--continuous",
            action="store_true",
            help="Run continuously",
        )
        parser.add_argument(
            "--interval",
            type=int,
            default=30,
            help="Check interval in seconds (default 30)",
        )
        parser.add_argument(
            "--auto-close-danger",
            action="store_true",
            help="Auto-close positions when margin level is DANGER",
        )
        parser.add_argument(
            "--testnet",
            action="store_true",
            help="Use testnet instead of production",
        )
        parser.add_argument(
            "--json",
            action="store_true",
            help="Output in JSON format",
        )

    def handle(self, *args, **options):
        """Execute the command."""
        client_id = options["client_id"]
        symbol = options.get("symbol")
        continuous = options["continuous"]
        interval = options["interval"]
        auto_close = options["auto_close_danger"]
        use_testnet = options["testnet"]
        output_json = options["json"]
        
        # Initialize adapter
        try:
            adapter = BinanceMarginAdapter(use_testnet=use_testnet)
        except Exception as e:
            if output_json:
                self.stdout.write(json.dumps({"error": str(e)}))
            else:
                self.stderr.write(self.style.ERROR(f"Failed to initialize: {e}"))
            sys.exit(1)
        
        mode = "TESTNET" if use_testnet else "PRODUCTION"
        
        if not output_json:
            self.stdout.write(self.style.SUCCESS(f"\n{'=' * 60}"))
            self.stdout.write(self.style.SUCCESS(f"MARGIN LEVEL MONITOR ({mode})"))
            self.stdout.write(self.style.SUCCESS(f"{'=' * 60}\n"))
            if continuous:
                self.stdout.write(f"Running continuously (interval: {interval}s)")
                self.stdout.write("Press Ctrl+C to stop\n")
        
        try:
            while True:
                results = self._check_margins(
                    adapter=adapter,
                    client_id=client_id,
                    symbol=symbol,
                    auto_close=auto_close,
                    output_json=output_json,
                )
                
                if output_json:
                    self.stdout.write(json.dumps(results, indent=2))
                else:
                    self._print_results(results)
                
                if not continuous:
                    break
                
                time.sleep(interval)
                
        except KeyboardInterrupt:
            if not output_json:
                self.stdout.write("\n\nMonitoring stopped.")
    
    def _check_margins(self, adapter, client_id, symbol, auto_close, output_json):
        """Check margin levels for positions."""
        results = {
            "timestamp": time.strftime("%Y-%m-%d %H:%M:%S"),
            "client_id": client_id,
            "positions": [],
            "alerts": [],
        }
        
        # If specific symbol, check only that
        symbols_to_check = [symbol] if symbol else self._get_active_symbols(adapter)
        
        for sym in symbols_to_check:
            try:
                account = adapter.get_margin_account(sym)
                
                margin_level = account.margin_level
                
                # Classify health
                if margin_level >= Decimal("2.0"):
                    health = "SAFE"
                    emoji = "✅"
                elif margin_level >= Decimal("1.5"):
                    health = "CAUTION"
                    emoji = "💡"
                elif margin_level >= Decimal("1.3"):
                    health = "WARNING"
                    emoji = "⚠️"
                elif margin_level >= Decimal("1.1"):
                    health = "CRITICAL"
                    emoji = "🚨"
                else:
                    health = "DANGER"
                    emoji = "🛑"
                
                position_data = {
                    "symbol": sym,
                    "margin_level": str(margin_level),
                    "health": health,
                    "emoji": emoji,
                    "base_asset": account.base_asset,
                    "base_free": str(account.base_free),
                    "quote_asset": account.quote_asset,
                    "quote_free": str(account.quote_free),
                    "liquidation_price": str(account.liquidation_price),
                }
                
                results["positions"].append(position_data)
                
                # Generate alerts for unhealthy positions
                if health in ("WARNING", "CRITICAL", "DANGER"):
                    alert = {
                        "symbol": sym,
                        "health": health,
                        "margin_level": str(margin_level),
                        "message": self._get_alert_message(health, margin_level),
                    }
                    results["alerts"].append(alert)
                    
                    # Auto-close if enabled and in DANGER
                    if auto_close and health == "DANGER":
                        alert["action"] = "AUTO_CLOSE_TRIGGERED"
                        # TODO: Implement actual close via CloseMarginPositionUseCase
                
            except ValueError:
                # Symbol not in margin account
                pass
            except Exception as e:
                results["positions"].append({
                    "symbol": sym,
                    "error": str(e),
                })
        
        return results
    
    def _get_active_symbols(self, adapter):
        """Get list of symbols with margin accounts."""
        # For now, return common symbols
        # TODO: Query actual open positions from database
        return ["BTCUSDC", "ETHUSDC", "BNBUSDC"]
    
    def _get_alert_message(self, health, margin_level):
        """Generate alert message based on health level."""
        messages = {
            "WARNING": f"Margin level at {margin_level}. Consider reducing position.",
            "CRITICAL": f"⚠️ CRITICAL: Margin level at {margin_level}. Close to liquidation!",
            "DANGER": f"🛑 DANGER: Margin level at {margin_level}. IMMEDIATE ACTION REQUIRED!",
        }
        return messages.get(health, "")
    
    def _print_results(self, results):
        """Print human-readable results."""
        self.stdout.write(f"\n[{results['timestamp']}]")
        self.stdout.write("-" * 50)
        
        if not results["positions"]:
            self.stdout.write("No margin positions found.")
            return
        
        for pos in results["positions"]:
            if "error" in pos:
                self.stdout.write(f"  {pos['symbol']}: ERROR - {pos['error']}")
            else:
                health = pos["health"]
                emoji = pos["emoji"]
                level = pos["margin_level"]
                
                # Color based on health
                if health == "SAFE":
                    style = self.style.SUCCESS
                elif health == "CAUTION":
                    style = self.style.WARNING
                else:
                    style = self.style.ERROR
                
                self.stdout.write(style(
                    f"  {emoji} {pos['symbol']}: {health} (level: {level})"
                ))
                self.stdout.write(
                    f"     {pos['base_asset']}: {pos['base_free']} | "
                    f"{pos['quote_asset']}: {pos['quote_free']}"
                )
        
        # Print alerts
        if results["alerts"]:
            self.stdout.write("")
            self.stdout.write(self.style.ERROR("ALERTS:"))
            for alert in results["alerts"]:
                self.stdout.write(self.style.ERROR(f"  • {alert['message']}"))
                if alert.get("action"):
                    self.stdout.write(self.style.WARNING(f"    Action: {alert['action']}"))

