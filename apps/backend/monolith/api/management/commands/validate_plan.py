"""
Django management command for plan validation.

This command is invoked by robson-go to perform operational and financial validation.
It acts as the "paper trading" stage of the agentic workflow.

Usage:
    python manage.py validate_plan --plan-id abc123 --client-id 1 --json
"""

import json
import sys
from django.core.management.base import BaseCommand, CommandError

from api.application import ValidatePlanUseCase
from api.models import Strategy


class Command(BaseCommand):
    help = "Validate an execution plan (operational and financial validation)"

    def add_arguments(self, parser):
        parser.add_argument(
            "--plan-id",
            type=str,
            required=True,
            help="Plan ID to validate",
        )
        parser.add_argument(
            "--client-id",
            type=int,
            required=True,
            help="Client ID (tenant) - MANDATORY for tenant isolation",
        )
        parser.add_argument(
            "--strategy-id",
            type=int,
            help="Strategy ID to load risk configuration from",
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
            "--json",
            action="store_true",
            help="Output in JSON format",
        )

    def handle(self, *args, **options):
        """Execute validation."""
        plan_id = options["plan_id"]
        client_id = options["client_id"]
        output_json = options["json"]

        # Build validation context
        context = {
            "plan_id": plan_id,
            "client_id": client_id,
        }

        # Load risk configuration from strategy (if provided)
        strategy_id = options.get("strategy_id")
        if strategy_id:
            try:
                strategy = Strategy.objects.get(id=strategy_id, client_id=client_id)
                context["risk_config"] = strategy.risk_config
                context["strategy_name"] = strategy.name
            except Strategy.DoesNotExist:
                if output_json:
                    self.stdout.write(
                        json.dumps(
                            {
                                "status": "FAIL",
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
            # No strategy specified, use empty risk config
            # (validation will fail if risk config is required)
            context["risk_config"] = {}

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

        # Execute validation
        use_case = ValidatePlanUseCase()
        report = use_case.execute(context)

        # Output results
        if output_json:
            self.stdout.write(json.dumps(report.to_dict(), indent=2))
        else:
            self.stdout.write(report.to_human_readable())

        # Exit with appropriate code
        if report.has_failures():
            sys.exit(1)
        else:
            sys.exit(0)
