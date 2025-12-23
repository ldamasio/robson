# api/management/commands/audit_binance_trades.py
"""
Django management command to audit trades by comparing Binance API history with database records.

This command:
1. Fetches order and trade history from Binance API
2. Compares with database records (Order, Trade, Operation)
3. Identifies gaps and inconsistencies
4. Optionally creates missing records for auditability

Usage:
    # Audit specific symbol
    python manage.py audit_binance_trades --client-id 1 --symbol BTCUSDC

    # Audit with auto-fix (create missing records)
    python manage.py audit_binance_trades --client-id 1 --symbol BTCUSDC --auto-fix

    # Dry run (show what would be done)
    python manage.py audit_binance_trades --client-id 1 --symbol BTCUSDC --auto-fix --dry-run
"""

from decimal import Decimal
from django.core.management.base import BaseCommand
from django.utils import timezone
from django.db import transaction
import json
from datetime import datetime

from api.models import Order, Trade, Operation, Strategy, Symbol
from clients.models import Client
from api.services.binance_service import BinanceService


class Command(BaseCommand):
    help = 'Audit trades by comparing Binance API with database records'

    def add_arguments(self, parser):
        parser.add_argument(
            '--client-id',
            type=int,
            required=True,
            help='Client ID to audit'
        )
        parser.add_argument(
            '--symbol',
            type=str,
            required=True,
            help='Trading symbol to audit (e.g., BTCUSDC)'
        )
        parser.add_argument(
            '--auto-fix',
            action='store_true',
            help='Automatically create missing database records'
        )
        parser.add_argument(
            '--dry-run',
            action='store_true',
            help='Show what would be done without making changes'
        )
        parser.add_argument(
            '--days',
            type=int,
            default=30,
            help='Number of days to look back (default: 30)'
        )

    def handle(self, *args, **options):
        client_id = options['client_id']
        symbol_name = options['symbol'].upper()
        auto_fix = options['auto_fix']
        dry_run = options['dry_run']
        days_back = options['days']

        self.stdout.write("=" * 80)
        self.stdout.write(f"BINANCE TRADE AUDIT")
        self.stdout.write("=" * 80)
        self.stdout.write(f"Client ID: {client_id}")
        self.stdout.write(f"Symbol: {symbol_name}")
        self.stdout.write(f"Days back: {days_back}")
        if auto_fix:
            self.stdout.write(self.style.WARNING("Auto-fix: ENABLED"))
        if dry_run:
            self.stdout.write(self.style.WARNING("DRY RUN MODE - No changes will be made"))
        self.stdout.write("=" * 80)

        try:
            # Get client
            client = Client.objects.get(id=client_id)
            self.stdout.write(f"\n✅ Client: {client.name}")

            # Get or create symbol
            symbol, created = Symbol.objects.get_or_create(
                client=client,
                name=symbol_name,
                defaults={
                    'base_asset': symbol_name[:-4] if symbol_name.endswith('USDC') or symbol_name.endswith('USDT') else symbol_name[:3],
                    'quote_asset': symbol_name[-4:] if symbol_name.endswith('USDC') or symbol_name.endswith('USDT') else symbol_name[3:],
                    'is_active': True,
                }
            )
            if created:
                self.stdout.write(f"✅ Created symbol: {symbol.name}")
            else:
                self.stdout.write(f"✅ Symbol exists: {symbol.name}")

            # STEP 1: Fetch from Binance API
            self.stdout.write("\n" + "=" * 80)
            self.stdout.write("STEP 1: Fetch data from Binance API")
            self.stdout.write("=" * 80)

            binance = BinanceService()

            # Get all orders
            self.stdout.write(f"\nFetching orders for {symbol_name}...")
            binance_orders = binance.client.get_all_orders(symbol=symbol_name)
            self.stdout.write(f"✅ Found {len(binance_orders)} orders on Binance")

            # Get all trades
            self.stdout.write(f"\nFetching trades for {symbol_name}...")
            binance_trades = binance.client.get_my_trades(symbol=symbol_name)
            self.stdout.write(f"✅ Found {len(binance_trades)} trades on Binance")

            # STEP 2: Fetch from Database
            self.stdout.write("\n" + "=" * 80)
            self.stdout.write("STEP 2: Fetch data from Database")
            self.stdout.write("=" * 80)

            db_orders = Order.objects.filter(
                client=client,
                symbol=symbol
            ).select_related('symbol', 'strategy')

            db_trades = Trade.objects.filter(
                client=client,
                symbol=symbol
            ).select_related('symbol', 'strategy')

            db_operations = Operation.objects.filter(
                client=client,
                symbol=symbol
            ).select_related('symbol', 'strategy')

            self.stdout.write(f"\n✅ Database has:")
            self.stdout.write(f"   - {db_orders.count()} orders")
            self.stdout.write(f"   - {db_trades.count()} trades")
            self.stdout.write(f"   - {db_operations.count()} operations")

            # STEP 3: Reconciliation
            self.stdout.write("\n" + "=" * 80)
            self.stdout.write("STEP 3: Reconciliation Analysis")
            self.stdout.write("=" * 80)

            # Map Binance orders by orderId
            binance_orders_map = {str(o['orderId']): o for o in binance_orders}

            # Map DB orders by binance_order_id
            db_orders_map = {o.binance_order_id: o for o in db_orders if o.binance_order_id}

            # Find missing orders (in Binance but not in DB)
            missing_order_ids = set(binance_orders_map.keys()) - set(db_orders_map.keys())
            extra_order_ids = set(db_orders_map.keys()) - set(binance_orders_map.keys())

            self.stdout.write(f"\n📊 Order Reconciliation:")
            self.stdout.write(f"   - Binance orders: {len(binance_orders_map)}")
            self.stdout.write(f"   - Database orders: {len(db_orders_map)}")
            self.stdout.write(f"   - Missing in DB: {len(missing_order_ids)}")
            self.stdout.write(f"   - Extra in DB: {len(extra_order_ids)}")

            if missing_order_ids:
                self.stdout.write(f"\n⚠️  MISSING ORDERS IN DATABASE:")
                for order_id in sorted(missing_order_ids):
                    binance_order = binance_orders_map[order_id]
                    self.stdout.write(f"\n   Order ID: {order_id}")
                    self.stdout.write(f"     Side: {binance_order['side']}")
                    self.stdout.write(f"     Type: {binance_order['type']}")
                    self.stdout.write(f"     Status: {binance_order['status']}")
                    self.stdout.write(f"     Quantity: {binance_order['executedQty']} / {binance_order['origQty']}")
                    self.stdout.write(f"     Price: {binance_order.get('price', 'MARKET')}")
                    self.stdout.write(f"     Time: {datetime.fromtimestamp(binance_order['time'] / 1000)}")

            # STEP 4: Analyze Trades
            self.stdout.write("\n" + "=" * 80)
            self.stdout.write("STEP 4: Analyze Trade Pairs (Buy + Sell)")
            self.stdout.write("=" * 80)

            # Separate BUY and SELL orders
            buy_orders = [o for o in binance_orders if o['side'] == 'BUY' and o['status'] == 'FILLED']
            sell_orders = [o for o in binance_orders if o['side'] == 'SELL' and o['status'] == 'FILLED']

            self.stdout.write(f"\n✅ Filled orders:")
            self.stdout.write(f"   - BUY orders: {len(buy_orders)}")
            self.stdout.write(f"   - SELL orders: {len(sell_orders)}")

            # Try to match buy/sell pairs (simple chronological matching)
            trade_pairs = []
            for buy in buy_orders:
                # Find next sell after this buy
                buy_time = buy['time']
                matching_sells = [s for s in sell_orders if s['time'] > buy_time]

                if matching_sells:
                    sell = matching_sells[0]  # Take the first (earliest) sell after buy

                    # Calculate P&L
                    buy_qty = Decimal(buy['executedQty'])
                    buy_price = Decimal(buy.get('cummulativeQuoteQty', '0')) / buy_qty if buy_qty > 0 else Decimal('0')

                    sell_qty = Decimal(sell['executedQty'])
                    sell_price = Decimal(sell.get('cummulativeQuoteQty', '0')) / sell_qty if sell_qty > 0 else Decimal('0')

                    qty = min(buy_qty, sell_qty)
                    pnl = (sell_price - buy_price) * qty

                    trade_pairs.append({
                        'buy_order': buy,
                        'sell_order': sell,
                        'quantity': qty,
                        'entry_price': buy_price,
                        'exit_price': sell_price,
                        'pnl': pnl,
                        'pnl_percent': (pnl / (buy_price * qty) * Decimal('100')) if (buy_price * qty) > 0 else Decimal('0'),
                    })

            self.stdout.write(f"\n📊 Identified {len(trade_pairs)} complete trade pair(s):")

            for i, pair in enumerate(trade_pairs, 1):
                self.stdout.write(f"\n   Trade #{i}:")
                self.stdout.write(f"     Buy Order ID: {pair['buy_order']['orderId']}")
                self.stdout.write(f"     Sell Order ID: {pair['sell_order']['orderId']}")
                self.stdout.write(f"     Quantity: {pair['quantity']}")
                self.stdout.write(f"     Entry: ${pair['entry_price']:.2f}")
                self.stdout.write(f"     Exit: ${pair['exit_price']:.2f}")
                self.stdout.write(f"     P&L: ${pair['pnl']:.8f} ({pair['pnl_percent']:.2f}%)")

            # STEP 5: Auto-fix (if enabled)
            if auto_fix and missing_order_ids:
                self.stdout.write("\n" + "=" * 80)
                self.stdout.write("STEP 5: Auto-Fix - Create Missing Records")
                self.stdout.write("=" * 80)

                if dry_run:
                    self.stdout.write(self.style.WARNING("\nDRY RUN - Would create the following records:\n"))

                # Get or create default strategy
                strategy, created = Strategy.objects.get_or_create(
                    client=client,
                    name="Manual Trading",
                    defaults={
                        'description': 'Manual trades executed outside the system',
                        'is_active': True,
                        'config': {},
                        'risk_config': {},
                    }
                )

                created_orders = []
                created_trades = []
                created_operations = []

                with transaction.atomic():
                    # Create missing orders
                    for order_id in sorted(missing_order_ids):
                        binance_order = binance_orders_map[order_id]

                        executed_qty = Decimal(binance_order['executedQty'])

                        # Calculate average fill price
                        if executed_qty > 0 and binance_order.get('cummulativeQuoteQty'):
                            avg_price = Decimal(binance_order['cummulativeQuoteQty']) / executed_qty
                        else:
                            avg_price = Decimal(binance_order.get('price', '0')) if binance_order.get('price') else None

                        order_data = {
                            'client': client,
                            'symbol': symbol,
                            'strategy': strategy,
                            'side': binance_order['side'],
                            'order_type': binance_order['type'],
                            'quantity': Decimal(binance_order['origQty']),
                            'price': Decimal(binance_order['price']) if binance_order.get('price') else None,
                            'status': binance_order['status'],
                            'binance_order_id': str(binance_order['orderId']),
                            'filled_quantity': executed_qty,
                            'avg_fill_price': avg_price,
                            'filled_at': datetime.fromtimestamp(binance_order['updateTime'] / 1000) if binance_order['status'] == 'FILLED' else None,
                        }

                        if dry_run:
                            self.stdout.write(f"\n   Would create Order:")
                            self.stdout.write(f"     {order_data['side']} {order_data['quantity']} @ {order_data['avg_fill_price']}")
                        else:
                            order = Order.objects.create(**order_data)
                            created_orders.append(order)
                            self.stdout.write(self.style.SUCCESS(f"\n   ✅ Created Order #{order.id}: {order}"))

                    # Create Trade records for complete pairs
                    for pair in trade_pairs:
                        # Check if Trade already exists
                        buy_order_id = str(pair['buy_order']['orderId'])
                        sell_order_id = str(pair['sell_order']['orderId'])

                        # Find or create orders in DB
                        buy_order_db = db_orders_map.get(buy_order_id) or next((o for o in created_orders if o.binance_order_id == buy_order_id), None)
                        sell_order_db = db_orders_map.get(sell_order_id) or next((o for o in created_orders if o.binance_order_id == sell_order_id), None)

                        if not buy_order_db or not sell_order_db:
                            self.stdout.write(self.style.WARNING(f"\n   ⚠️  Skipping trade pair - orders not found in DB"))
                            continue

                        trade_data = {
                            'client': client,
                            'symbol': symbol,
                            'strategy': strategy,
                            'side': 'BUY',  # Position side
                            'quantity': pair['quantity'],
                            'entry_price': pair['entry_price'],
                            'exit_price': pair['exit_price'],
                            'entry_time': datetime.fromtimestamp(pair['buy_order']['time'] / 1000),
                            'exit_time': datetime.fromtimestamp(pair['sell_order']['time'] / 1000),
                            'pnl': pair['pnl'].quantize(Decimal('0.00000001')),
                        }

                        if dry_run:
                            self.stdout.write(f"\n   Would create Trade:")
                            self.stdout.write(f"     {trade_data['quantity']} {symbol_name}")
                            self.stdout.write(f"     Entry: ${trade_data['entry_price']} @ {trade_data['entry_time']}")
                            self.stdout.write(f"     Exit: ${trade_data['exit_price']} @ {trade_data['exit_time']}")
                            self.stdout.write(f"     P&L: ${trade_data['pnl']}")
                        else:
                            trade = Trade.objects.create(**trade_data)
                            created_trades.append(trade)
                            self.stdout.write(self.style.SUCCESS(f"\n   ✅ Created Trade #{trade.id}: P&L ${trade.pnl}"))

                            # Create Operation for this trade
                            operation = Operation.objects.create(
                                client=client,
                                symbol=symbol,
                                strategy=strategy,
                                side='BUY',
                                status='CLOSED',
                            )
                            operation.entry_orders.add(buy_order_db)
                            operation.exit_orders.add(sell_order_db)
                            created_operations.append(operation)
                            self.stdout.write(self.style.SUCCESS(f"   ✅ Created Operation #{operation.id}"))

                    if dry_run:
                        raise Exception("DRY RUN - Rolling back transaction")

                self.stdout.write(f"\n✅ Auto-fix completed:")
                self.stdout.write(f"   - Created {len(created_orders)} orders")
                self.stdout.write(f"   - Created {len(created_trades)} trades")
                self.stdout.write(f"   - Created {len(created_operations)} operations")

            # STEP 6: Summary
            self.stdout.write("\n" + "=" * 80)
            self.stdout.write("AUDIT SUMMARY")
            self.stdout.write("=" * 80)

            self.stdout.write(f"\n✅ Audit completed for {symbol_name}")
            self.stdout.write(f"   - Binance: {len(binance_orders)} orders, {len(binance_trades)} trades")
            self.stdout.write(f"   - Database: {db_orders.count()} orders, {db_trades.count()} trades")
            self.stdout.write(f"   - Identified: {len(trade_pairs)} complete trade pairs")

            if missing_order_ids:
                self.stdout.write(f"\n⚠️  Action required: {len(missing_order_ids)} orders missing in database")
                self.stdout.write(f"   Run with --auto-fix to create missing records")
            else:
                self.stdout.write(f"\n✅ All Binance orders are recorded in database")

        except Client.DoesNotExist:
            self.stdout.write(self.style.ERROR(f"\n❌ Client #{client_id} not found"))
        except Exception as e:
            if "DRY RUN" in str(e):
                self.stdout.write(self.style.WARNING(f"\n✅ Dry run completed - no changes made"))
            else:
                self.stdout.write(self.style.ERROR(f"\n❌ Error: {e}"))
                import traceback
                traceback.print_exc()
