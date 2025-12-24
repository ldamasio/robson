"""
Account Status Command.

Show current account balances, P&L, and risk status.

Usage:
    python manage.py account_status
    python manage.py account_status --capital 1000
"""

from decimal import Decimal
from datetime import datetime

from django.core.management.base import BaseCommand
from django.utils import timezone

from api.application.adapters import BinanceExecution, BinanceMarketData
from api.views.risk_managed_trading import _get_monthly_pnl


class Command(BaseCommand):
    help = 'Show account status, balances, and risk metrics'

    def add_arguments(self, parser):
        parser.add_argument(
            '--capital',
            type=str,
            help='Total capital for drawdown calculation (default: USDC balance)',
        )
        parser.add_argument(
            '--symbol',
            type=str,
            default='BTCUSDC',
            help='Trading pair for price info (default: BTCUSDC)',
        )

    def handle(self, *args, **options):
        self.stdout.write(self.style.HTTP_INFO('=' * 60))
        self.stdout.write(self.style.HTTP_INFO('ROBSON - Account Status'))
        self.stdout.write(self.style.HTTP_INFO('=' * 60))
        self.stdout.write('')

        # Initialize adapters
        try:
            execution = BinanceExecution()
            market_data = BinanceMarketData()
        except Exception as e:
            self.stdout.write(self.style.ERROR(f'Failed to initialize: {e}'))
            return

        env = 'PRODUCTION' if not execution.use_testnet else 'TESTNET'
        self.stdout.write(f'Environment: {env}')
        self.stdout.write(f'Timestamp: {timezone.now().isoformat()}')
        self.stdout.write('')

        # Get balances
        self.stdout.write(self.style.HTTP_INFO('--- Balances ---'))
        try:
            usdc = execution.get_account_balance('USDC')
            btc = execution.get_account_balance('BTC')
            
            usdc_free = Decimal(str(usdc.get('free', '0')))
            usdc_locked = Decimal(str(usdc.get('locked', '0')))
            btc_free = Decimal(str(btc.get('free', '0')))
            btc_locked = Decimal(str(btc.get('locked', '0')))
            
            self.stdout.write(f'USDC: {usdc_free} (locked: {usdc_locked})')
            self.stdout.write(f'BTC:  {btc_free} (locked: {btc_locked})')
        except Exception as e:
            self.stdout.write(self.style.ERROR(f'Failed to get balances: {e}'))
            return

        self.stdout.write('')

        # Get market data
        symbol = options['symbol']
        self.stdout.write(self.style.HTTP_INFO(f'--- Market ({symbol}) ---'))
        try:
            bid = market_data.best_bid(symbol)
            ask = market_data.best_ask(symbol)
            spread = ask - bid
            spread_pct = (spread / bid) * Decimal('100')
            
            self.stdout.write(f'Bid: ${bid}')
            self.stdout.write(f'Ask: ${ask}')
            self.stdout.write(f'Spread: ${spread} ({spread_pct:.4f}%)')
            
            # BTC value in USDC
            btc_value = btc_free * bid
            total_value = usdc_free + btc_value
            self.stdout.write(f'BTC Value: ${btc_value:.2f} USDC')
            self.stdout.write(f'Total Portfolio: ${total_value:.2f} USDC')
        except Exception as e:
            self.stdout.write(self.style.ERROR(f'Failed to get market data: {e}'))

        self.stdout.write('')

        # Get monthly P&L and risk status
        self.stdout.write(self.style.HTTP_INFO('--- Risk Status ---'))
        try:
            monthly_pnl = _get_monthly_pnl()
            
            # Capital for drawdown calculation
            if options.get('capital'):
                capital = Decimal(options['capital'])
            else:
                capital = usdc_free + btc_value if 'btc_value' in dir() else usdc_free
            
            # Calculate drawdown
            if monthly_pnl < 0:
                drawdown_pct = (abs(monthly_pnl) / capital) * Decimal('100')
            else:
                drawdown_pct = Decimal('0')
            
            max_drawdown = Decimal('4.0')
            remaining = max_drawdown - drawdown_pct
            is_trading_allowed = drawdown_pct < max_drawdown
            
            self.stdout.write(f'Monthly P&L: ${monthly_pnl}')
            self.stdout.write(f'Capital: ${capital:.2f}')
            self.stdout.write(f'Drawdown: {drawdown_pct:.2f}% (max: {max_drawdown}%)')
            self.stdout.write(f'Remaining: {remaining:.2f}%')
            
            if is_trading_allowed:
                self.stdout.write(self.style.SUCCESS('Trading: ALLOWED'))
            else:
                self.stdout.write(self.style.ERROR('Trading: PAUSED (drawdown limit reached)'))
        except Exception as e:
            self.stdout.write(self.style.WARNING(f'Could not calculate risk status: {e}'))

        self.stdout.write('')

        # Risk rules reminder
        self.stdout.write(self.style.HTTP_INFO('--- Risk Rules ---'))
        self.stdout.write('Max Risk Per Trade: 1% of capital')
        self.stdout.write('Max Monthly Drawdown: 4% of capital')
        self.stdout.write('Stop-Loss: REQUIRED for every trade')

        self.stdout.write('')
        self.stdout.write(self.style.HTTP_INFO('=' * 60))

