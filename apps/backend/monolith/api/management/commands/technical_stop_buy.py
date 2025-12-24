"""
Technical Stop Buy Command.

Execute a BUY order with stop-loss calculated from TECHNICAL ANALYSIS,
not arbitrary percentages.

The technical stop is placed at:
- The Nth support level on the specified timeframe (default: 2nd support on 15m)
- Falls back to swing points or ATR if no clear levels found

This is how professional traders manage risk:
1. Identify technical invalidation level (where trade thesis is wrong)
2. Calculate position size backwards from that level
3. Risk exactly 1% of capital

Usage:
    # Analyze and show technical stop (dry-run)
    python manage.py technical_stop_buy --capital 100

    # Use 4h timeframe instead of 15m
    python manage.py technical_stop_buy --capital 100 --timeframe 4h

    # Execute with technical stop
    python manage.py technical_stop_buy --capital 100 --live --confirm

Examples:
    # Show technical analysis for $1000 capital
    python manage.py technical_stop_buy --capital 1000 --symbol BTCUSDC

    # Execute real order with technical stop
    python manage.py technical_stop_buy --capital 1000 --live --confirm
"""

import logging
from decimal import Decimal, InvalidOperation

from django.core.management.base import BaseCommand, CommandError

from api.application.adapters import BinanceExecution
from api.application.technical_stop_adapter import BinanceTechnicalStopService
from api.application.execution import ExecutionMode
from api.application.risk_managed_trade import RiskManagedTradeUseCase
from api.views.risk_managed_trading import DjangoPnLRepository

logger = logging.getLogger(__name__)


