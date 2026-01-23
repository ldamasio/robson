"""
Management command to seed default LONG and SHORT strategies for BTC/USDC.

This ensures that users always have pre-configured strategies available
for Isolated Margin trading on BTC/USDC.
"""

from clients.models import Client
from django.core.management.base import BaseCommand

from api.models.trading import Strategy


class Command(BaseCommand):
    help = "Seed default BTC/USDC LONG and SHORT strategies for all clients"

    def handle(self, *args, **options):
        clients = Client.objects.all()

        if not clients.exists():
            self.stdout.write(self.style.WARNING("No clients found - skipping strategy seeding"))
            return

        for client in clients:
            # Create LONG strategy
            long_strategy, long_created = Strategy.objects.get_or_create(
                client=client,
                name="BTC Long",
                defaults={
                    "description": "Estratégia de compra (LONG) para BTC/USDC Isolated Margin",
                    "config": {
                        "symbol": "BTCUSDC",
                        "side": "BUY",
                        "account_type": "ISOLATED_MARGIN",
                        "default_leverage": 3,
                    },
                    "risk_config": {
                        "max_position_size_percent": 10,
                        "default_stop_loss_percent": 2,
                        "default_take_profit_percent": 6,
                    },
                    "is_active": True,
                },
            )

            if long_created:
                self.stdout.write(
                    self.style.SUCCESS(f"✅ Created LONG strategy for client {client.name}")
                )
            else:
                self.stdout.write(
                    self.style.WARNING(f"⚠️  LONG strategy already exists for client {client.name}")
                )

            # Create SHORT strategy
            short_strategy, short_created = Strategy.objects.get_or_create(
                client=client,
                name="BTC Short",
                defaults={
                    "description": "Estratégia de venda (SHORT) para BTC/USDC Isolated Margin",
                    "config": {
                        "symbol": "BTCUSDC",
                        "side": "SELL",
                        "account_type": "ISOLATED_MARGIN",
                        "default_leverage": 3,
                    },
                    "risk_config": {
                        "max_position_size_percent": 10,
                        "default_stop_loss_percent": 2,
                        "default_take_profit_percent": 6,
                    },
                    "is_active": True,
                },
            )

            if short_created:
                self.stdout.write(
                    self.style.SUCCESS(f"✅ Created SHORT strategy for client {client.name}")
                )
            else:
                self.stdout.write(
                    self.style.WARNING(f"⚠️  SHORT strategy already exists for client {client.name}")
                )

        self.stdout.write(
            self.style.SUCCESS(f"\n🎉 Strategy seeding complete for {clients.count()} client(s)")
        )
