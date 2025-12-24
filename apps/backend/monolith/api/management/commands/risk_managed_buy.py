"""
Risk-Managed Buy Command.

Execute a BUY order with mandatory risk management enforcement.

This command:
1. Validates all risk parameters (stop-loss, 1% rule, 4% drawdown)
2. Calculates safe position size if not provided
3. Shows order preview before execution
4. Executes only in LIVE mode with explicit confirmation

Usage:
    # Dry-run (default - no real order)
    python manage.py risk_managed_buy --capital 100 --stop-percent 1

    # Live execution (requires --live and --confirm)
    python manage.py risk_managed_buy --capital 100 --stop-percent 1 --live --confirm

Examples:
    # Calculate order for $100 capital with 1% stop
    python manage.py risk_managed_buy --capital 100 --stop-percent 1

    # Execute real order
    python manage.py risk_managed_buy --capital 100 --stop-percent 1 --live --confirm

    # Use specific stop price instead of percentage
    python manage.py risk_managed_buy --capital 100 --stop-price 94000 --live --confirm
"""

import logging
from decimal import Decimal, InvalidOperation

from django.core.management.base import BaseCommand, CommandError
from django.db import transaction
from django.utils import timezone

from api.application.adapters import BinanceExecution, BinanceMarketData
from api.application.execution import ExecutionMode
from api.application.risk_managed_trade import RiskManagedTradeUseCase
from api.models import Order, Symbol, Trade
from api.views.risk_managed_trading import DjangoPnLRepository

logger = logging.getLogger(__name__)


def _get_or_create_symbol(symbol_str: str) -> Symbol:
    """Get or create a Symbol from a symbol string like 'BTCUSDC'."""
    symbol_obj, _ = Symbol.objects.get_or_create(
        name=symbol_str,
        defaults={'base': symbol_str[:-4], 'quote': symbol_str[-4:]}
    )
    return symbol_obj


