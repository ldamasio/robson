"""
Create Global Symbols Management Command

Creates global system trading symbols (client=null) that are
available to all users. These represent trading pairs like BTC/USDT.

Run: python manage.py create_global_symbols
"""

from django.core.management.base import BaseCommand
from api.models import Symbol


class Command(BaseCommand):
    help = 'Create global system trading symbols (available to all users)'

    # Pre-defined trading pairs offered by Robson
    DEFAULT_SYMBOLS = [
        {
            "name": "BTCUSDT",
            "base_asset": "BTC",
            "quote_asset": "USDT",
            "description": "Bitcoin to TetherUS"
        },
        {
            "name": "ETHUSDT",
            "base_asset": "ETH",
            "quote_asset": "USDT",
            "description": "Ethereum to TetherUS"
        },
        {
            "name": "BNBUSDT",
            "base_asset": "BNB",
            "quote_asset": "USDT",
            "description": "Binance Coin to TetherUS"
        },
        {
            "name": "SOLUSDT",
            "base_asset": "SOL",
            "quote_asset": "USDT",
            "description": "Solana to TetherUS"
        },
        {
            "name": "XRPUSDT",
            "base_asset": "XRP",
            "quote_asset": "USDT",
            "description": "Ripple to TetherUS"
        },
        {
            "name": "ADAUSDT",
            "base_asset": "ADA",
            "quote_asset": "USDT",
            "description": "Cardano to TetherUS"
        },
        {
            "name": "DOGEUSDT",
            "base_asset": "DOGE",
            "quote_asset": "USDT",
            "description": "Dogecoin to TetherUS"
        },
        {
            "name": "DOTUSDT",
            "base_asset": "DOT",
            "quote_asset": "USDT",
            "description": "Polkadot to TetherUS"
        }
    ]

    def handle(self, *args, **options):
        """Create or update global trading symbols."""

        self.stdout.write(self.style.HTTP_INFO('Creating global trading symbols...'))

        created_count = 0
        updated_count = 0

        for symbol_data in self.DEFAULT_SYMBOLS:
            symbol, created = Symbol.objects.get_or_create(
                name=symbol_data["name"],
                client=None,  # NULL = global (available to all users)
                defaults={
                    "base_asset": symbol_data["base_asset"],
                    "quote_asset": symbol_data["quote_asset"],
                    "description": symbol_data["description"],
                    "is_active": True
                }
            )

            if created:
                created_count += 1
                self.stdout.write(
                    self.style.SUCCESS(f'  ✓ Created: {symbol.name} ({symbol.base_asset}/{symbol.quote_asset})')
                )
            else:
                # Update existing global symbol with latest data
                symbol.base_asset = symbol_data["base_asset"]
                symbol.quote_asset = symbol_data["quote_asset"]
                symbol.description = symbol_data["description"]
                symbol.is_active = True
                symbol.save()
                updated_count += 1
                self.stdout.write(
                    self.style.HTTP_INFO(f'  ~ Updated: {symbol.name}')
                )

        # Show summary
        self.stdout.write('\n' + self.style.SUCCESS(
            f'Successfully processed {created_count + updated_count} global symbols:\n'
            f'  • {created_count} created\n'
            f'  • {updated_count} updated\n\n'
            f'Global symbols are now available to all users via GET /api/symbols/'
        ))

        # Show verification query
        self.stdout.write(self.style.HTTP_INFO(
            '\nVerify with:\n'
            '  python manage.py shell -c "from api.models import Symbol; '
            'print(list(Symbol.objects.filter(client__isnull=True).values_list(\'name\', flat=True)))"'
        ))
