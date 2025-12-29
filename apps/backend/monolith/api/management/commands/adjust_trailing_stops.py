"""
Management command to adjust trailing stops for all eligible positions.

Usage:
    python manage.py adjust_trailing_stops
    python manage.py adjust_trailing_stops --client-id 1
    python manage.py adjust_trailing_stops --dry-run
    python manage.py adjust_trailing_stops --position-id 123
"""

from django.core.management.base import BaseCommand
from decimal import Decimal
import logging

logger = logging.getLogger(__name__)


class Command(BaseCommand):
    """Adjust trailing stops for eligible positions."""

    help = "Adjust hand-span trailing stops for all eligible positions"

    def add_arguments(self, parser):
        """Add command arguments."""
        parser.add_argument(
            '--client-id',
            type=int,
            help='Only adjust positions for this client ID (multi-tenant filter)',
        )
        parser.add_argument(
            '--position-id',
            type=str,
            help='Only adjust this specific position ID',
        )
        parser.add_argument(
            '--dry-run',
            action='store_true',
            help='Simulate adjustments without making changes',
        )
        parser.add_argument(
            '--fee-percent',
            type=float,
            default=0.1,
            help='Trading fee percentage (default: 0.1%%)',
        )
        parser.add_argument(
            '--slippage-percent',
            type=float,
            default=0.05,
            help='Slippage buffer percentage (default: 0.05%%)',
        )

    def handle(self, *args, **options):
        """Execute the command."""
        from api.application.trailing_stop import (
            HandSpanCalculator,
            AdjustTrailingStopUseCase,
            AdjustAllTrailingStopsUseCase,
        )
        from api.application.trailing_stop.domain import FeeConfig
        from api.application.trailing_stop.adapters import (
            BinancePriceProvider,
            DjangoTrailingStopRepository,
            ActivePositionFilter,
            StopAdjustmentEventPublisher,
            LoggingNotificationService,
        )

        # Parse options
        client_id = options.get('client_id')
        position_id = options.get('position_id')
        dry_run = options.get('dry_run', False)
        fee_percent = Decimal(str(options.get('fee_percent', 0.1)))
        slippage_percent = Decimal(str(options.get('slippage_percent', 0.05)))

        # Setup fee configuration
        fee_config = FeeConfig(
            trading_fee_percent=fee_percent,
            slippage_buffer_percent=slippage_percent,
        )

        # Setup dependencies
        calculator = HandSpanCalculator(fee_config=fee_config)
        price_provider = BinancePriceProvider()
        repository = DjangoTrailingStopRepository()
        event_publisher = StopAdjustmentEventPublisher()
        notification_service = LoggingNotificationService()

        # Create use case
        adjust_use_case = AdjustTrailingStopUseCase(
            calculator=calculator,
            price_provider=price_provider,
            repository=repository,
            event_publisher=event_publisher,
            notification_service=notification_service,
        )

        # DRY-RUN: Override repository methods to prevent changes
        if dry_run:
            self.stdout.write(self.style.WARNING("🔍 DRY-RUN MODE: No changes will be made"))
            original_update_stop = repository.update_stop
            original_save_adjustment = repository.save_adjustment

            def dry_run_update_stop(pos_id, new_stop):
                self.stdout.write(
                    f"  [DRY-RUN] Would update position {pos_id} stop to {new_stop}"
                )

            def dry_run_save_adjustment(adjustment):
                self.stdout.write(
                    f"  [DRY-RUN] Would save adjustment: {adjustment.adjustment_token}"
                )

            repository.update_stop = dry_run_update_stop
            repository.save_adjustment = dry_run_save_adjustment

        # Execute
        if position_id:
            # Single position
            self.stdout.write(f"Adjusting trailing stop for position {position_id}...")
            result = adjust_use_case.execute(position_id)
            self._display_result(result)
        else:
            # All eligible positions
            position_filter = ActivePositionFilter(client_id=client_id)

            adjust_all_use_case = AdjustAllTrailingStopsUseCase(
                adjust_use_case=adjust_use_case,
                adjustment_filter=position_filter,
            )

            self.stdout.write("Adjusting trailing stops for all eligible positions...")
            if client_id:
                self.stdout.write(f"  Filter: Client ID = {client_id}")

            results = adjust_all_use_case.execute()

            # Display summary
            self._display_summary(results)

        if dry_run:
            self.stdout.write(self.style.WARNING("\n✅ DRY-RUN COMPLETE (no changes made)"))

    def _display_result(self, result):
        """Display a single adjustment result."""
        if result.adjusted:
            self.stdout.write(
                self.style.SUCCESS(
                    f"✅ Position {result.position_id}: "
                    f"{result.adjustment.old_stop} → {result.adjustment.new_stop} "
                    f"({result.adjustment.reason.value}, step {result.adjustment.step_index})"
                )
            )
        elif result.error:
            self.stdout.write(
                self.style.ERROR(
                    f"❌ Position {result.position_id}: {result.error}"
                )
            )
        else:
            self.stdout.write(
                self.style.WARNING(
                    f"⚠️  Position {result.position_id}: No adjustment needed"
                )
            )

    def _display_summary(self, results):
        """Display summary of all adjustments."""
        adjusted_count = sum(1 for r in results if r.adjusted)
        error_count = sum(1 for r in results if r.error)
        no_change_count = len(results) - adjusted_count - error_count

        self.stdout.write("\n" + "=" * 60)
        self.stdout.write(f"Total positions checked: {len(results)}")
        self.stdout.write(self.style.SUCCESS(f"  ✅ Adjusted: {adjusted_count}"))
        self.stdout.write(self.style.WARNING(f"  ⚠️  No change: {no_change_count}"))
        self.stdout.write(self.style.ERROR(f"  ❌ Errors: {error_count}"))
        self.stdout.write("=" * 60)

        # Display each adjustment
        if adjusted_count > 0:
            self.stdout.write("\nAdjustments made:")
            for result in results:
                if result.adjusted:
                    adj = result.adjustment
                    self.stdout.write(
                        f"  • Position {result.position_id}: "
                        f"{adj.old_stop} → {adj.new_stop} "
                        f"({adj.reason.value}, step {adj.step_index}, "
                        f"price {adj.current_price})"
                    )

        # Display errors
        if error_count > 0:
            self.stdout.write("\nErrors:")
            for result in results:
                if result.error:
                    self.stdout.write(
                        self.style.ERROR(
                            f"  • Position {result.position_id}: {result.error}"
                        )
                    )
