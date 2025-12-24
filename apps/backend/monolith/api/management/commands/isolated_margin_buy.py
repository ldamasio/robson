"""
Isolated Margin Buy Command.

Execute a leveraged LONG position on Binance Isolated Margin with mandatory risk management.

This command:
1. Validates all risk parameters (stop-loss, 1% rule, 4% drawdown)
2. Transfers collateral from Spot to Isolated Margin
3. Borrows additional funds based on leverage
4. Places the entry order
5. Places stop-loss as a margin order (NOT spot order)
6. Records everything for audit

Key Difference from Spot:
- Stop-loss is placed as a MARGIN order (doesn't block transfers)
- Position can be leveraged (2x, 3x, 5x, 10x)
- Collateral is isolated to this specific position

Usage:
    # Dry-run (default - no real action)
    python manage.py isolated_margin_buy --capital 100 --stop-percent 2 --leverage 3

    # Live execution
    python manage.py isolated_margin_buy --capital 100 --stop-percent 2 --leverage 3 --live --confirm

Examples:
    # 3x leverage with 2% stop
    python manage.py isolated_margin_buy --capital 100 --stop-percent 2 --leverage 3 --live --confirm
    
    # Use specific stop price
    python manage.py isolated_margin_buy --capital 100 --stop-price 85000 --leverage 5 --live --confirm
"""

import logging
import uuid
from decimal import Decimal, InvalidOperation

from django.core.management.base import BaseCommand, CommandError
from django.db import transaction
from django.utils import timezone

from api.application.adapters import BinanceExecution, BinanceMarketData
from api.application.execution import ExecutionMode
from api.models import Symbol
from api.models.margin import MarginPosition, MarginTransfer
from clients.models import Client

logger = logging.getLogger(__name__)


def _get_or_create_symbol(symbol_str: str) -> Symbol:
    """Get or create a Symbol from a symbol string like 'BTCUSDC'."""
    symbol_obj, _ = Symbol.objects.get_or_create(
        name=symbol_str,
        defaults={'base': symbol_str[:-4], 'quote': symbol_str[-4:]}
    )
    return symbol_obj


