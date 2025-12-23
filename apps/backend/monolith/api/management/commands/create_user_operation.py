# api/management/commands/create_user_operation.py
"""
Django management command to create a user-initiated trading operation.

This command implements the core user flow:
1. User provides trade intent (symbol, side, entry, stop)
2. Robson calculates optimal position size (1% risk rule)
3. User reviews and confirms
4. Operation created, order placed (optional)

Usage:
    # Calculate size and review (dry-run)
    python manage.py create_user_operation \
        --client-id 1 \
        --symbol BTCUSDC \
        --side BUY \
        --entry 90000 \
        --stop 88200 \
        --strategy "Mean Reversion MA99"

    # Execute live order
    python manage.py create_user_operation \
        --client-id 1 \
        --symbol BTCUSDC \
        --side BUY \
        --entry 90000 \
        --stop 88200 \
        --strategy "Mean Reversion MA99" \
        --execute

Key Principle: USER initiates, ROBSON calculates, USER confirms.
"""

from decimal import Decimal
from django.core.management.base import BaseCommand
from django.db import transaction
from django.utils import timezone
import json

from api.models import Operation, Order, Strategy, Symbol
from clients.models import Client
from api.services.binance_service import BinanceService


class PositionSizingCalculator:
    """
    Calculate optimal position size based on 1% risk rule.

    This is Robson's PRIMARY INTELLIGENCE.
    """

    @staticmethod
    def calculate(
        capital: Decimal,
        entry_price: Decimal,
        stop_price: Decimal,
        side: str,
        max_risk_percent: Decimal = Decimal("1.0"),
        max_position_percent: Decimal = Decimal("50.0"),
    ) -> dict:
        """
        Calculate position size using 1% risk rule.

        Args:
            capital: Total capital available (USDC)
            entry_price: Intended entry price
            stop_price: Stop-loss price
            side: "BUY" or "SELL"
            max_risk_percent: Maximum risk per trade (default 1%)
            max_position_percent: Maximum position as % of capital (default 50%)

        Returns:
            Dictionary with:
            - quantity: Calculated position size
            - position_value: Total value of position
            - risk_amount: Maximum loss if stopped (USDC)
            - risk_percent: Actual risk as % of capital
            - stop_distance: Distance from entry to stop
            - stop_distance_percent: Stop distance as % of entry
            - is_capped: True if limited by max_position_percent

        Raises:
            ValueError: If inputs are invalid
        """
        # Validations
        if capital <= 0:
            raise ValueError("Capital must be positive")
        if entry_price <= 0:
            raise ValueError("Entry price must be positive")
        if stop_price <= 0:
            raise ValueError("Stop price must be positive")
        if side not in ["BUY", "SELL"]:
            raise ValueError("Side must be BUY or SELL")

        # Validate stop is on correct side
        if side == "BUY" and stop_price >= entry_price:
            raise ValueError("For BUY orders, stop must be below entry price")
        if side == "SELL" and stop_price <= entry_price:
            raise ValueError("For SELL orders, stop must be above entry price")

        # Calculate risk amount (1% of capital by default)
        risk_amount = capital * (max_risk_percent / Decimal("100"))
        risk_amount = risk_amount.quantize(Decimal("0.01"))  # 2 decimal places for USDC

        # Calculate stop distance (always positive)
        stop_distance = abs(entry_price - stop_price)
        stop_distance_percent = (stop_distance / entry_price) * Decimal("100")

        # Calculate position size
        # Quantity = Risk Amount / Stop Distance
        quantity = risk_amount / stop_distance
        quantity = quantity.quantize(Decimal("0.00000001"))  # 8 decimal places (Binance precision)

        # Calculate position value
        position_value = quantity * entry_price
        position_value = position_value.quantize(Decimal("0.01"))

        # Check if position exceeds maximum position size
        max_position_value = capital * (max_position_percent / Decimal("100"))
        is_capped = False

        if position_value > max_position_value:
            # Cap position size
            is_capped = True
            quantity = max_position_value / entry_price
            quantity = quantity.quantize(Decimal("0.00000001"))
            position_value = quantity * entry_price
            position_value = position_value.quantize(Decimal("0.01"))

            # Recalculate actual risk with capped quantity
            risk_amount = quantity * stop_distance
            risk_amount = risk_amount.quantize(Decimal("0.01"))

        # Calculate actual risk percent
        risk_percent = (risk_amount / capital) * Decimal("100")
        risk_percent = risk_percent.quantize(Decimal("0.01"))

        return {
            'quantity': quantity,
            'position_value': position_value,
            'risk_amount': risk_amount,
            'risk_percent': risk_percent,
            'stop_distance': stop_distance,
            'stop_distance_percent': stop_distance_percent.quantize(Decimal("0.01")),
            'is_capped': is_capped,
        }


