# api/management/commands/close_position.py
"""
Django management command to close a trading position.
Creates new strategy if needed, executes sell order, calculates P&L.

Usage:
    python manage.py close_position --operation-id 1 --strategy-name "Pullback MA99 Long"
"""

from decimal import Decimal
from django.core.management.base import BaseCommand
from django.utils import timezone
import json

from api.models import Operation, Order, Strategy, Symbol
from clients.models import Client
from api.services.binance_service import BinanceService
from api.services.market_price_cache import get_cached_bid


class Command(BaseCommand):
    help = 'Close a trading position and document results'

    def add_arguments(self, parser):
        parser.add_argument(
            '--operation-id',
            type=int,
            required=True,
            help='ID of the operation to close'
        )
        parser.add_argument(
            '--strategy-name',
            type=str,
            default='Manual Close',
            help='Name of the strategy to associate with this closure'
        )
        parser.add_argument(
            '--dry-run',
            action='store_true',
            help='Simulate the closure without executing real orders'
        )

    def handle(self, *args, **options):
        operation_id = options['operation_id']
        strategy_name = options['strategy_name']
        dry_run = options['dry_run']

        self.stdout.write("=" * 60)
        self.stdout.write(f"CLOSING OPERATION #{operation_id}")
        if dry_run:
            self.stdout.write(self.style.WARNING("DRY RUN MODE - No real orders will be placed"))
        self.stdout.write("=" * 60)

        try:
            # STEP 1: Get Operation details
            self.stdout.write("\n" + "=" * 60)
            self.stdout.write("STEP 1: Get Operation details")
            self.stdout.write("=" * 60)

            op = Operation.objects.select_related('symbol', 'client').get(id=operation_id)
            self.stdout.write(f"Operation #{op.id}")
            self.stdout.write(f"  Client: {op.client.name if op.client else 'None'} (ID: {op.client_id})")
            self.stdout.write(f"  Symbol: {op.symbol.name}")
            self.stdout.write(f"  Side: {op.side}")
            self.stdout.write(f"  Status: {op.status}")

            if op.status == 'CLOSED':
                self.stdout.write(self.style.WARNING(f"Operation #{operation_id} is already CLOSED"))
                return

            current_strategy = op.strategy
            self.stdout.write(f"  Current Strategy: {current_strategy.name if current_strategy else 'None'}")

            # STEP 2: Create or get strategy
            self.stdout.write("\n" + "=" * 60)
            self.stdout.write(f"STEP 2: Get/Create strategy '{strategy_name}'")
            self.stdout.write("=" * 60)

            client_id = op.client_id

            strategy = Strategy.objects.filter(
                client_id=client_id,
                name=strategy_name
            ).first()

            if strategy:
                self.stdout.write(self.style.SUCCESS(f"✅ Using existing strategy #{strategy.id}: {strategy.name}"))
            else:
                if dry_run:
                    self.stdout.write(self.style.WARNING(f"Would create strategy: {strategy_name}"))
                    # Create a temporary strategy object for dry run
                    strategy = Strategy(
                        client_id=client_id,
                        name=strategy_name,
                        description='',
                        is_active=True
                    )
                else:
                    strategy = Strategy.objects.create(
                        client_id=client_id,
                        name=strategy_name,
                        description=f'Pullback agressivo na MA curta após dump - Mean reversion strategy targeting short-term bounces near MA(99) support after price dumps. High risk, requires strict stop-loss execution.',
                        config={
                            'strategy_type': 'mean_reversion',
                            'timeframe': '15m',
                            'indicators': {'MA_short': 7, 'MA_medium': 25, 'MA_long': 99},
                            'risk_profile': 'HIGH',
                            'horizon': 'short_term',
                            'bias': 'countertrend',
                        },
                        risk_config={
                            'max_leverage': 10,
                            'default_stop_loss_percent': str(Decimal('5.0')),
                            'default_take_profit_percent': str(Decimal('4.0')),
                        },
                        is_active=True
                    )
                    self.stdout.write(self.style.SUCCESS(f"✅ Created strategy #{strategy.id}: {strategy.name}"))

            # STEP 3: Close the position
            self.stdout.write("\n" + "=" * 60)
            self.stdout.write("STEP 3: Close the position")
            self.stdout.write("=" * 60)

            # Calculate position size
            filled_orders = op.entry_orders.filter(status='FILLED')
            total_qty = sum(
                o.filled_quantity or o.quantity or Decimal('0')
                for o in filled_orders
            )

            self.stdout.write(f"Position to close: {total_qty} {op.symbol.name}")

            # Get current price
            current_price = get_cached_bid(op.symbol.name)
            self.stdout.write(f"Current price: ${current_price}")

            if dry_run:
                self.stdout.write(self.style.WARNING(
                    f"DRY RUN: Would place SELL order for {total_qty} {op.symbol.name} at market price ~${current_price}"
                ))
                # Create a fake exit order for P&L calculation
                exit_order = Order(
                    client_id=op.client_id,
                    symbol=op.symbol,
                    strategy=strategy,
                    side='SELL',
                    order_type='MARKET',
                    quantity=total_qty,
                    filled_quantity=total_qty,
                    avg_fill_price=current_price,
                    status='FILLED'
                )
            else:
                # Create exit order
                exit_order = Order.objects.create(
                    client_id=op.client_id,
                    symbol=op.symbol,
                    strategy=strategy,
                    side='SELL',
                    order_type='MARKET',
                    quantity=total_qty,
                    price=None,
                    status='PENDING'
                )

                self.stdout.write(f"Created exit order #{exit_order.id}")

                # Execute order on Binance
                result = binance.place_order(
                    symbol=op.symbol.name,
                    side='SELL',
                    order_type='MARKET',
                    quantity=float(total_qty)
                )

                # Update order with results
                exit_order.binance_order_id = result.get('orderId')
                exit_order.status = result.get('status', 'FILLED')
                exit_order.filled_quantity = Decimal(str(result.get('executedQty', str(total_qty))))

                if 'fills' in result:
                    total_cost = sum(
                        Decimal(f['price']) * Decimal(f['qty'])
                        for f in result['fills']
                    )
                    total_filled = sum(Decimal(f['qty']) for f in result['fills'])
                    exit_order.avg_fill_price = total_cost / total_filled if total_filled else current_price
                else:
                    exit_order.avg_fill_price = current_price

                exit_order.filled_at = timezone.now()
                exit_order.save()

                self.stdout.write(self.style.SUCCESS(
                    f"✅ Exit order FILLED: {exit_order.filled_quantity} @ ${exit_order.avg_fill_price}"
                ))

                # Link to operation
                op.exit_orders.add(exit_order)

                # Update operation status
                op.status = 'CLOSED'
                op.save()

                self.stdout.write(self.style.SUCCESS(f"✅ Operation #{op.id} marked as CLOSED"))

            # STEP 4: Calculate P&L
            self.stdout.write("\n" + "=" * 60)
            self.stdout.write("STEP 4: Calculate P&L and update strategy stats")
            self.stdout.write("=" * 60)

            entry_order = filled_orders.first()
            if entry_order and exit_order:
                entry_price = entry_order.avg_fill_price
                exit_price = exit_order.avg_fill_price
                quantity = exit_order.filled_quantity

                # Calculate P&L (BUY position: profit = exit - entry)
                realized_pnl = (exit_price - entry_price) * quantity
                cost_basis = entry_price * quantity
                pnl_percent = (realized_pnl / cost_basis * Decimal('100')) if cost_basis else Decimal('0')

                summary = {
                    'operation_id': op.id,
                    'strategy': strategy.name,
                    'symbol': op.symbol.name,
                    'side': op.side,
                    'quantity': str(quantity),
                    'entry_price': str(entry_price),
                    'exit_price': str(exit_price),
                    'cost_basis': str(cost_basis),
                    'realized_pnl': str(realized_pnl),
                    'pnl_percent': str(pnl_percent),
                    'entry_time': str(entry_order.filled_at or entry_order.created_at),
                    'exit_time': str(exit_order.filled_at if hasattr(exit_order, 'filled_at') else timezone.now()),
                }

                self.stdout.write("\n📊 TRADE SUMMARY:")
                self.stdout.write(json.dumps(summary, indent=2))

                if not dry_run:
                    # Update strategy performance stats
                    strategy.total_trades += 1
                    if realized_pnl > 0:
                        strategy.winning_trades += 1
                    strategy.total_pnl += realized_pnl
                    strategy.save()

                    self.stdout.write(f"\n✅ Updated strategy performance:")
                    self.stdout.write(f"   Total trades: {strategy.total_trades}")
                    self.stdout.write(f"   Winning trades: {strategy.winning_trades}")
                    self.stdout.write(f"   Total P&L: ${strategy.total_pnl}")
                else:
                    self.stdout.write(self.style.WARNING(
                        f"\nDRY RUN: Would update strategy stats (P&L: ${realized_pnl})"
                    ))

            else:
                self.stdout.write(self.style.WARNING("⚠️  Could not find entry/exit orders for P&L calculation"))

            self.stdout.write("\n" + "=" * 60)
            self.stdout.write(self.style.SUCCESS("✅ DONE!"))
            self.stdout.write("=" * 60)

        except Operation.DoesNotExist:
            self.stdout.write(self.style.ERROR(f"❌ Operation #{operation_id} not found"))
        except Exception as e:
            self.stdout.write(self.style.ERROR(f"❌ Error: {e}"))
            import traceback
            traceback.print_exc()