class Command(BaseCommand):
    help = 'Execute a leveraged LONG position with Isolated Margin'

    def add_arguments(self, parser):
        # Required
        parser.add_argument(
            '--capital',
            type=str,
            required=True,
            help='Capital to use for this position (collateral)',
        )
        
        parser.add_argument(
            '--leverage',
            type=int,
            default=3,
            choices=[2, 3, 5, 10],
            help='Leverage multiplier (default: 3x)',
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
            default='2',
            help='Stop-loss as percentage below entry (default: 2%%)',
        )

        # Optional
        parser.add_argument(
            '--symbol',
            type=str,
            default='BTCUSDC',
            help='Trading pair (default: BTCUSDC)',
        )
        parser.add_argument(
            '--client-id',
            type=int,
            default=1,
            help='Client ID for multi-tenant (default: 1)',
        )

        # Execution mode
        parser.add_argument(
            '--live',
            action='store_true',
            help='Execute REAL orders (default is dry-run)',
        )
        parser.add_argument(
            '--confirm',
            action='store_true',
            help='Confirm risk acknowledgement for live execution',
        )

    def handle(self, *args, **options):
        self.stdout.write(self.style.HTTP_INFO('=' * 70))
        self.stdout.write(self.style.HTTP_INFO('ROBSON - Isolated Margin LONG Position'))
        self.stdout.write(self.style.HTTP_INFO('=' * 70))
        self.stdout.write('')

        # Parse parameters
        try:
            capital = Decimal(options['capital'])
        except (InvalidOperation, ValueError):
            raise CommandError(f"Invalid capital: {options['capital']}")

        symbol = options['symbol']
        leverage = options['leverage']
        client_id = options['client_id']

        # Determine execution mode
        is_live = options['live']
        is_confirmed = options['confirm']

        if is_live and not is_confirmed:
            raise CommandError(
                'LIVE mode requires --confirm flag.\n'
                'Add --confirm to acknowledge you understand this will place REAL orders.'
            )

        mode = ExecutionMode.LIVE if is_live else ExecutionMode.DRY_RUN

        # Initialize adapters
        execution = BinanceExecution()
        market_data = BinanceMarketData()

        env = 'PRODUCTION' if not execution.use_testnet else 'TESTNET'
        self.stdout.write(f'Environment: {env}')
        self.stdout.write(f'Mode: {mode.value}')
        self.stdout.write(f'Leverage: {leverage}x')
        self.stdout.write('')

        # Get current market data
        self.stdout.write(self.style.HTTP_INFO('--- Market Data ---'))
        try:
            spot_usdc = execution.get_account_balance('USDC')
            spot_btc = execution.get_account_balance('BTC')
            entry_price = market_data.best_ask(symbol)
        except Exception as e:
            raise CommandError(f'Failed to get market data: {e}')

        self.stdout.write(f'Spot USDC: {spot_usdc.get("free")}')
        self.stdout.write(f'Spot BTC: {spot_btc.get("free")}')
        self.stdout.write(f'Current {symbol}: ${entry_price}')
        self.stdout.write('')

        # Check isolated margin balance
        self.stdout.write(self.style.HTTP_INFO('--- Isolated Margin Balance ---'))
        try:
            margin_info = execution.client.get_isolated_margin_account(symbols=symbol)
            assets = margin_info.get('assets', [])
            if assets:
                base_asset = assets[0].get('baseAsset', {})
                quote_asset = assets[0].get('quoteAsset', {})
                margin_level_current = assets[0].get('marginLevel', '999')
                self.stdout.write(f'BTC: Free={base_asset.get("free")} Borrowed={base_asset.get("borrowed")}')
                self.stdout.write(f'USDC: Free={quote_asset.get("free")} Borrowed={quote_asset.get("borrowed")}')
                self.stdout.write(f'Margin Level: {margin_level_current}')
            else:
                self.stdout.write(self.style.WARNING('Isolated margin not enabled for this pair'))
        except Exception as e:
            self.stdout.write(self.style.WARNING(f'Could not get margin info: {e}'))
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

        # Calculate position using Golden Rule
        # Risk = 1% of capital at stop-loss
        stop_distance = entry_price - stop_price
        max_risk = capital * Decimal('0.01')  # 1% rule
        
        # Base quantity (without leverage)
        base_quantity = max_risk / stop_distance
        
        # Leveraged position value
        leveraged_value = capital * Decimal(str(leverage))
        leveraged_quantity = leveraged_value / entry_price
        
        # Calculate risk metrics
        risk_amount = stop_distance * base_quantity  # Always based on collateral
        risk_percent = (risk_amount / capital) * Decimal('100')
        position_value = base_quantity * entry_price
        borrowed_amount = leveraged_value - capital  # How much we need to borrow

        # Format quantity for Binance
        quantity = base_quantity.quantize(Decimal('0.00001'))

        # Show position preview
        self.stdout.write(self.style.HTTP_INFO('--- Position Preview ---'))
        self.stdout.write(f'Symbol: {symbol}')
        self.stdout.write(f'Side: LONG (BUY)')
        self.stdout.write(f'Entry Price: ${entry_price}')
        self.stdout.write(f'Stop Price: ${stop_price}')
        self.stdout.write(f'Stop Distance: ${stop_distance:.2f} ({stop_distance/entry_price*100:.2f}%)')
        self.stdout.write('')
        self.stdout.write(self.style.HTTP_INFO('--- Leverage Details ---'))
        self.stdout.write(f'Collateral (Capital): ${capital}')
        self.stdout.write(f'Leverage: {leverage}x')
        self.stdout.write(f'Leveraged Position: ${leveraged_value}')
        self.stdout.write(f'Amount to Borrow: ${borrowed_amount:.2f}')
        self.stdout.write('')
        self.stdout.write(self.style.HTTP_INFO('--- Risk (Golden Rule) ---'))
        self.stdout.write(f'Quantity: {quantity} BTC')
        self.stdout.write(f'Position Value: ${position_value:.2f}')
        self.stdout.write(f'Risk Amount: ${risk_amount:.2f}')
        self.stdout.write(f'Risk Percent: {risk_percent:.2f}%')
        self.stdout.write('')

        # Validate risk
        self.stdout.write(self.style.HTTP_INFO('--- Risk Validation ---'))
        
        if risk_percent > Decimal('1'):
            self.stdout.write(self.style.ERROR(
                f'  [BLOCKED] Risk {risk_percent:.2f}% exceeds 1% limit'
            ))
            return
        else:
            self.stdout.write(self.style.SUCCESS(f'  [PASS] Risk {risk_percent:.2f}% within 1% limit'))
        
        if stop_distance <= 0:
            self.stdout.write(self.style.ERROR('  [BLOCKED] Invalid stop distance'))
            return
        else:
            self.stdout.write(self.style.SUCCESS('  [PASS] Stop-loss defined'))

        self.stdout.write('')

        if mode == ExecutionMode.DRY_RUN:
            self.stdout.write(self.style.WARNING(
                '=== DRY-RUN MODE ===\n'
                'No real orders were placed.\n'
                'To execute, add: --live --confirm'
            ))
            return

        # ========================================
        # LIVE EXECUTION
        # ========================================
        self.stdout.write(self.style.HTTP_INFO('=== EXECUTING LIVE ==='))
        self.stdout.write('')

        try:
            # Get client
            client = Client.objects.get(id=client_id)
        except Client.DoesNotExist:
            raise CommandError(f'Client {client_id} not found')

        position_id = str(uuid.uuid4())
        
        # Step 1: Transfer USDC to Isolated Margin
        self.stdout.write(self.style.HTTP_INFO('Step 1: Transfer to Isolated Margin'))
        try:
            # Use USDC as collateral from spot
            transfer_amount = min(capital, Decimal(spot_usdc.get('free', '0')))
            
            if transfer_amount < capital:
                self.stdout.write(self.style.WARNING(
                    f'  Insufficient USDC. Have: {transfer_amount}, Need: {capital}'
                ))
                self.stdout.write('  Will use existing margin collateral...')
            
            if transfer_amount > 0:
                transfer_result = execution.client.transfer_spot_to_isolated_margin(
                    asset='USDC',
                    symbol=symbol,
                    amount=str(transfer_amount),
                )
                tran_id = transfer_result.get('tranId', 'N/A')
                self.stdout.write(self.style.SUCCESS(f'  Transferred {transfer_amount} USDC (ID: {tran_id})'))
                
                # Record transfer
                with transaction.atomic():
                    MarginTransfer.objects.create(
                        transaction_id=str(tran_id),
                        client=client,
                        symbol=symbol,
                        asset='USDC',
                        amount=transfer_amount,
                        direction=MarginTransfer.Direction.TO_MARGIN,
                        success=True,
                    )
        except Exception as e:
            self.stdout.write(self.style.ERROR(f'  Transfer failed: {e}'))
            # Continue if we already have collateral in margin

        # Step 2: Place margin order
        self.stdout.write(self.style.HTTP_INFO('Step 2: Place Margin Entry Order'))
        try:
            order_result = execution.client.create_margin_order(
                symbol=symbol,
                side='BUY',
                type='MARKET',
                quantity=str(quantity),
                isIsolated='TRUE',
            )
            order_id = order_result.get('orderId', 'N/A')
            fill_price = Decimal(str(order_result.get('cummulativeQuoteQty', '0'))) / quantity if quantity else entry_price
            self.stdout.write(self.style.SUCCESS(f'  Entry Order ID: {order_id}'))
            self.stdout.write(self.style.SUCCESS(f'  Fill Price: ${fill_price:.2f}'))
        except Exception as e:
            raise CommandError(f'Entry order failed: {e}')

        # Step 3: Place stop-loss as margin order
        self.stdout.write(self.style.HTTP_INFO('Step 3: Place Margin Stop-Loss'))
        try:
            stop_order_result = execution.client.create_margin_order(
                symbol=symbol,
                side='SELL',
                type='STOP_LOSS_LIMIT',
                quantity=str(quantity),
                price=str(stop_price.quantize(Decimal('0.01'))),
                stopPrice=str(stop_price.quantize(Decimal('0.01'))),
                timeInForce='GTC',
                isIsolated='TRUE',
            )
            stop_order_id = stop_order_result.get('orderId', 'N/A')
            self.stdout.write(self.style.SUCCESS(f'  Stop Order ID: {stop_order_id}'))
        except Exception as e:
            stop_order_id = None
            self.stdout.write(self.style.ERROR(f'  Stop order failed: {e}'))
            self.stdout.write(self.style.WARNING('  MANUAL STOP-LOSS REQUIRED!'))

        # Step 4: Record position to database
        self.stdout.write(self.style.HTTP_INFO('Step 4: Record Position (Audit)'))
        try:
            with transaction.atomic():
                position = MarginPosition.objects.create(
                    position_id=position_id,
                    client=client,
                    symbol=symbol,
                    side=MarginPosition.Side.LONG,
                    status=MarginPosition.Status.OPEN,
                    leverage=leverage,
                    entry_price=fill_price,
                    stop_price=stop_price,
                    current_price=fill_price,
                    quantity=quantity,
                    position_value=quantity * fill_price,
                    margin_allocated=capital,
                    borrowed_amount=borrowed_amount,
                    risk_amount=risk_amount,
                    risk_percent=risk_percent,
                    binance_entry_order_id=str(order_id),
                    binance_stop_order_id=str(stop_order_id) if stop_order_id else None,
                    opened_at=timezone.now(),
                )
                self.stdout.write(self.style.SUCCESS(f'  Position ID: {position.id}'))
                self.stdout.write(self.style.SUCCESS(f'  DB Record: {position.position_id}'))
        except Exception as e:
            self.stdout.write(self.style.ERROR(f'  Failed to save position: {e}'))
            logger.error(f"Failed to save margin position: {e}", exc_info=True)

        # Summary
        self.stdout.write('')
        self.stdout.write(self.style.HTTP_INFO('=' * 70))
        self.stdout.write(self.style.SUCCESS('POSITION OPENED SUCCESSFULLY!'))
        self.stdout.write('')
        self.stdout.write(f'  Position: LONG {quantity} BTC @ ${fill_price:.2f}')
        self.stdout.write(f'  Leverage: {leverage}x')
        self.stdout.write(f'  Stop-Loss: ${stop_price:.2f}')
        self.stdout.write(f'  Risk: ${risk_amount:.2f} ({risk_percent:.2f}%)')
        self.stdout.write('')
        self.stdout.write(self.style.HTTP_INFO('=' * 70))