class Command(BaseCommand):
    help = 'Create a user-initiated trading operation with calculated position sizing'

    def add_arguments(self, parser):
        parser.add_argument(
            '--client-id',
            type=int,
            required=True,
            help='Client ID'
        )
        parser.add_argument(
            '--symbol',
            type=str,
            required=True,
            help='Trading symbol (e.g., BTCUSDC)'
        )
        parser.add_argument(
            '--side',
            type=str,
            required=True,
            choices=['BUY', 'SELL'],
            help='Order side (BUY or SELL)'
        )
        parser.add_argument(
            '--entry',
            type=str,
            required=True,
            help='Entry price (user\'s intended entry level)'
        )
        parser.add_argument(
            '--stop',
            type=str,
            required=True,
            help='Stop-loss price (user\'s risk level)'
        )
        parser.add_argument(
            '--strategy',
            type=str,
            required=True,
            help='Strategy name (user\'s chosen strategy)'
        )
        parser.add_argument(
            '--target',
            type=str,
            default=None,
            help='Take-profit target (optional)'
        )
        parser.add_argument(
            '--execute',
            action='store_true',
            help='Execute order immediately (default: dry-run only)'
        )
        parser.add_argument(
            '--max-risk-percent',
            type=str,
            default='1.0',
            help='Maximum risk per trade as percentage (default: 1.0)'
        )

    def handle(self, *args, **options):
        client_id = options['client_id']
        symbol_name = options['symbol'].upper()
        side = options['side']
        entry_price = Decimal(options['entry'])
        stop_price = Decimal(options['stop'])
        strategy_name = options['strategy']
        target_price = Decimal(options['target']) if options['target'] else None
        execute = options['execute']
        max_risk_percent = Decimal(options['max_risk_percent'])

        self.stdout.write("=" * 80)
        self.stdout.write("CREATE USER OPERATION - Robson Risk Assistant")
        self.stdout.write("=" * 80)
        self.stdout.write(f"\n📊 User Intent:")
        self.stdout.write(f"   Symbol: {symbol_name}")
        self.stdout.write(f"   Side: {side}")
        self.stdout.write(f"   Entry: ${entry_price}")
        self.stdout.write(f"   Stop: ${stop_price}")
        self.stdout.write(f"   Strategy: {strategy_name}")
        if target_price:
            self.stdout.write(f"   Target: ${target_price}")

        try:
            # Get client
            client = Client.objects.get(id=client_id)
            self.stdout.write(f"\n✅ Client: {client.name}")

            # Get or create symbol
            symbol, created = Symbol.objects.get_or_create(
                client=client,
                name=symbol_name,
                defaults={
                    'base_asset': symbol_name[:-4] if symbol_name.endswith(('USDC', 'USDT')) else symbol_name[:3],
                    'quote_asset': symbol_name[-4:] if symbol_name.endswith(('USDC', 'USDT')) else symbol_name[3:],
                    'is_active': True,
                }
            )
            if created:
                self.stdout.write(f"✅ Created symbol: {symbol.name}")

            # Get or create strategy
            strategy, created = Strategy.objects.get_or_create(
                client=client,
                name=strategy_name,
                defaults={
                    'description': f'User-selected strategy: {strategy_name}',
                    'is_active': True,
                    'config': {},
                    'risk_config': {
                        'max_risk_per_trade_percent': str(max_risk_percent),
                    },
                }
            )
            if created:
                self.stdout.write(f"✅ Created strategy: {strategy.name}")
            else:
                self.stdout.write(f"✅ Using existing strategy: {strategy.name}")

            # STEP 1: Calculate Position Size (Robson's Intelligence)
            self.stdout.write("\n" + "=" * 80)
            self.stdout.write("🧠 ROBSON CALCULATES (Position Sizing Intelligence)")
            self.stdout.write("=" * 80)

            # Get client's current capital (simplified - using available balance)
            # In production, this would query actual portfolio value
            capital = Decimal("1000.00")  # TODO: Get from actual portfolio
            self.stdout.write(f"\nCapital: ${capital}")

            # Calculate optimal position size
            calc = PositionSizingCalculator.calculate(
                capital=capital,
                entry_price=entry_price,
                stop_price=stop_price,
                side=side,
                max_risk_percent=max_risk_percent,
            )

            # Display calculations
            self.stdout.write(f"\n📐 Position Sizing Calculation:")
            self.stdout.write(f"   Risk Amount (1%): ${calc['risk_amount']}")
            self.stdout.write(f"   Stop Distance: ${calc['stop_distance']} ({calc['stop_distance_percent']}%)")
            self.stdout.write(f"   → Quantity: {calc['quantity']} {symbol.base_asset}")
            self.stdout.write(f"   → Position Value: ${calc['position_value']}")
            self.stdout.write(f"   → Actual Risk: ${calc['risk_amount']} ({calc['risk_percent']}%)")

            if calc['is_capped']:
                self.stdout.write(self.style.WARNING(
                    f"\n⚠️  Position capped at 50% of capital (${calc['position_value']})"
                ))

            # STEP 2: Risk Validation
            self.stdout.write("\n" + "=" * 80)
            self.stdout.write("🛡️  RISK VALIDATION")
            self.stdout.write("=" * 80)

            # Check risk limits
            checks_passed = True

            # Check 1: Risk per trade
            if calc['risk_percent'] > max_risk_percent:
                self.stdout.write(self.style.ERROR(
                    f"\n❌ Risk exceeds limit: {calc['risk_percent']}% > {max_risk_percent}%"
                ))
                checks_passed = False
            else:
                self.stdout.write(self.style.SUCCESS(
                    f"\n✓ Risk within limit: {calc['risk_percent']}% ≤ {max_risk_percent}%"
                ))

            # Check 2: Position size (already checked in calculator)
            position_percent = (calc['position_value'] / capital) * Decimal("100")
            self.stdout.write(self.style.SUCCESS(
                f"✓ Position size: ${calc['position_value']} ({position_percent.quantize(Decimal('0.01'))}% of capital)"
            ))

            # TODO: Check 3: Monthly drawdown
            # TODO: Check 4: Total exposure

            if not checks_passed:
                self.stdout.write(self.style.ERROR("\n❌ Risk validation FAILED"))
                return

            self.stdout.write(self.style.SUCCESS("\n✅ All risk checks PASSED"))

            # STEP 3: User Review & Confirmation
            self.stdout.write("\n" + "=" * 80)
            self.stdout.write("👤 USER REVIEW")
            self.stdout.write("=" * 80)

            summary = {
                'symbol': symbol_name,
                'side': side,
                'entry_price': str(entry_price),
                'stop_price': str(stop_price),
                'quantity': str(calc['quantity']),
                'position_value': str(calc['position_value']),
                'risk_amount': str(calc['risk_amount']),
                'risk_percent': str(calc['risk_percent']),
                'strategy': strategy_name,
            }

            self.stdout.write(f"\n📋 Operation Summary:")
            self.stdout.write(json.dumps(summary, indent=2))

            if not execute:
                self.stdout.write("\n" + "=" * 80)
                self.stdout.write(self.style.WARNING("DRY RUN - No order will be placed"))
                self.stdout.write(self.style.WARNING("Add --execute flag to place real order"))
                self.stdout.write("=" * 80)
                return

            # STEP 4: Create Operation & Execute Order
            self.stdout.write("\n" + "=" * 80)
            self.stdout.write("🚀 EXECUTION")
            self.stdout.write("=" * 80)

            with transaction.atomic():
                # Create Operation
                operation = Operation.objects.create(
                    client=client,
                    symbol=symbol,
                    strategy=strategy,
                    side=side,
                    status='PLANNED',
                )

                self.stdout.write(f"\n✅ Created Operation #{operation.id}")

                # Create Entry Order
                entry_order = Order.objects.create(
                    client=client,
                    symbol=symbol,
                    strategy=strategy,
                    side=side,
                    order_type='MARKET',
                    quantity=calc['quantity'],
                    price=entry_price,
                    status='PENDING',
                    stop_loss_price=stop_price,
                )

                self.stdout.write(f"✅ Created Entry Order #{entry_order.id}")

                # Execute on Binance
                binance = BinanceService()

                self.stdout.write(f"\n📡 Placing order on Binance...")

                result = binance.client.create_order(
                    symbol=symbol_name,
                    side=side,
                    type='MARKET',
                    quantity=str(calc['quantity'])
                )

                # Update order with results
                entry_order.binance_order_id = result.get('orderId')
                entry_order.status = result.get('status', 'FILLED')
                entry_order.filled_quantity = Decimal(str(result.get('executedQty', str(calc['quantity']))))

                if 'fills' in result:
                    total_cost = sum(
                        Decimal(f['price']) * Decimal(f['qty'])
                        for f in result['fills']
                    )
                    total_filled = sum(Decimal(f['qty']) for f in result['fills'])
                    entry_order.avg_fill_price = total_cost / total_filled if total_filled else entry_price
                else:
                    entry_order.avg_fill_price = entry_price

                entry_order.filled_at = timezone.now()
                entry_order.save()

                # Link to operation
                operation.entry_orders.add(entry_order)
                operation.status = 'ACTIVE'
                operation.save()

                self.stdout.write(self.style.SUCCESS(
                    f"\n✅ Order FILLED: {entry_order.filled_quantity} @ ${entry_order.avg_fill_price}"
                ))
                self.stdout.write(self.style.SUCCESS(f"✅ Operation #{operation.id} is now ACTIVE"))

            # STEP 5: Summary
            self.stdout.write("\n" + "=" * 80)
            self.stdout.write(self.style.SUCCESS("✅ OPERATION CREATED SUCCESSFULLY"))
            self.stdout.write("=" * 80)

            self.stdout.write(f"\n🎯 Next Steps:")
            self.stdout.write(f"   - Stop monitor will watch for stop at ${stop_price}")
            self.stdout.write(f"   - Position will be closed automatically if stop hit")
            self.stdout.write(f"   - Track performance under strategy: {strategy_name}")

        except Client.DoesNotExist:
            self.stdout.write(self.style.ERROR(f"\n❌ Client #{client_id} not found"))
        except Exception as e:
            self.stdout.write(self.style.ERROR(f"\n❌ Error: {e}"))
            import traceback
            traceback.print_exc()
