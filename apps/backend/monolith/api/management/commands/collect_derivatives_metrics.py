"""
Django management command for collecting derivatives metrics.

Collects funding rate, open interest, and mark price from Binance Futures API
and persists to database for later use by Market Context Engine.

Usage:
    # Single run (collect once)
    python manage.py collect_derivatives_metrics --symbol BTCUSDT --client-id 1

    # Continuous mode (loop with interval)
    python manage.py collect_derivatives_metrics --symbol BTCUSDT --client-id 1 --continuous --interval 60

    # Multiple symbols
    python manage.py collect_derivatives_metrics --symbol BTCUSDT,ETHUSDT --client-id 1

Part of Core 2: Market Research & Context Engine (ADR-0017)
"""

from django.core.management.base import BaseCommand, CommandError
from django.db import transaction
import time
import logging

logger = logging.getLogger(__name__)


class Command(BaseCommand):
    help = (
        "Collect derivatives metrics (funding rate, OI, mark price) from Binance Futures API. "
        "Supports single run or continuous mode for scheduled execution."
    )

    def add_arguments(self, parser):
        """Define command-line arguments."""
        # Required arguments
        parser.add_argument(
            "--symbol",
            type=str,
            required=True,
            help="Trading pair(s) to collect (comma-separated for multiple, e.g., 'BTCUSDT,ETHUSDT')",
        )

        parser.add_argument(
            "--client-id",
            type=int,
            required=True,
            help="Client ID (tenant) for multi-tenant isolation",
        )

        # Optional arguments
        parser.add_argument(
            "--continuous",
            action="store_true",
            help="Run continuously in loop mode (for scheduled jobs)",
        )

        parser.add_argument(
            "--interval",
            type=int,
            default=60,
            help="Interval between collections in seconds (default: 60, only used with --continuous)",
        )

        parser.add_argument(
            "--testnet",
            action="store_true",
            help="Use Binance testnet instead of production (default: from settings.BINANCE_USE_TESTNET)",
        )

    def handle(self, *args, **options):
        """Execute the command."""
        # Extract options
        symbol_str = options["symbol"]
        client_id = options["client_id"]
        continuous = options["continuous"]
        interval = options["interval"]
        use_testnet = options.get("testnet", None)

        # Parse symbols (comma-separated)
        symbols = [s.strip().upper() for s in symbol_str.split(",")]

        # Validate client exists
        try:
            from clients.models import Client

            client = Client.objects.get(id=client_id)
        except Client.DoesNotExist:
            raise CommandError(f"Client with ID {client_id} does not exist")

        # Log startup
        mode = "CONTINUOUS" if continuous else "SINGLE RUN"
        testnet_str = "TESTNET" if use_testnet else "PRODUCTION"
        self.stdout.write(
            self.style.SUCCESS(
                f"Starting derivatives metrics collection ({mode}, {testnet_str})"
            )
        )
        self.stdout.write(f"Symbols: {', '.join(symbols)}")
        self.stdout.write(f"Client: {client.name} (ID: {client_id})")
        if continuous:
            self.stdout.write(f"Interval: {interval} seconds")

        # Initialize use case (dependency injection)
        from api.application.market_context import (
            BinanceDerivativesAdapter,
            DjangoMetricRepository,
            CollectDerivativesMetrics,
        )

        collector = BinanceDerivativesAdapter(use_testnet=use_testnet)
        repository = DjangoMetricRepository(client_id=client_id)
        use_case = CollectDerivativesMetrics(collector, repository)

        # Execute collection
        if continuous:
            self._continuous_collection(use_case, symbols, interval)
        else:
            self._single_collection(use_case, symbols)

    def _single_collection(self, use_case, symbols):
        """Execute a single collection run for all symbols."""
        self.stdout.write(
            self.style.MIGRATE_HEADING(
                f"Collecting metrics for {len(symbols)} symbol(s)..."
            )
        )

        total_metrics = 0

        for symbol in symbols:
            try:
                with transaction.atomic():
                    count = use_case.execute(symbol)
                    total_metrics += count

                self.stdout.write(
                    self.style.SUCCESS(
                        f"✓ {symbol}: {count} metrics collected"
                    )
                )

            except Exception as e:
                self.stdout.write(
                    self.style.ERROR(f"✗ {symbol}: Failed - {e}")
                )
                logger.exception(f"Failed to collect metrics for {symbol}")

        # Summary
        self.stdout.write(
            self.style.SUCCESS(
                f"\nCollection complete: {total_metrics} total metrics"
            )
        )

    def _continuous_collection(self, use_case, symbols, interval):
        """Execute continuous collection loop."""
        self.stdout.write(
            self.style.WARNING(
                f"Starting continuous collection (Ctrl+C to stop)..."
            )
        )

        iteration = 0

        try:
            while True:
                iteration += 1
                self.stdout.write(
                    self.style.MIGRATE_HEADING(
                        f"\n[Iteration {iteration}] Collecting metrics..."
                    )
                )

                total_metrics = 0

                for symbol in symbols:
                    try:
                        with transaction.atomic():
                            count = use_case.execute(symbol)
                            total_metrics += count

                        self.stdout.write(
                            f"✓ {symbol}: {count} metrics"
                        )

                    except Exception as e:
                        self.stdout.write(
                            self.style.ERROR(f"✗ {symbol}: {e}")
                        )
                        logger.exception(
                            f"Failed to collect metrics for {symbol}"
                        )

                self.stdout.write(
                    f"Iteration {iteration} complete: {total_metrics} metrics"
                )
                self.stdout.write(f"Sleeping for {interval} seconds...")
                time.sleep(interval)

        except KeyboardInterrupt:
            self.stdout.write(
                self.style.WARNING(
                    f"\n\nStopped by user after {iteration} iterations"
                )
            )
