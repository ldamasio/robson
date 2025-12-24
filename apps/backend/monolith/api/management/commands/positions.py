"""
Positions Command - Display active margin positions.

Shows a beautiful, UX-friendly summary of all open positions
with real-time P&L and risk metrics.

Usage:
    python manage.py positions              # All open positions
    python manage.py positions --all        # Include closed
    python manage.py positions --symbol BTCUSDC
    python manage.py positions --json       # JSON output for scripts
"""

import json
import logging
from decimal import Decimal

from django.core.management.base import BaseCommand

from api.application.adapters import BinanceExecution, BinanceMarketData
from api.models.margin import MarginPosition, MarginTransfer

logger = logging.getLogger(__name__)


class Command(BaseCommand):
    help = 'Display active margin positions with real-time data'

    def add_arguments(self, parser):
        parser.add_argument(
            '--all',
            action='store_true',
            help='Show all positions including closed',
        )
        parser.add_argument(
            '--symbol',
            type=str,
            help='Filter by symbol (e.g., BTCUSDC)',
        )
        parser.add_argument(
            '--client-id',
            type=int,
            default=1,
            help='Client ID (default: 1)',
        )
        parser.add_argument(
            '--json',
            action='store_true',
            help='Output as JSON for scripts',
        )
        parser.add_argument(
            '--live',
            action='store_true',
            help='Fetch live prices from Binance',
        )

    def handle(self, *args, **options):
        show_all = options['all']
        symbol_filter = options.get('symbol')
        client_id = options['client_id']
        as_json = options['json']
        fetch_live = options['live']

        # Query positions
        queryset = MarginPosition.objects.filter(client_id=client_id)
        
        if not show_all:
            queryset = queryset.filter(status=MarginPosition.Status.OPEN)
        
        if symbol_filter:
            queryset = queryset.filter(symbol=symbol_filter)
        
        positions = list(queryset.order_by('-created_at'))

        if not positions:
            if as_json:
                self.stdout.write(json.dumps({'positions': [], 'count': 0}))
            else:
                self.stdout.write(self.style.WARNING('No positions found.'))
            return

        # Fetch live data if requested
        live_prices = {}
        margin_info = {}
        
        if fetch_live:
            try:
                execution = BinanceExecution()
                market_data = BinanceMarketData()
                
                symbols = set(p.symbol for p in positions)
                for sym in symbols:
                    live_prices[sym] = market_data.best_bid(sym)
                    
                    # Get margin account info
                    result = execution.client.get_isolated_margin_account(symbols=sym)
                    assets = result.get('assets', [])
                    if assets:
                        margin_info[sym] = {
                            'margin_level': Decimal(assets[0].get('marginLevel', '999')),
                            'liquidation_price': Decimal(assets[0].get('liquidatePrice', '0')),
                            'base_free': Decimal(assets[0].get('baseAsset', {}).get('free', '0')),
                            'base_borrowed': Decimal(assets[0].get('baseAsset', {}).get('borrowed', '0')),
                            'quote_borrowed': Decimal(assets[0].get('quoteAsset', {}).get('borrowed', '0')),
                        }
            except Exception as e:
                logger.warning(f"Could not fetch live data: {e}")

        if as_json:
            self._output_json(positions, live_prices, margin_info)
        else:
            self._output_table(positions, live_prices, margin_info)

    def _output_json(self, positions, live_prices, margin_info):
        """Output positions as JSON."""
        data = {
            'positions': [],
            'count': len(positions),
        }
        
        for pos in positions:
            current_price = live_prices.get(pos.symbol, pos.current_price)
            
            # Calculate P&L
            if pos.side == MarginPosition.Side.LONG:
                pnl = (current_price - pos.entry_price) * pos.quantity
            else:
                pnl = (pos.entry_price - current_price) * pos.quantity
            
            pos_data = {
                'id': pos.id,
                'position_id': pos.position_id,
                'symbol': pos.symbol,
                'side': pos.side,
                'status': pos.status,
                'entry_price': str(pos.entry_price),
                'stop_price': str(pos.stop_price),
                'current_price': str(current_price),
                'quantity': str(pos.quantity),
                'leverage': pos.leverage,
                'risk_amount': str(pos.risk_amount),
                'risk_percent': str(pos.risk_percent),
                'unrealized_pnl': str(pnl),
                'margin_level': str(margin_info.get(pos.symbol, {}).get('margin_level', pos.margin_level)),
                'opened_at': pos.opened_at.isoformat() if pos.opened_at else None,
            }
            data['positions'].append(pos_data)
        
        self.stdout.write(json.dumps(data, indent=2))

    def _output_table(self, positions, live_prices, margin_info):
        """Output positions as beautiful ASCII table."""
        
        for pos in positions:
            current_price = live_prices.get(pos.symbol, pos.current_price)
            info = margin_info.get(pos.symbol, {})
            margin_level = info.get('margin_level', pos.margin_level)
            
            # Calculate P&L
            if pos.side == MarginPosition.Side.LONG:
                pnl = (current_price - pos.entry_price) * pos.quantity
                pnl_percent = ((current_price / pos.entry_price) - 1) * 100
            else:
                pnl = (pos.entry_price - current_price) * pos.quantity
                pnl_percent = ((pos.entry_price / current_price) - 1) * 100

            position_value = pos.quantity * current_price
            
            # Get transfer count
            transfer_count = MarginTransfer.objects.filter(position=pos).count()
            
            # Determine margin health
            margin_status = self._get_margin_status(margin_level)
            
            # P&L styling
            if pnl >= 0:
                pnl_str = f"+${pnl:.2f} (+{pnl_percent:.2f}%)"
                pnl_style = self.style.SUCCESS
            else:
                pnl_str = f"-${abs(pnl):.2f} ({pnl_percent:.2f}%)"
                pnl_style = self.style.ERROR
            
            # Build the display
            self._print_position_card(
                pos=pos,
                current_price=current_price,
                pnl_str=pnl_str,
                pnl_style=pnl_style,
                position_value=position_value,
                margin_level=margin_level,
                margin_status=margin_status,
                transfer_count=transfer_count,
            )

    def _get_margin_status(self, margin_level: Decimal) -> tuple:
        """Get margin status text and style."""
        if margin_level >= Decimal('2.0'):
            return ('SAFE', self.style.SUCCESS)
        elif margin_level >= Decimal('1.5'):
            return ('CAUTION', self.style.WARNING)
        elif margin_level >= Decimal('1.3'):
            return ('WARNING', self.style.ERROR)
        elif margin_level >= Decimal('1.1'):
            return ('CRITICAL', self.style.ERROR)
        else:
            return ('DANGER', self.style.ERROR)

    def _print_position_card(self, pos, current_price, pnl_str, pnl_style, 
                              position_value, margin_level, margin_status, 
                              transfer_count):
        """Print a single position card."""
        
        status_text, status_style = margin_status
        
        # Header
        side_emoji = "LONG" if pos.side == MarginPosition.Side.LONG else "SHORT"
        title = f"  {pos.symbol} {side_emoji}"
        
        # Calculate widths
        w = 58  # inner width
        
        # Border characters (works on all terminals)
        h_line = '-' * (w + 2)
        
        self.stdout.write('')
        self.stdout.write(f"+{h_line}+")
        self.stdout.write(f"|  POSITION #{pos.id} - {pos.symbol} {side_emoji}".ljust(w + 2) + " |")
        self.stdout.write(f"|  Status: {pos.status}".ljust(w + 2) + " |")
        self.stdout.write(f"+{h_line}+")
        
        # Prices section
        self.stdout.write(f"|  Entry:   ${pos.entry_price:>12}".ljust(w + 2) + " |")
        self.stdout.write(f"|  Current: ${current_price:>12}".ljust(w + 2) + " |")
        self.stdout.write(f"|  Stop:    ${pos.stop_price:>12}".ljust(w + 2) + " |")
        if pos.target_price:
            self.stdout.write(f"|  Target:  ${pos.target_price:>12}".ljust(w + 2) + " |")
        
        self.stdout.write(f"|{' ' * (w + 2)}|")
        
        # Position details
        self.stdout.write(f"|  Quantity: {pos.quantity} BTC".ljust(w + 2) + " |")
        self.stdout.write(f"|  Value:    ${position_value:.2f}".ljust(w + 2) + " |")
        self.stdout.write(f"|  Leverage: {pos.leverage}x".ljust(w + 2) + " |")
        
        self.stdout.write(f"|{' ' * (w + 2)}|")
        
        # Risk section
        self.stdout.write(f"|  Risk Amount:  ${pos.risk_amount}".ljust(w + 2) + " |")
        self.stdout.write(f"|  Risk Percent: {pos.risk_percent}%".ljust(w + 2) + " |")
        self.stdout.write(f"|  Margin Level: {margin_level:.2f}x ({status_text})".ljust(w + 2) + " |")
        
        self.stdout.write(f"|{' ' * (w + 2)}|")
        
        # P&L section
        self.stdout.write(f"|  P&L: {pnl_str}".ljust(w + 2) + " |")
        
        self.stdout.write(f"|{' ' * (w + 2)}|")
        
        # Binance references
        self.stdout.write(f"|  Entry Order:  {pos.binance_entry_order_id or 'N/A'}".ljust(w + 2) + " |")
        self.stdout.write(f"|  Stop Order:   {pos.binance_stop_order_id or 'N/A'}".ljust(w + 2) + " |")
        
        self.stdout.write(f"|{' ' * (w + 2)}|")
        
        # Audit info
        self.stdout.write(f"|  DB ID: {pos.id}".ljust(w + 2) + " |")
        self.stdout.write(f"|  Transfers: {transfer_count}".ljust(w + 2) + " |")
        if pos.opened_at:
            opened_str = pos.opened_at.strftime('%Y-%m-%d %H:%M UTC')
            self.stdout.write(f"|  Opened: {opened_str}".ljust(w + 2) + " |")
        
        self.stdout.write(f"+{h_line}+")
        self.stdout.write('')

