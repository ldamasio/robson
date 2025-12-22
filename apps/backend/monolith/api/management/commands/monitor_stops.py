"""
Management command to monitor and execute stop losses.

Usage:
    # Single check
    python manage.py monitor_stops
    
    # Continuous monitoring (every 5 seconds)
    python manage.py monitor_stops --continuous --interval 5
    
    # Dry run (no execution)
    python manage.py monitor_stops --dry-run
"""

import time
import json
from django.core.management.base import BaseCommand

from api.application.stop_monitor import (
    PriceMonitor,
    StopExecutor,
    run_stop_monitor,
    TriggerType,
)


class Command(BaseCommand):
    help = "Monitor active operations and execute stops when triggered"

    def add_arguments(self, parser):
        parser.add_argument(
            "--continuous",
            action="store_true",
            help="Run continuously (loop)",
        )
        parser.add_argument(
            "--interval",
            type=int,
            default=5,
            help="Check interval in seconds (default: 5)",
        )
        parser.add_argument(
            "--dry-run",
            action="store_true",
            help="Check only, don't execute",
        )
        parser.add_argument(
            "--json",
            action="store_true",
            help="Output in JSON format",
        )

    def handle(self, *args, **options):
        continuous = options["continuous"]
        interval = options["interval"]
        dry_run = options["dry_run"]
        output_json = options["json"]

        if continuous:
            self.stdout.write("🔍 Starting continuous stop monitor...")
            self.stdout.write(f"   Interval: {interval}s")
            self.stdout.write(f"   Dry run: {dry_run}")
            self.stdout.write("   Press Ctrl+C to stop")
            self.stdout.write("")

        try:
            while True:
                results = self._run_check(dry_run, output_json)
                
                if not continuous:
                    break
                
                time.sleep(interval)
                
        except KeyboardInterrupt:
            self.stdout.write("\n👋 Monitor stopped")

    def _run_check(self, dry_run: bool, output_json: bool):
        """Run a single check cycle."""
        monitor = PriceMonitor()
        executor = StopExecutor() if not dry_run else None
        
        triggers = monitor.check_all_operations()
        
        if not triggers:
            if not output_json:
                self.stdout.write("✓ No triggers")
            return []
        
        results = []
        for trigger in triggers:
            if output_json:
                self.stdout.write(json.dumps({
                    "type": "trigger",
                    "operation_id": trigger.operation_id,
                    "trigger_type": trigger.trigger_type.value,
                    "symbol": trigger.symbol,
                    "current_price": str(trigger.current_price),
                    "trigger_price": str(trigger.trigger_price),
                    "expected_pnl": str(trigger.expected_pnl),
                }))
            else:
                emoji = "🛑" if trigger.trigger_type == TriggerType.STOP_LOSS else "🎯"
                self.stdout.write(
                    f"{emoji} {trigger.trigger_type.value}: Op#{trigger.operation_id} "
                    f"{trigger.symbol} @ {trigger.current_price}"
                )
            
            if not dry_run and executor:
                result = executor.execute(trigger)
                results.append(result)
                
                if output_json:
                    self.stdout.write(json.dumps({
                        "type": "execution",
                        "success": result.success,
                        "operation_id": result.operation_id,
                        "order_id": result.order_id,
                        "pnl": str(result.pnl) if result.pnl else None,
                        "error": result.error,
                    }))
                else:
                    if result.success:
                        self.stdout.write(
                            self.style.SUCCESS(f"   ✅ Executed: PnL = {result.pnl}")
                        )
                    else:
                        self.stdout.write(
                            self.style.ERROR(f"   ❌ Failed: {result.error}")
                        )
        
        return results