class Command(BaseCommand):
    help = 'Execute a BUY order with TECHNICAL stop-loss (support/resistance based)'

    def add_arguments(self, parser):
        # Required
        parser.add_argument(
            '--capital',
            type=str,
            required=True,
            help='Total capital for risk calculation (in USDC)',
        )

        # Optional - Technical Analysis
        parser.add_argument(
            '--symbol',
            type=str,
            default='BTCUSDC',
            help='Trading pair (default: BTCUSDC)',
        )
        parser.add_argument(
            '--timeframe',
            type=str,
            default='15m',
            choices=['1m', '5m', '15m', '30m', '1h', '4h', '1d'],
            help='Chart timeframe for technical analysis (default: 15m)',
        )
        parser.add_argument(
            '--level-n',
            type=int,
            default=2,
            help='Which support level to use: 1=closest, 2=second (default: 2)',
        )
        parser.add_argument(
            '--strategy',
            type=str,
            default='technical',
            help='Strategy name for tracking (default: technical)',
        )

        # Execution mode
        parser.add_argument(
            '--live',
            action='store_true',
            help='Execute REAL order (default is dry-run/analysis only)',
        )
        parser.add_argument(
            '--confirm',
            action='store_true',
            help='Confirm risk acknowledgement for live execution',
        )
        
        # Show levels only
        parser.add_argument(
            '--levels-only',
            action='store_true',
            help='Only show support/resistance levels, no order',
        )

    def handle(self, *args, **options):
        self.stdout.write(self.style.HTTP_INFO('=' * 60))
        self.stdout.write(self.style.HTTP_INFO('ROBSON - Technical Stop Buy Order'))
        self.stdout.write(self.style.HTTP_INFO('=' * 60))
        self.stdout.write('')

        # Parse parameters
        try:
            capital = Decimal(options['capital'])
        except (InvalidOperation, ValueError):
            raise CommandError(f"Invalid capital: {options['capital']}")

        symbol = options['symbol']
        timeframe = options['timeframe']
        level_n = options['level_n']
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

        # Initialize services
        tech_service = BinanceTechnicalStopService(
            level_n=level_n,
            default_timeframe=timeframe,
        )
        execution = BinanceExecution()
        pnl_repo = DjangoPnLRepository()

        env = 'PRODUCTION' if not execution.use_testnet else 'TESTNET'
        self.stdout.write(f'Environment: {env}')
        self.stdout.write(f'Mode: {mode.value}')
        self.stdout.write(f'Symbol: {symbol}')
        self.stdout.write(f'Timeframe: {timeframe}')
        self.stdout.write(f'Using: {level_n}{"st" if level_n == 1 else "nd" if level_n == 2 else "rd" if level_n == 3 else "th"} support level')
        self.stdout.write('')

        # Get account balances
        self.stdout.write(self.style.HTTP_INFO('--- Account ---'))
        try:
            usdc_balance = execution.get_account_balance('USDC')
            btc_balance = execution.get_account_balance('BTC')
            self.stdout.write(f'USDC: {usdc_balance.get("free")}')
            self.stdout.write(f'BTC:  {btc_balance.get("free")}')
        except Exception as e:
            raise CommandError(f'Failed to get balances: {e}')
        self.stdout.write('')

        # If levels-only mode, just show levels and exit
        if options['levels_only']:
            self._show_levels(tech_service, symbol, timeframe)
            return

        # Calculate technical stop and position size
        self.stdout.write(self.style.HTTP_INFO('--- Technical Analysis ---'))
        try:
            result = tech_service.calculate_position_with_technical_stop(
                symbol=symbol,
                side="BUY",
                capital=capital,
                timeframe=timeframe,
            )
        except Exception as e:
            raise CommandError(f'Technical analysis failed: {e}')

        stop_result = result['stop_result']

        # Show analysis results
        self.stdout.write(f'Entry Price: ${result["entry_price"]}')
        self.stdout.write(f'Technical Stop: ${result["stop_price"]}')
        self.stdout.write(f'Stop Distance: ${result["stop_distance"]} ({result["stop_distance_pct"]:.2f}%)')
        self.stdout.write(f'Method: {result["method_used"]}')
        self.stdout.write(f'Confidence: {result["confidence"]}')
        self.stdout.write(f'Levels Found: {result["levels_found"]}')
        
        if stop_result.selected_level:
            lvl = stop_result.selected_level
            self.stdout.write(f'Selected Level: ${lvl.price} ({lvl.touches} touches, strength: {lvl.strength})')
        
        if stop_result.warnings:
            for warning in stop_result.warnings:
                self.stdout.write(self.style.WARNING(f'  ⚠️  {warning}'))
        
        self.stdout.write('')

        # Show detected levels
        if stop_result.levels_found:
            self.stdout.write(self.style.HTTP_INFO('--- Support Levels Found ---'))
            for i, lvl in enumerate(stop_result.levels_found, 1):
                marker = " ← SELECTED" if i == level_n else ""
                self.stdout.write(f'  {i}. ${lvl.price} ({lvl.touches} touches, strength: {lvl.strength}){marker}')
            self.stdout.write('')

        # Show position sizing
        self.stdout.write(self.style.HTTP_INFO('--- Position Sizing (1% Risk Rule) ---'))
        self.stdout.write(f'Capital: ${capital}')
        self.stdout.write(f'Max Risk (1%): ${result["risk_amount"]}')
        self.stdout.write(f'Position Size: {result["quantity"]} BTC')
        self.stdout.write(f'Position Value: ${result["position_value"]:.2f}')
        self.stdout.write('')

        # Validate
        if not stop_result.is_valid():
            self.stdout.write(self.style.ERROR('INVALID: Stop price is on wrong side of entry!'))
            return

        if stop_result.confidence.value == "low":
            self.stdout.write(self.style.WARNING(
                'LOW CONFIDENCE: Technical levels not clear. Consider manual analysis.'
            ))

        # Create use case for execution
        use_case = RiskManagedTradeUseCase(
            execution_adapter=execution,
            pnl_repository=pnl_repo,
        )

        # Get monthly P&L
        monthly_pnl = pnl_repo.get_monthly_pnl()

        # Validate with risk guards
        self.stdout.write(self.style.HTTP_INFO('--- Risk Validation ---'))
        validation = use_case.validate(
            symbol=symbol,
            side='BUY',
            quantity=result['quantity'],
            entry_price=result['entry_price'],
            stop_price=result['stop_price'],
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
            return

        self.stdout.write(self.style.SUCCESS('All checks PASSED'))
        self.stdout.write('')

        # Execute
        self.stdout.write(self.style.HTTP_INFO('--- Execution ---'))

        exec_result = use_case.execute_buy(
            symbol=symbol,
            capital=capital,
            entry_price=result['entry_price'],
            stop_price=result['stop_price'],
            quantity=result['quantity'],
            mode=mode,
            monthly_pnl=monthly_pnl,
            strategy_name=f"{strategy}_{timeframe}",
        )

        # Show result
        if exec_result.is_success():
            self.stdout.write(self.style.SUCCESS(f'Status: {exec_result.status.value}'))
        elif exec_result.is_blocked():
            self.stdout.write(self.style.ERROR(f'Status: {exec_result.status.value}'))
        else:
            self.stdout.write(self.style.WARNING(f'Status: {exec_result.status.value}'))

        for action in exec_result.actions:
            action_type = action.get('type', 'unknown')
            description = action.get('description', '')
            self.stdout.write(f'  [{action_type}] {description}')

        self.stdout.write('')

        if mode == ExecutionMode.DRY_RUN:
            self.stdout.write(self.style.WARNING(
                'This was a DRY-RUN (analysis only). No real order placed.\n'
                'To execute a real order, add: --live --confirm'
            ))
        else:
            self.stdout.write(self.style.SUCCESS('LIVE order executed with technical stop!'))

        self.stdout.write('')
        self.stdout.write(self.style.HTTP_INFO('=' * 60))

    def _show_levels(self, service, symbol, timeframe):
        """Show support/resistance levels only."""
        self.stdout.write(self.style.HTTP_INFO(f'--- Support/Resistance Levels ({symbol} {timeframe}) ---'))
        
        try:
            levels = service.get_support_resistance_levels(symbol, timeframe)
        except Exception as e:
            self.stdout.write(self.style.ERROR(f'Failed to get levels: {e}'))
            return

        self.stdout.write(f'Current Price: ${levels["current_price"]}')
        self.stdout.write(f'Candles Analyzed: {levels["candles_analyzed"]}')
        self.stdout.write('')

        self.stdout.write(self.style.SUCCESS('RESISTANCES (above price):'))
        if levels['resistances']:
            for i, r in enumerate(levels['resistances'], 1):
                self.stdout.write(f'  {i}. ${r["price"]} ({r["touches"]} touches, strength: {r["strength"]})')
        else:
            self.stdout.write('  (none found)')

        self.stdout.write('')
        self.stdout.write(self.style.WARNING('SUPPORTS (below price):'))
        if levels['supports']:
            for i, s in enumerate(levels['supports'], 1):
                self.stdout.write(f'  {i}. ${s["price"]} ({s["touches"]} touches, strength: {s["strength"]})')
        else:
            self.stdout.write('  (none found)')

        self.stdout.write('')