class Command(BaseCommand):
    help = 'Execute a risk-managed BUY order with mandatory stop-loss'

    def add_arguments(self, parser):
        # Required
        parser.add_argument(
            '--capital',
            type=str,
            required=True,
            help='Total capital to consider for risk calculation (in USDC)',
        )

        # Stop-loss (one of these is required)
        parser.add_argument(
            '--stop-price',
            type=str,
            help='Exact stop-loss price',
        )
        parser.add_argument(
            '--stop-percent',
            type=str,
            default='1',
            help='Stop-loss as percentage below entry (default: 1%%)',
        )

        # Optional
        parser.add_argument(
            '--symbol',
            type=str,
            default='BTCUSDC',
            help='Trading pair (default: BTCUSDC)',
        )
        parser.add_argument(
            '--quantity',
            type=str,
            help='Position size (calculated automatically if not provided)',
        )
        parser.add_argument(
            '--strategy',
            type=str,
            default='manual',
            help='Strategy name for tracking (default: manual)',
        )

        # Execution mode
        parser.add_argument(
            '--live',
            action='store_true',
            help='Execute REAL order (default is dry-run)',
        )
        parser.add_argument(
            '--confirm',
            action='store_true',
            help='Confirm risk acknowledgement for live execution',
        )

    def handle(self, *args, **options):
        self.stdout.write(self.style.HTTP_INFO('=' * 60))
        self.stdout.write(self.style.HTTP_INFO('ROBSON - Risk-Managed Buy Order'))
        self.stdout.write(self.style.HTTP_INFO('=' * 60))
        self.stdout.write('')

        # Parse parameters
        try:
            capital = Decimal(options['capital'])
        except (InvalidOperation, ValueError):
            raise CommandError(f"Invalid capital: {options['capital']}")

        symbol = options['symbol']
        strategy = options['strategy']

        # Determine execution mode
        is_live = options['live']
        is_confirmed = options['confirm']

        if is_live and not is_confirmed:
            raise CommandError(
                'LIVE mode requires --confirm flag.\n'
                'Add --confirm to acknowledge you understand this will place a REAL order.'
            )

        mode = ExecutionMode.LIVE if is_live else ExecutionMode.DRY_RUN

        # Initialize adapters
        execution = BinanceExecution()
        market_data = BinanceMarketData()
        pnl_repo = DjangoPnLRepository()

        env = 'PRODUCTION' if not execution.use_testnet else 'TESTNET'
        self.stdout.write(f'Environment: {env}')
        self.stdout.write(f'Mode: {mode.value}')
        self.stdout.write('')

        # Get current market data
        self.stdout.write(self.style.HTTP_INFO('--- Market Data ---'))
        try:
            usdc_balance = execution.get_account_balance('USDC')
            btc_balance = execution.get_account_balance('BTC')
            entry_price = market_data.best_ask(symbol)
        except Exception as e:
            raise CommandError(f'Failed to get market data: {e}')

        self.stdout.write(f'USDC Balance: {usdc_balance.get("free")}')
        self.stdout.write(f'BTC Balance: {btc_balance.get("free")}')
        self.stdout.write(f'Current {symbol}: {entry_price}')
        self.stdout.write('')

        # Calculate stop price
        if options.get('stop_price'):
            try:
                stop_price = Decimal(options['stop_price'])
            except (InvalidOperation, ValueError):
                raise CommandError(f"Invalid stop price: {options['stop_price']}")
        else:
            try:
                stop_percent = Decimal(options['stop_percent'])
                stop_price = entry_price * (Decimal('1') - stop_percent / Decimal('100'))
            except (InvalidOperation, ValueError):
                raise CommandError(f"Invalid stop percent: {options['stop_percent']}")

        # Get quantity if provided
        quantity = None
        if options.get('quantity'):
            try:
                quantity = Decimal(options['quantity'])
            except (InvalidOperation, ValueError):
                raise CommandError(f"Invalid quantity: {options['quantity']}")

        # Get monthly P&L
        monthly_pnl = pnl_repo.get_monthly_pnl()

        # Create use case
        use_case = RiskManagedTradeUseCase(
            execution_adapter=execution,
            pnl_repository=pnl_repo,
        )

        # Calculate safe quantity if not provided
        if quantity is None:
            quantity = use_case.calculate_position_size(capital, entry_price, stop_price)

        # Calculate risk metrics
        stop_distance = abs(entry_price - stop_price)
        risk_amount = stop_distance * quantity
        risk_percent = (risk_amount / capital) * Decimal('100')
        position_value = quantity * entry_price

        # Show order preview
        self.stdout.write(self.style.HTTP_INFO('--- Order Preview ---'))
        self.stdout.write(f'Symbol: {symbol}')
        self.stdout.write(f'Side: BUY')
        self.stdout.write(f'Entry Price: ${entry_price}')
        self.stdout.write(f'Stop Price: ${stop_price}')
        self.stdout.write(f'Stop Distance: ${stop_distance} ({stop_distance/entry_price*100:.2f}%)')
        self.stdout.write(f'Quantity: {quantity} BTC')
        self.stdout.write(f'Position Value: ${position_value:.2f}')
        self.stdout.write(f'Risk Amount: ${risk_amount:.2f}')
        self.stdout.write(f'Risk Percent: {risk_percent:.2f}%')
        self.stdout.write(f'Capital: ${capital}')
        self.stdout.write(f'Monthly P&L: ${monthly_pnl}')
        self.stdout.write(f'Strategy: {strategy}')
        self.stdout.write('')

        # Validate first
        self.stdout.write(self.style.HTTP_INFO('--- Risk Validation ---'))
        validation = use_case.validate(
            symbol=symbol,
            side='BUY',
            quantity=quantity,
            entry_price=entry_price,
            stop_price=stop_price,
            capital=capital,
            monthly_pnl=monthly_pnl,
        )

        for guard in validation.guards:
            if guard.passed:
                self.stdout.write(self.style.SUCCESS(f'  {guard}'))
            else:
                self.stdout.write(self.style.ERROR(f'  {guard}'))

        self.stdout.write('')

        if not validation.is_valid:
            self.stdout.write(self.style.ERROR(f'BLOCKED: {validation.blocked_by}'))
            self.stdout.write(self.style.WARNING(validation.message))
            return

        self.stdout.write(self.style.SUCCESS('All risk checks PASSED'))
        self.stdout.write('')

        # Execute
        self.stdout.write(self.style.HTTP_INFO('--- Execution ---'))

        result = use_case.execute_buy(
            symbol=symbol,
            capital=capital,
            entry_price=entry_price,
            stop_price=stop_price,
            quantity=quantity,
            mode=mode,
            monthly_pnl=monthly_pnl,
            strategy_name=strategy,
        )

        # Show result
        if result.is_success():
            self.stdout.write(self.style.SUCCESS(f'Status: {result.status.value}'))
        elif result.is_blocked():
            self.stdout.write(self.style.ERROR(f'Status: {result.status.value}'))
        else:
            self.stdout.write(self.style.WARNING(f'Status: {result.status.value}'))

        for action in result.actions:
            action_type = action.get('type', 'unknown')
            description = action.get('description', '')
            self.stdout.write(f'  [{action_type}] {description}')
            if action.get('result'):
                self.stdout.write(f'    Result: {action["result"]}')

        self.stdout.write('')

        # AUDIT: Record trade in database if LIVE and successful
        if mode == ExecutionMode.LIVE and result.is_success():
            self.stdout.write(self.style.HTTP_INFO('--- Recording to Database (Audit) ---'))
            try:
                with transaction.atomic():
                    symbol_obj = _get_or_create_symbol(symbol)
                    order_data = result.metadata.get('order', {})
                    
                    # Create Trade record
                    trade = Trade.objects.create(
                        symbol=symbol_obj,
                        side='BUY',
                        quantity=quantity,
                        entry_price=entry_price,
                        entry_time=timezone.now(),
                    )
                    
                    # Create Order record
                    order = Order.objects.create(
                        symbol=symbol_obj,
                        binance_order_id=order_data.get('orderId'),
                        side='BUY',
                        order_type='MARKET',
                        quantity=quantity,
                        avg_fill_price=Decimal(str(order_data.get('avgFillPrice', entry_price))),
                        status='FILLED',
                    )
                    
                    self.stdout.write(self.style.SUCCESS(f'  Trade ID: {trade.id}'))
                    self.stdout.write(self.style.SUCCESS(f'  Order ID: {order.id}'))
                    self.stdout.write(self.style.SUCCESS(f'  Binance Order: {order.binance_order_id}'))
                    
            except Exception as e:
                self.stdout.write(self.style.ERROR(f'  Failed to save to DB: {e}'))
                logger.error(f"Failed to save trade to database: {e}", exc_info=True)

        if mode == ExecutionMode.DRY_RUN:
            self.stdout.write(self.style.WARNING(
                'This was a DRY-RUN. No real order was placed.\n'
                'To execute a real order, add: --live --confirm'
            ))
        else:
            self.stdout.write(self.style.SUCCESS('LIVE order executed and recorded!'))

        self.stdout.write('')
        self.stdout.write(self.style.HTTP_INFO('=' * 60))

