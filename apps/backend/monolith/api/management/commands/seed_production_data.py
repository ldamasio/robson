import random
from decimal import Decimal
from datetime import timedelta
from django.core.management.base import BaseCommand
from django.contrib.auth import get_user_model
from django.utils import timezone
from api.models import Symbol, Strategy, Order, Operation, Trade, MarginPosition, MarginTransfer

class Command(BaseCommand):
    help = 'Seeds database with realistic production-like data for user robson'

    def handle(self, *args, **options):
        User = get_user_model()
        try:
            user = User.objects.get(username='robson')
        except User.DoesNotExist:
            self.stdout.write(self.style.ERROR('User "robson" not found. Please create it first.'))
            return

        if not user.client:
            from clients.models import Client
            client, _ = Client.objects.get_or_create(
                email="robson@example.com",
                defaults={"name": "Robson Trading", "is_active": True}
            )
            user.client = client
            user.save()
            self.stdout.write(f"Created/Assigned client to user: {client.name}")
        
        client = user.client

        # 1. Create Symbols
        symbols_data = [
            ("BTCUSDC", "BTC", "USDC"),
            ("ETHUSDC", "ETH", "USDC"),
            ("SOLUSDC", "SOL", "USDC"),
            ("BNBUSDC", "BNB", "USDC"),
        ]
        
        symbols = {}
        for name, base, quote in symbols_data:
            symbol, created = Symbol.objects.get_or_create(
                name=name,
                client=client,
                defaults={
                    "base_asset": base,
                    "quote_asset": quote,
                    "is_active": True
                }
            )
            symbols[name] = symbol
            if created:
                self.stdout.write(f"Created symbol: {name}")

        # 2. Create Strategies
        strategies_data = ["Trend Following", "Mean Reversion", "Breakout"]
        strategies = {}
        for s_name in strategies_data:
            strategy, created = Strategy.objects.get_or_create(
                name=s_name,
                client=client,
                defaults={
                    "description": f"Standard {s_name} strategy",
                    "is_active": True
                }
            )
            strategies[s_name] = strategy
            if created:
                self.stdout.write(f"Created strategy: {s_name}")

        # 3. Create active Operations (Positions)
        # Position 1: BTCUSDC Long (Winning)
        self._create_active_position(
            client, 
            symbols["BTCUSDC"], 
            strategies["Trend Following"],
            side="BUY",
            entry_price=Decimal("95000.00"),
            quantity=Decimal("0.15"),
            current_mock_price=Decimal("96500.00") # $1500 profit/unit
        )

        # Position 2: ETHUSDC Long (Losing)
        self._create_active_position(
            client, 
            symbols["ETHUSDC"], 
            strategies["Mean Reversion"],
            side="BUY",
            entry_price=Decimal("3500.00"),
            quantity=Decimal("2.5"),
            current_mock_price=Decimal("3420.00") # $80 loss/unit
        )
        
        # Position 3: SOLUSDC Short (Winning)
        self._create_active_position(
            client, 
            symbols["SOLUSDC"], 
            strategies["Breakout"],
            side="SELL",
            entry_price=Decimal("190.00"),
            quantity=Decimal("15.0"),
            current_mock_price=Decimal("185.00") # $5 profit/unit
        )

        # 4. Create Historical Trades
        self._create_history(client, symbols, strategies)

        # 5. Create Margin Positions & Transfers
        self._create_margin_data(client, symbols)

        self.stdout.write(self.style.SUCCESS('Successfully seeded production-like data!'))

    def _create_active_position(self, client, symbol, strategy, side, entry_price, quantity, current_mock_price):
        # Create the Operation
        op = Operation.objects.create(
            client=client,
            symbol=symbol,
            strategy=strategy,
            side=side,
            status="ACTIVE",
            stop_loss_percent=Decimal("2.0"),
            stop_gain_percent=Decimal("5.0")
        )

        # Create the Entry Order
        order = Order.objects.create(
            client=client,
            symbol=symbol,
            strategy=strategy,
            side=side,
            order_type="MARKET",
            quantity=quantity,
            price=entry_price,
            filled_quantity=quantity,
            avg_fill_price=entry_price,
            status="FILLED",
            filled_at=timezone.now() - timedelta(hours=random.randint(1, 48)),
            binance_order_id=str(random.randint(1000000, 9999999))
        )
        op.entry_orders.add(order)
        op.save()
        self.stdout.write(f"Created Active Position: {side} {symbol.name} x {quantity}")

    def _create_history(self, client, symbols, strategies):
        # Generate last 30 days of trades
        base_time = timezone.now()
        
        history_scenarios = [
            # (Symbol, Strategy, Side, Entry, Exit, Qty, is_win)
            ("BTCUSDC", "Trend Following", "BUY", 92000, 94500, 0.1, True),
            ("BTCUSDC", "Trend Following", "BUY", 94000, 93200, 0.1, False),
            ("ETHUSDC", "Mean Reversion", "SELL", 3550, 3400, 2.0, True),
            ("ETHUSDC", "Mean Reversion", "BUY", 3300, 3350, 2.0, True),
            ("SOLUSDC", "Breakout", "BUY", 150, 180, 10.0, True),
            ("SOLUSDC", "Breakout", "SELL", 185, 190, 10.0, False), # Short sold at 185, buy back at 190 (Loss)
            ("BNBUSDC", "Trend Following", "BUY", 600, 610, 5.0, True),
        ]

        for sym_name, strat_name, side, entry, exit_val, qty, is_win in history_scenarios:
            days_ago = random.randint(1, 30)
            entry_time = base_time - timedelta(days=days_ago)
            exit_time = entry_time + timedelta(hours=random.randint(1, 12))
            
            entry_px = Decimal(str(entry))
            exit_px = Decimal(str(exit_val))
            quantity = Decimal(str(qty))
            
            # Calculate fees (approx 0.1%)
            entry_fee = entry_px * quantity * Decimal("0.001")
            exit_fee = exit_px * quantity * Decimal("0.001")
            
            # Create Trade
            Trade.objects.create(
                client=client,
                symbol=symbols[sym_name],
                strategy=strategies[strat_name],
                side=side,
                quantity=quantity,
                entry_price=entry_px,
                exit_price=exit_px,
                entry_fee=entry_fee,
                exit_fee=exit_fee,
                entry_time=entry_time,
                exit_time=exit_time,
                # PnL is auto-calculated on save
            )
        
        self.stdout.write(f"Created {len(history_scenarios)} historical trades.")

    def _create_margin_data(self, client, symbols):
        import uuid
        
        # Scenario 1: BTCUSDC Long (Requested style)
        entry_price = Decimal("87193.34")
        stop_price = Decimal("85449.47")
        quantity = Decimal("0.00047985")
        risk_amount = Decimal("0.28")
        
        pos = MarginPosition.objects.create(
            position_id=f"margin-{uuid.uuid4()}",
            client=client,
            symbol="BTCUSDC",
            side="LONG",
            status="OPEN",
            leverage=3,
            entry_price=entry_price,
            stop_price=stop_price,
            quantity=quantity,
            position_value=entry_price * quantity,
            margin_allocated=Decimal("14.00"),
            risk_amount=risk_amount,
            risk_percent=Decimal("1.00"),
            current_price=Decimal("87350.00"),
            binance_entry_order_id="99887766",
            binance_stop_order_id="7634794756",
            margin_level=Decimal("3.06"),
            opened_at=timezone.now() - timedelta(hours=2)
        )
        
        # Add 3 transfers
        for i in range(3):
            MarginTransfer.objects.create(
                transaction_id=f"tx-{uuid.uuid4()}",
                client=client,
                symbol="BTCUSDC",
                asset="USDC",
                amount=Decimal("10.00"),
                direction="TO_MARGIN" if i == 0 else "FROM_MARGIN",
                success=True,
                position=pos
            )
            
        self.stdout.write(f"Created Margin Position: {pos.symbol} (ID: {pos.id}) with 3 transfers.")
