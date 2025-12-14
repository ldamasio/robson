"""
Django management command for plan execution.

This command is invoked by robson-go to execute plans.
It enforces SAFE BY DEFAULT semantics:
- DRY-RUN is the default (no real orders)
- LIVE requires explicit acknowledgement
- All executions are audited

Usage:
    # DRY-RUN (default, safe)
    python manage.py execute_plan --plan-id abc123 --client-id 1

    # LIVE (requires acknowledgement)
    python manage.py execute_plan --plan-id abc123 --client-id 1 --live --acknowledge-risk
"""

import json
import sys
from django.core.management.base import BaseCommand
from decimal import Decimal

from api.application import (
    ExecutePlanUseCase,
    ExecutionMode,
    ExecutionStatus,
)
from api.models import Strategy, Order


class Command(BaseCommand):
    help = "Execute a plan (DRY-RUN by default, LIVE requires --live and --acknowledge-risk)"

    def add_arguments(self, parser):
        parser.add_argument(
            "--plan-id",
            type=str,
            required=True,
            help="Plan ID to execute",
        )
        parser.add_argument(
            "--client-id",
            type=int,
            required=True,
            help="Client ID (tenant) - MANDATORY",
        )
        parser.add_argument(
            "--strategy-id",
            type=int,
            help="Strategy ID for limits and configuration",
        )
        parser.add_argument(
            "--operation-type",
            type=str,
            choices=["buy", "sell", "cancel"],
            help="Operation type",
        )
        parser.add_argument(
            "--symbol",
            type=str,
            help="Trading symbol (e.g., BTCUSDT)",
        )
        parser.add_argument(
            "--quantity",
            type=str,
            help="Order quantity",
        )
        parser.add_argument(
            "--price",
            type=str,
            help="Order price (for limit orders)",
        )
        parser.add_argument(
            "--live",
            action="store_true",
            help="LIVE mode (real orders) - requires --acknowledge-risk",
        )
        parser.add_argument(
            "--acknowledge-risk",
            action="store_true",
            help="Acknowledge risk of LIVE execution (real orders will be placed)",
        )
        parser.add_argument(
            "--validated",
            action="store_true",
            help="Mark plan as validated (normally set by prior validation step)",
        )
        parser.add_argument(
            "--validation-passed",
            action="store_true",
            help="Mark validation as passed (normally set by prior validation step)",
        )
        parser.add_argument(
            "--json",
            action="store_true",
            help="Output in JSON format",
        )

    def handle(self, *args, **options):
        """Execute plan with safety checks."""
        plan_id = options["plan_id"]
        client_id = options["client_id"]
        live_mode = options["live"]
        output_json = options["json"]

        # Determine execution mode
        mode = ExecutionMode.LIVE if live_mode else ExecutionMode.DRY_RUN

        # Build execution context
        context = {
            "plan_id": plan_id,
            "client_id": client_id,
            "mode": mode,
            "acknowledge_risk": options.get("acknowledge_risk", False),
            "validated": options.get("validated", False),
            "validation_passed": options.get("validation_passed", False),
        }

        # Load strategy limits (if provided)
        strategy_id = options.get("strategy_id")
        if strategy_id:
            try:
                strategy = Strategy.objects.get(id=strategy_id, client_id=client_id)
                risk_config = strategy.risk_config or {}

                # Extract limits
                context["limits"] = {
                    "max_orders_per_day": risk_config.get("max_orders_per_day"),
                    "max_notional_per_day": risk_config.get("max_notional_per_day"),
                    "max_loss_per_day": risk_config.get("max_loss_per_day"),
                }

                # Get current stats (orders today)
                from datetime import datetime, timezone as tz
                today_start = datetime.now(tz.utc).replace(hour=0, minute=0, second=0, microsecond=0)
                orders_today = Order.objects.filter(
                    client_id=client_id,
                    created_at__gte=today_start,
                ).count()

                context["stats"] = {
                    "orders_today": orders_today,
                    "notional_today": 0,  # TODO: Calculate from orders
                    "loss_today": 0,  # TODO: Calculate from positions
                }

            except Strategy.DoesNotExist:
                if output_json:
                    self.stdout.write(
                        json.dumps(
                            {
                                "status": "FAILED",
                                "error": f"Strategy {strategy_id} not found for client {client_id}",
                            }
                        )
                    )
                else:
                    self.stderr.write(
                        self.style.ERROR(
                            f"Strategy {strategy_id} not found for client {client_id}"
                        )
                    )
                sys.exit(1)
        else:
            context["limits"] = {}
            context["stats"] = {
                "orders_today": 0,
                "notional_today": 0,
                "loss_today": 0,
            }

        # Build operation context
        operation = {}
        if options.get("operation_type"):
            operation["type"] = options["operation_type"]
        if options.get("symbol"):
            operation["symbol"] = options["symbol"]
        if options.get("quantity"):
            operation["quantity"] = options["quantity"]
        if options.get("price"):
            operation["price"] = options["price"]

        if operation:
            context["operation"] = operation

        # Execute plan
        use_case = ExecutePlanUseCase()
        result = use_case.execute(context)

        # Output results
        if output_json:
            self.stdout.write(json.dumps(result.to_dict(), indent=2))
        else:
            self.stdout.write(result.to_human_readable())

        # Exit with appropriate code
        if result.is_blocked() or result.status == ExecutionStatus.FAILED:
            sys.exit(1)
        else:
            sys.exit(0)
