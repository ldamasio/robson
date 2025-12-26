"""
Django Management Command: Portfolio BTC

This command shows portfolio value denominated in BTC.

Usage:
    python manage.py portfolio_btc                          # Show current portfolio in BTC
    python manage.py portfolio_btc --profit                 # Show profit in BTC
    python manage.py portfolio_btc --since 2025-01-01       # Calculate profit since date
"""

from django.core.management.base import BaseCommand
from decimal import Decimal
from datetime import datetime
from clients.models import Client
from api.services.portfolio_btc_service import PortfolioBTCService


class Command(BaseCommand):
    help = 'Show portfolio value in BTC'

    def add_arguments(self, parser):
        parser.add_argument(
            '--client-id',
            type=int,
            default=1,
            help='Client ID (default: 1)',
        )
        parser.add_argument(
            '--profit',
            action='store_true',
            help='Calculate profit in BTC',
        )
        parser.add_argument(
            '--since',
            type=str,
            help='Start date for profit calculation (YYYY-MM-DD)',
        )

    def handle(self, *args, **options):
        client_id = options['client_id']
        client = Client.objects.get(id=client_id)

        service = PortfolioBTCService(client)

        if options['profit']:
            # Show profit
            since = None
            if options['since']:
                try:
                    since = datetime.strptime(options['since'], '%Y-%m-%d')
                except ValueError:
                    self.stdout.write(self.style.ERROR("Invalid date format. Use YYYY-MM-DD"))
                    return

            profit = service.calculate_profit_btc(since=since)

            profit_color = self.style.SUCCESS if profit['profit_btc'] >= 0 else self.style.ERROR

            self.stdout.write(profit_color(f"""
Portfolio Profit (BTC)
{'=' * 50}
Current Balance:  {profit['current_balance_btc']} BTC
Total Deposits:   {profit['total_deposits_btc']} BTC
Total Withdrawals: {profit['total_withdrawals_btc']} BTC
Net Inflows:      {profit['net_inflows_btc']} BTC

PROFIT:           {profit['profit_btc']} BTC
PROFIT %:         {profit['profit_percent']}%

Since: {profit['start_date']}
Calculated at: {profit['calculated_at']}
            """))
        else:
            # Show current portfolio
            portfolio = service.calculate_total_portfolio_btc()

            self.stdout.write(self.style.SUCCESS(f"""
Portfolio Value (BTC)
{'=' * 50}
Total:  {portfolio['total_btc']} BTC
Spot:   {portfolio['spot_btc']} BTC
Margin: {portfolio['margin_btc']} BTC
Debt:   {portfolio['margin_debt_btc']} BTC

Breakdown by Asset:
"""))

            for asset, btc_value in portfolio['breakdown'].items():
                self.stdout.write(f"  {asset}: {btc_value} BTC")
