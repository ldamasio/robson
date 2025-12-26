"""
Django Management Command: Sync Deposits and Withdrawals

This command syncs deposit and withdrawal history from Binance
to the audit trail.

Usage:
    python manage.py sync_deposits                    # Sync last 30 days
    python manage.py sync_deposits --days-back 90     # Sync last 90 days
    python manage.py sync_deposits --client-id 2      # Sync for specific client
"""

from django.core.management.base import BaseCommand
from clients.models import Client
from api.services.audit_service import AuditService


class Command(BaseCommand):
    help = 'Sync deposit and withdrawal history from Binance'

    def add_arguments(self, parser):
        parser.add_argument(
            '--client-id',
            type=int,
            default=1,
            help='Client ID (default: 1)',
        )
        parser.add_argument(
            '--days-back',
            type=int,
            default=30,
            help='Days back to sync (default: 30)',
        )

    def handle(self, *args, **options):
        client_id = options['client_id']
        days_back = options['days_back']

        self.stdout.write(f"Syncing deposits/withdrawals for client {client_id} (last {days_back} days)...")

        try:
            client = Client.objects.get(id=client_id)
            audit_service = AuditService(client)

            count = audit_service.sync_deposits_and_withdrawals(days_back=days_back)

            if count > 0:
                self.stdout.write(self.style.SUCCESS(f"Successfully synced {count} transactions"))
            else:
                self.stdout.write(self.style.WARNING("No new transactions found"))

        except Client.DoesNotExist:
            self.stdout.write(self.style.ERROR(f"Client {client_id} does not exist"))
        except Exception as e:
            self.stdout.write(self.style.ERROR(f"Error: {str(e)}"))
