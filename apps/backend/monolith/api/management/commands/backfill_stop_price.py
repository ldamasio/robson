# api/management/commands/backfill_stop_price.py
"""
Backfill stop_price from stop_loss_percent for existing operations.

This is separated from migrations for better control:
- Progress monitoring
- Batch processing (no long-running transactions)
- Resume capability on failure
- Production-safe (row-level locks only)

Usage:
    python manage.py backfill_stop_price
    python manage.py backfill_stop_price --batch-size 500
    python manage.py backfill_stop_price --dry-run
"""

from decimal import Decimal
from django.core.management.base import BaseCommand
from django.db import transaction
from api.models import Operation


class Command(BaseCommand):
    help = "Backfill stop_price from stop_loss_percent for existing operations"

    def add_arguments(self, parser):
        parser.add_argument(
            '--batch-size',
            type=int,
            default=1000,
            help='Number of operations to update per batch (default: 1000)',
        )
        parser.add_argument(
            '--dry-run',
            action='store_true',
            help='Show what would be updated without making changes',
        )

    def handle(self, *args, **options):
        batch_size = options['batch_size']
        dry_run = options['dry_run']

        self.stdout.write("🔄 Starting stop_price backfill...")
        if dry_run:
            self.stdout.write(self.style.WARNING("   DRY RUN MODE (no changes will be made)"))
        self.stdout.write(f"   Batch size: {batch_size}")
        self.stdout.write("")

        # Query operations that need backfill
        operations_to_backfill = Operation.objects.filter(
            stop_price__isnull=True,  # No stop_price set yet
            stop_loss_percent__isnull=False,  # Has percentage
            average_entry_price__isnull=False,  # Has entry price (required for calculation)
        )

        total_count = operations_to_backfill.count()

        if total_count == 0:
            self.stdout.write(self.style.SUCCESS("✅ No operations need backfill"))
            return

        self.stdout.write(f"📊 Found {total_count} operations to backfill")
        self.stdout.write("")

        # Process in batches
        updated_count = 0
        skipped_count = 0
        error_count = 0

        for offset in range(0, total_count, batch_size):
            batch = operations_to_backfill[offset:offset + batch_size]

            self.stdout.write(f"Processing batch {offset // batch_size + 1} ({offset + 1}-{min(offset + batch_size, total_count)} of {total_count})...")

            # Prepare updates
            to_update = []

            for op in batch:
                try:
                    # Calculate stop_price from percentage
                    entry_price = op.average_entry_price
                    stop_pct = op.stop_loss_percent

                    if op.side == 'BUY':
                        # Long position: stop below entry
                        op.stop_price = entry_price * (Decimal('1') - stop_pct / Decimal('100'))
                    else:
                        # Short position: stop above entry
                        op.stop_price = entry_price * (Decimal('1') + stop_pct / Decimal('100'))

                    # Calculate target_price if stop_gain_percent exists
                    if op.stop_gain_percent:
                        if op.side == 'BUY':
                            # Long position: target above entry
                            op.target_price = entry_price * (Decimal('1') + op.stop_gain_percent / Decimal('100'))
                        else:
                            # Short position: target below entry
                            op.target_price = entry_price * (Decimal('1') - op.stop_gain_percent / Decimal('100'))

                    # Validate calculated stop_price
                    if op.side == 'BUY' and op.stop_price >= entry_price:
                        self.stderr.write(
                            self.style.ERROR(
                                f"⚠️  Op#{op.id}: Invalid stop (BUY stop >= entry: {op.stop_price} >= {entry_price})"
                            )
                        )
                        skipped_count += 1
                        continue

                    if op.side == 'SELL' and op.stop_price <= entry_price:
                        self.stderr.write(
                            self.style.ERROR(
                                f"⚠️  Op#{op.id}: Invalid stop (SELL stop <= entry: {op.stop_price} <= {entry_price})"
                            )
                        )
                        skipped_count += 1
                        continue

                    to_update.append(op)

                except Exception as e:
                    self.stderr.write(
                        self.style.ERROR(f"❌ Op#{op.id}: Error calculating stop_price: {e}")
                    )
                    error_count += 1

            # Bulk update (single query per batch)
            if to_update and not dry_run:
                with transaction.atomic():
                    Operation.objects.bulk_update(
                        to_update,
                        ['stop_price', 'target_price'],
                        batch_size=batch_size,
                    )
                    updated_count += len(to_update)

            elif to_update and dry_run:
                # Dry run: just log what would be updated
                for op in to_update:
                    self.stdout.write(
                        f"  Op#{op.id}: stop_price={op.stop_price} (from {op.stop_loss_percent}%)"
                    )
                updated_count += len(to_update)

            self.stdout.write(
                self.style.SUCCESS(f"  ✅ Batch complete: {len(to_update)} updated")
            )

        # Summary
        self.stdout.write("")
        self.stdout.write("=" * 60)
        self.stdout.write(self.style.SUCCESS(f"✅ Backfill complete!"))
        self.stdout.write(f"   Total operations processed: {total_count}")
        self.stdout.write(f"   Successfully updated: {updated_count}")
        if skipped_count > 0:
            self.stdout.write(self.style.WARNING(f"   Skipped (validation failed): {skipped_count}"))
        if error_count > 0:
            self.stdout.write(self.style.ERROR(f"   Errors: {error_count}"))

        if dry_run:
            self.stdout.write(self.style.WARNING("   DRY RUN: No changes were made"))

        # Verify backfill success
        if not dry_run:
            remaining = Operation.objects.filter(
                stop_price__isnull=True,
                stop_loss_percent__isnull=False,
                average_entry_price__isnull=False,
            ).count()

            if remaining > 0:
                self.stdout.write("")
                self.stdout.write(
                    self.style.WARNING(f"⚠️  {remaining} operations still need backfill (check errors above)")
                )
            else:
                self.stdout.write("")
                self.stdout.write(
                    self.style.SUCCESS("🎉 All operations successfully backfilled!")
                )
