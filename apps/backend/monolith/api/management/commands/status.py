"""
Robson Status Command - Quick overview of account and positions.

Displays a compact, beautiful summary suitable for all terminals.

Usage:
    python manage.py status              # Quick overview
    python manage.py status --detailed   # Full details
"""

import logging
from decimal import Decimal

from django.core.management.base import BaseCommand
from django.utils import timezone

from api.application.adapters import BinanceExecution, BinanceMarketData
from api.models.margin import MarginPosition, MarginTransfer

logger = logging.getLogger(__name__)


class Command(BaseCommand):
    help = 'Display Robson account status and positions overview'

    def add_arguments(self, parser):
        parser.add_argument(
            '--detailed',
            action='store_true',
            help='Show detailed information',
        )
        parser.add_argument(
            '--client-id',
            type=int,
            default=1,
            help='Client ID (default: 1)',
        )

    def handle(self, *args, **options):
        detailed = options['detailed']
        client_id = options['client_id']
        
        try:
            execution = BinanceExecution()
            market_data = BinanceMarketData()
        except Exception as e:
            self.stdout.write(self.style.ERROR(f'Failed to connect to Binance: {e}'))
            return

        # Get account balances
        try:
            spot_usdc = execution.get_account_balance('USDC')
            spot_btc = execution.get_account_balance('BTC')
            btc_price = market_data.best_bid('BTCUSDC')
        except Exception as e:
            self.stdout.write(self.style.ERROR(f'Failed to get balances: {e}'))
            return

        spot_usdc_free = Decimal(spot_usdc.get('free', '0'))
        spot_btc_free = Decimal(spot_btc.get('free', '0'))
        spot_btc_value = spot_btc_free * btc_price

        # Get open positions
        open_positions = MarginPosition.objects.filter(
            client_id=client_id,
            status=MarginPosition.Status.OPEN
        )

        # Get margin account info
        margin_data = {}
        total_margin_btc = Decimal('0')
        total_margin_usdc_borrowed = Decimal('0')
        
        symbols = set(p.symbol for p in open_positions) or {'BTCUSDC'}
        
        for sym in symbols:
            try:
                result = execution.client.get_isolated_margin_account(symbols=sym)
                assets = result.get('assets', [])
                if assets:
                    base = assets[0].get('baseAsset', {})
                    quote = assets[0].get('quoteAsset', {})
                    margin_data[sym] = {
                        'btc_free': Decimal(base.get('free', '0')),
                        'btc_net': Decimal(base.get('netAsset', '0')),
                        'usdc_borrowed': Decimal(quote.get('borrowed', '0')),
                        'margin_level': Decimal(assets[0].get('marginLevel', '999')),
                        'index_price': Decimal(assets[0].get('indexPrice', '0')),
                    }
                    total_margin_btc += Decimal(base.get('netAsset', '0'))
                    total_margin_usdc_borrowed += Decimal(quote.get('borrowed', '0'))
            except Exception as e:
                logger.warning(f"Could not get margin info for {sym}: {e}")

        total_margin_value = total_margin_btc * btc_price

        # Calculate total equity
        total_equity = spot_usdc_free + spot_btc_value + total_margin_value - total_margin_usdc_borrowed

        # Calculate total P&L from open positions
        total_pnl = Decimal('0')
        for pos in open_positions:
            current_price = margin_data.get(pos.symbol, {}).get('index_price', pos.current_price)
            if pos.side == MarginPosition.Side.LONG:
                pnl = (current_price - pos.entry_price) * pos.quantity
            else:
                pnl = (pos.entry_price - current_price) * pos.quantity
            total_pnl += pnl

        # Print the status
        self._print_header()
        self._print_balances(spot_usdc_free, spot_btc_free, spot_btc_value, btc_price)
        self._print_margin_summary(total_margin_btc, total_margin_value, total_margin_usdc_borrowed, margin_data)
        self._print_positions_summary(open_positions, total_pnl, margin_data)
        self._print_equity(total_equity)
        self._print_footer()

        if detailed:
            self._print_detailed_positions(open_positions, margin_data)

    def _print_header(self):
        now = timezone.now().strftime('%Y-%m-%d %H:%M UTC')
        self.stdout.write('')
        self.stdout.write('+' + '-' * 58 + '+')
        self.stdout.write('|' + ' ROBSON BOT - Account Status '.center(58) + '|')
        self.stdout.write('|' + f' {now} '.center(58) + '|')
        self.stdout.write('+' + '-' * 58 + '+')

    def _print_balances(self, usdc, btc, btc_value, btc_price):
        self.stdout.write('|' + ' SPOT BALANCES '.center(58, '-') + '|')
        self.stdout.write(f"|  USDC: ${usdc:>15.2f}".ljust(59) + '|')
        self.stdout.write(f"|  BTC:  {btc:>15.8f} (~${btc_value:.2f})".ljust(59) + '|')
        self.stdout.write(f"|  BTC Price: ${btc_price:,.2f}".ljust(59) + '|')

    def _print_margin_summary(self, margin_btc, margin_value, borrowed, margin_data):
        self.stdout.write('|' + ' ISOLATED MARGIN '.center(58, '-') + '|')
        
        if not margin_data:
            self.stdout.write('|  No margin accounts active'.ljust(59) + '|')
            return
            
        self.stdout.write(f"|  BTC in Margin: {margin_btc:.8f} (~${margin_value:.2f})".ljust(59) + '|')
        self.stdout.write(f"|  USDC Borrowed: ${borrowed:.2f}".ljust(59) + '|')
        
        for sym, data in margin_data.items():
            level = data['margin_level']
            status = self._margin_status(level)
            self.stdout.write(f"|  {sym}: Level {level:.2f}x ({status})".ljust(59) + '|')

    def _margin_status(self, level: Decimal) -> str:
        if level >= Decimal('2.0'):
            return 'SAFE'
        elif level >= Decimal('1.5'):
            return 'CAUTION'
        elif level >= Decimal('1.3'):
            return 'WARNING'
        elif level >= Decimal('1.1'):
            return 'CRITICAL'
        else:
            return 'DANGER'

    def _print_positions_summary(self, positions, total_pnl, margin_data):
        self.stdout.write('|' + ' OPEN POSITIONS '.center(58, '-') + '|')
        
        count = positions.count()
        if count == 0:
            self.stdout.write('|  No open positions'.ljust(59) + '|')
            return
        
        self.stdout.write(f"|  Count: {count}".ljust(59) + '|')
        
        for pos in positions:
            side_str = 'L' if pos.side == MarginPosition.Side.LONG else 'S'
            current_price = margin_data.get(pos.symbol, {}).get('index_price', pos.current_price)
            
            if pos.side == MarginPosition.Side.LONG:
                pnl = (current_price - pos.entry_price) * pos.quantity
            else:
                pnl = (pos.entry_price - current_price) * pos.quantity
            
            pnl_sign = '+' if pnl >= 0 else ''
            line = f"|  [{side_str}] {pos.symbol} {pos.quantity:.5f} @ ${pos.entry_price:.2f} | P&L: {pnl_sign}${pnl:.2f}"
            self.stdout.write(line.ljust(59) + '|')
        
        pnl_sign = '+' if total_pnl >= 0 else ''
        self.stdout.write(f"|  Total P&L: {pnl_sign}${total_pnl:.2f}".ljust(59) + '|')

    def _print_equity(self, total_equity):
        self.stdout.write('|' + ' TOTAL EQUITY '.center(58, '-') + '|')
        self.stdout.write(f"|  ${total_equity:,.2f}".ljust(59) + '|')

    def _print_footer(self):
        self.stdout.write('+' + '-' * 58 + '+')
        self.stdout.write('')

    def _print_detailed_positions(self, positions, margin_data):
        """Print detailed position cards."""
        if not positions:
            return
            
        self.stdout.write('')
        self.stdout.write('=' * 60)
        self.stdout.write(' DETAILED POSITIONS '.center(60))
        self.stdout.write('=' * 60)
        
        for pos in positions:
            current_price = margin_data.get(pos.symbol, {}).get('index_price', pos.current_price)
            margin_level = margin_data.get(pos.symbol, {}).get('margin_level', pos.margin_level)
            
            if pos.side == MarginPosition.Side.LONG:
                pnl = (current_price - pos.entry_price) * pos.quantity
                pnl_pct = ((current_price / pos.entry_price) - 1) * 100
            else:
                pnl = (pos.entry_price - current_price) * pos.quantity
                pnl_pct = ((pos.entry_price / current_price) - 1) * 100
            
            pnl_sign = '+' if pnl >= 0 else ''
            status = self._margin_status(margin_level)
            
            self.stdout.write('')
            self.stdout.write(f'  Position #{pos.id}: {pos.symbol} {pos.side}')
            self.stdout.write(f'  ' + '-' * 40)
            self.stdout.write(f'  Entry:      ${pos.entry_price}')
            self.stdout.write(f'  Current:    ${current_price}')
            self.stdout.write(f'  Stop:       ${pos.stop_price}')
            self.stdout.write(f'  Quantity:   {pos.quantity} BTC')
            self.stdout.write(f'  Leverage:   {pos.leverage}x')
            self.stdout.write(f'  Margin:     {margin_level:.2f}x ({status})')
            self.stdout.write(f'  Risk:       ${pos.risk_amount} ({pos.risk_percent}%)')
            self.stdout.write(f'  P&L:        {pnl_sign}${pnl:.2f} ({pnl_sign}{pnl_pct:.2f}%)')
            self.stdout.write(f'  Entry Order: {pos.binance_entry_order_id}')
            self.stdout.write(f'  Stop Order:  {pos.binance_stop_order_id}')
        
        self.stdout.write('')
        self.stdout.write('=' * 60)

