"""
Sync Transactions Command.

Syncs all transactions from Binance to ensure complete audit trail.

Usage:
    python manage.py sync_transactions              # Sync last 7 days
    python manage.py sync_transactions --days 30   # Sync last 30 days
    python manage.py sync_transactions --snapshot  # Also take balance snapshot
"""

import logging
from django.core.management.base import BaseCommand

from api.services.audit_service import AuditService
from clients.models import Client

logger = logging.getLogger(__name__)


class Command(BaseCommand):
    help = 'Sync transactions from Binance for complete audit trail'

    def add_arguments(self, parser):
        parser.add_argument(
            '--client-id',
            type=int,
            default=1,
            help='Client ID (default: 1)',
        )
        parser.add_argument(
            '--days',
            type=int,
            default=7,
            help='Days to sync back (default: 7)',
        )
        parser.add_argument(
            '--snapshot',
            action='store_true',
            help='Also take a balance snapshot',
        )

    def handle(self, *args, **options):
        client_id = options['client_id']
        days = options['days']
        take_snapshot = options['snapshot']

        self.stdout.write(self.style.HTTP_INFO('=' * 60))
        self.stdout.write(self.style.HTTP_INFO('ROBSON - Transaction Sync'))
        self.stdout.write(self.style.HTTP_INFO('=' * 60))
        self.stdout.write('')

        try:
            client = Client.objects.get(id=client_id)
        except Client.DoesNotExist:
            self.stdout.write(self.style.ERROR(f'Client {client_id} not found'))
            return

        self.stdout.write(f'Client: {client.name}')
        self.stdout.write(f'Syncing last {days} days...')
        self.stdout.write('')

        # Initialize service
        audit_service = AuditService(client)

        # Sync transactions
        self.stdout.write(self.style.HTTP_INFO('--- Syncing Transactions ---'))
        count = audit_service.sync_from_binance(days_back=days)
        self.stdout.write(self.style.SUCCESS(f'Synced {count} new transactions'))

        # Take snapshot if requested
        if take_snapshot:
            self.stdout.write('')
            self.stdout.write(self.style.HTTP_INFO('--- Taking Balance Snapshot ---'))
            snapshot = audit_service.take_balance_snapshot()
            self.stdout.write(self.style.SUCCESS(f'Snapshot taken: ${snapshot.total_equity}'))

        self.stdout.write('')
        self.stdout.write(self.style.HTTP_INFO('=' * 60))
        self.stdout.write(self.style.SUCCESS('Sync complete!'))

