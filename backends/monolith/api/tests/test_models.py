class TestStrategyModel(BaseTestCase):
    """Tests for Strategy model"""
    
    def test_strategy_creation(self):
        """Test basic strategy creation"""
        strategy = Strategy.objects.create(
            client=self.client,
            name="New Strategy",
            description="Test strategy",
            config={"test": "value"}
        )
        
        self.assertEqual(strategy.name, "New Strategy")
        self.assertTrue(strategy.is_active)
        self.assertEqual(strategy.config["test"], "value")
        self.assertEqual(strategy.total_trades, 0)
        self.assertEqual(strategy.win_rate, 0)
    
    def test_strategy_config_methods(self):
        """Test configuration methods"""
        # Test get_config_value
        self.assertEqual(self.strategy.get_config_value("sma_fast"), 10)
        self.assertEqual(self.strategy.get_config_value("nonexistent", "default"), "default")
        
        # Test set_config_value
        self.strategy.set_config_value("new_param", "new_value")
        self.strategy.refresh_from_db()
        self.assertEqual(self.strategy.get_config_value("new_param"), "new_value")
    
    def test_strategy_performance_tracking(self):
        """Test performance tracking"""
        # Simulate trades
        self.strategy.update_performance(Decimal('100.0'), is_winner=True)
        self.strategy.update_performance(Decimal('-50.0'), is_winner=False)
        
        self.assertEqual(self.strategy.total_trades, 2)
        self.assertEqual(self.strategy.winning_trades, 1)
        self.assertEqual(self.strategy.total_pnl, Decimal('50.0'))
        self.assertEqual(self.strategy.win_rate, 50.0)
        self.assertEqual(self.strategy.average_pnl_per_trade, Decimal('25.0'))

class TestOrderModel(BaseTestCase):
    """Tests for Order model"""
    
    def test_order_creation(self):
        """Test basic order creation"""
        order = Order.objects.create(
            client=self.client,
            symbol=self.symbol,
            strategy=self.strategy,
            side='BUY',
            order_type='MARKET',
            quantity=Decimal('0.1'),
            price=Decimal('50000.0')
        )
        
        self.assertEqual(order.side, 'BUY')
        self.assertEqual(order.quantity, Decimal('0.1'))
        self.assertEqual(order.status, 'PENDING')
        self.assertEqual(order.remaining_quantity, Decimal('0.1'))
        self.assertTrue(order.is_active)
        self.assertFalse(order.is_filled)
    
    def test_order_str_method(self):
        """Test Order __str__ method"""
        order = Order.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            price=Decimal('50000.0')
        )
        
        expected = "BUY 0.1 BTCUSDT @ 50000.0"
        self.assertEqual(str(order), expected)
    
    def test_order_fill_tracking(self):
        """Test order fill tracking"""
        order = Order.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            price=Decimal('50000.0')
        )
        
        # Mark as partially filled
        order.mark_as_filled(Decimal('49000.0'), Decimal('0.05'))
        
        self.assertEqual(order.filled_quantity, Decimal('0.05'))
        self.assertEqual(order.avg_fill_price, Decimal('49000.0'))
        self.assertEqual(order.status, 'PARTIALLY_FILLED')
        self.assertEqual(order.remaining_quantity, Decimal('0.05'))
        self.assertEqual(order.fill_percentage, 50.0)
        
        # Fill completely
        order.mark_as_filled(Decimal('49000.0'), Decimal('0.1'))
        
        self.assertEqual(order.status, 'FILLED')
        self.assertTrue(order.is_filled)
        self.assertFalse(order.is_active)
        self.assertIsNotNone(order.filled_at)# api/tests/test_models.py
"""
Tests for refactored models.
Ensures refactoring doesn't break existing functionality.
"""

from django.test import TestCase
from django.core.exceptions import ValidationError
from django.utils import timezone
from decimal import Decimal
from clients.models import Client
from api.models import Symbol, Strategy, Order, Operation, Position, Trade

class BaseTestCase(TestCase):
    """Base class for model tests"""
    
    def setUp(self):
        """Common setup for all tests"""
        self.client = Client.objects.create(name="Test Client")
        
        self.symbol = Symbol.objects.create(
            client=self.client,
            name="BTCUSDT",
            description="Bitcoin/USDT pair",
            base_asset="BTC",
            quote_asset="USDT"
        )
        
        self.strategy = Strategy.objects.create(
            client=self.client,
            name="Test Strategy",
            description="Strategy for testing",
            config={
                "sma_fast": 10,
                "sma_slow": 30,
                "rsi_period": 14
            },
            risk_config={
                "max_position_size": 0.02,
                "stop_loss_pct": 0.03
            }
        )

class TestSymbolModel(BaseTestCase):
    """Tests for Symbol model"""
    
    def test_symbol_creation(self):
        """Test basic symbol creation"""
        symbol = Symbol.objects.create(
            client=self.client,
            name="ETHUSDT",
            description="Ethereum/USDT pair",
            base_asset="ETH",
            quote_asset="USDT"
        )
        
        self.assertEqual(symbol.name, "ETHUSDT")
        self.assertEqual(symbol.base_asset, "ETH")
        self.assertEqual(symbol.quote_asset, "USDT")
        self.assertTrue(symbol.is_active)
        self.assertIsNotNone(symbol.created_at)
    
    def test_symbol_str_method(self):
        """Test Symbol __str__ method"""
        expected = f"BTCUSDT ({self.client.name})"
        self.assertEqual(str(self.symbol), expected)
    
    def test_symbol_display_properties(self):
        """Test display properties"""
        self.assertEqual(self.symbol.display_name, "BTCUSDT")
        self.assertEqual(self.symbol.pair_display, "BTC/USDT")
    
    def test_symbol_name_uppercase(self):
        """Test if symbol name is converted to uppercase"""
        symbol = Symbol.objects.create(
            client=self.client,
            name="btcusdt",  # lowercase
            description="Test",
            base_asset="BTC",
            quote_asset="USDT"
        )
        
        # Call clean() manually (Django doesn't call automatically in tests)
        symbol.clean()
        symbol.save()
        
        self.assertEqual(symbol.name, "BTCUSDT")
    
    def test_quantity_validation(self):
        """Test quantity validation"""
        # Valid quantity
        self.assertTrue(self.symbol.is_quantity_valid(Decimal('0.001')))
        
        # Too small quantity
        self.assertFalse(self.symbol.is_quantity_valid(Decimal('0.000000001')))
        
        # Set max_qty and test
        self.symbol.max_qty = Decimal('100.0')
        self.symbol.save()
        
        # Too large quantity
        self.assertFalse(self.symbol.is_quantity_valid(Decimal('1000.0')))

    def test_order_validation(self):
        """Test order validations"""
        # Order with invalid stop loss for buy order
        order = Order(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            price=Decimal('50000.0'),
            stop_loss_price=Decimal('55000.0')  # Stop loss higher than price = invalid
        )
        
        with self.assertRaises(ValidationError):
            order.clean()
    
    def test_order_pnl_calculation(self):
        """Test P&L calculation"""
        order = Order.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            price=Decimal('50000.0')
        )
        
        # Mark as filled
        order.mark_as_filled(Decimal('49000.0'))
        
        # Calculate P&L with higher current price
        pnl = order.calculate_pnl(Decimal('51000.0'))
        expected = (Decimal('51000.0') - Decimal('49000.0')) * Decimal('0.1')
        self.assertEqual(pnl, expected)

class TestOperationModel(BaseTestCase):
    """Tests for Operation model"""
    
    def test_operation_creation(self):
        """Test basic operation creation"""
        operation = Operation.objects.create(
            client=self.client,
            strategy=self.strategy,
            symbol=self.symbol,
            side='BUY',
            stop_gain_percent=Decimal('5.0'),
            stop_loss_percent=Decimal('2.0')
        )
        
        self.assertEqual(operation.side, 'BUY')
        self.assertEqual(operation.status, 'PLANNED')
        self.assertEqual(operation.stop_gain_percent, Decimal('5.0'))
        self.assertFalse(operation.is_complete)
    
    def test_operation_with_orders(self):
        """Test operation with associated orders"""
        operation = Operation.objects.create(
            client=self.client,
            strategy=self.strategy,
            symbol=self.symbol,
            side='BUY'
        )
        
        # Create entry order
        entry_order = Order.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            price=Decimal('50000.0')
        )
        entry_order.mark_as_filled(Decimal('49000.0'))
        
        # Create exit order
        exit_order = Order.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='SELL',
            quantity=Decimal('0.1'),
            price=Decimal('52000.0')
        )
        exit_order.mark_as_filled(Decimal('52000.0'))
        
        # Associate orders with operation
        operation.entry_orders.add(entry_order)
        operation.exit_orders.add(exit_order)
        
        # Test calculated properties
        self.assertEqual(operation.total_entry_quantity, Decimal('0.1'))
        self.assertEqual(operation.total_exit_quantity, Decimal('0.1'))
        self.assertEqual(operation.average_entry_price, Decimal('49000.0'))
        self.assertEqual(operation.average_exit_price, Decimal('52000.0'))
    
    def test_operation_pnl_calculation(self):
        """Test operation P&L calculation"""
        operation = Operation.objects.create(
            client=self.client,
            strategy=self.strategy,
            symbol=self.symbol,
            side='BUY'
        )
        
        # Simulate entry
        entry_order = Order.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1')
        )
        entry_order.mark_as_filled(Decimal('50000.0'))
        operation.entry_orders.add(entry_order)
        
        # Calculate unrealized P&L
        unrealized = operation.calculate_unrealized_pnl(Decimal('55000.0'))
        expected = (Decimal('55000.0') - Decimal('50000.0')) * Decimal('0.1')
        self.assertEqual(unrealized, expected)

class TestPositionModel(BaseTestCase):
    """Tests for Position model"""
    
    def test_position_creation(self):
        """Test basic position creation"""
        position = Position.objects.create(
            client=self.client,
            symbol=self.symbol,
            strategy=self.strategy,
            side='BUY',
            quantity=Decimal('0.1'),
            average_price=Decimal('50000.0')
        )
        
        self.assertEqual(position.side, 'BUY')
        self.assertTrue(position.is_long)
        self.assertFalse(position.is_short)
        self.assertTrue(position.is_open)
        self.assertEqual(position.cost_basis, Decimal('5000.0'))
    
    def test_position_pnl_update(self):
        """Test unrealized P&L update"""
        position = Position.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            average_price=Decimal('50000.0')
        )
        
        # Update P&L with current price
        position.update_unrealized_pnl(Decimal('55000.0'))
        
        expected_pnl = (Decimal('55000.0') - Decimal('50000.0')) * Decimal('0.1')
        self.assertEqual(position.unrealized_pnl, expected_pnl)
    
    def test_position_add_order(self):
        """Test adding order to position"""
        position = Position.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            average_price=Decimal('50000.0')
        )
        
        # Create order that increases position
        order = Order.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1')
        )
        order.mark_as_filled(Decimal('52000.0'))
        
        # Add order to position
        position.add_order(order)
        
        # Check if average price was recalculated
        expected_avg = (Decimal('50000.0') + Decimal('52000.0')) / 2
        self.assertEqual(position.average_price, expected_avg)
        self.assertEqual(position.quantity, Decimal('0.2'))
    
    def test_position_close(self):
        """Test position closing"""
        position = Position.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            average_price=Decimal('50000.0')
        )
        
        # Close position
        final_pnl = position.close_position(Decimal('55000.0'))
        
        self.assertEqual(position.status, 'CLOSED')
        self.assertIsNotNone(position.closed_at)
        expected_pnl = (Decimal('55000.0') - Decimal('50000.0')) * Decimal('0.1')
        self.assertEqual(final_pnl, expected_pnl)

class TestTradeModel(BaseTestCase):
    """Tests for Trade model"""
    
    def test_trade_creation(self):
        """Test basic trade creation"""
        trade = Trade.objects.create(
            client=self.client,
            symbol=self.symbol,
            strategy=self.strategy,
            side='BUY',
            quantity=Decimal('0.1'),
            entry_price=Decimal('50000.0'),
            exit_price=Decimal('55000.0'),
            entry_time=timezone.now(),
            exit_time=timezone.now()
        )
        
        self.assertEqual(trade.side, 'BUY')
        self.assertTrue(trade.is_closed)
        self.assertTrue(trade.is_winner)
        self.assertIsNotNone(trade.duration)
    
    def test_trade_pnl_calculation(self):
        """Test automatic P&L calculation on save"""
        entry_time = timezone.now()
        exit_time = entry_time + timezone.timedelta(hours=2)
        
        trade = Trade.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            entry_price=Decimal('50000.0'),
            exit_price=Decimal('55000.0'),
            entry_fee=Decimal('5.0'),
            exit_fee=Decimal('5.0'),
            entry_time=entry_time,
            exit_time=exit_time
        )
        
        # P&L should be calculated automatically on save
        expected_gross = (Decimal('55000.0') - Decimal('50000.0')) * Decimal('0.1')
        expected_net = expected_gross - Decimal('10.0')  # Discount fees
        
        self.assertEqual(trade.pnl, expected_net)
        self.assertEqual(trade.total_fees, Decimal('10.0'))
        self.assertEqual(trade.duration_hours, 2.0)
        
        # P&L percentage
        cost_basis = Decimal('50000.0') * Decimal('0.1')
        expected_pct = (expected_net / cost_basis) * 100
        self.assertEqual(trade.pnl_percentage, expected_pct)
    
    def test_trade_properties(self):
        """Test trade properties"""
        # Winning trade
        winning_trade = Trade.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            entry_price=Decimal('50000.0'),
            exit_price=Decimal('55000.0'),
            entry_time=timezone.now()
        )
        
        self.assertTrue(winning_trade.is_winner)
        self.assertTrue(winning_trade.is_closed)
        
        # Losing trade
        losing_trade = Trade.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            entry_price=Decimal('50000.0'),
            exit_price=Decimal('45000.0'),
            entry_time=timezone.now()
        )
        
        self.assertFalse(losing_trade.is_winner)

class TestMixinsAndBaseClasses(BaseTestCase):
    """Tests for mixins and base classes"""
    
    def test_timestamp_mixin(self):
        """Test TimestampMixin"""
        symbol = Symbol.objects.create(
            client=self.client,
            name="TESTUSDT",
            description="Test",
            base_asset="TEST",
            quote_asset="USDT"
        )
        
        self.assertIsNotNone(symbol.created_at)
        self.assertIsNotNone(symbol.updated_at)
        self.assertIsNotNone(symbol.age)
        self.assertIsNotNone(symbol.time_since_last_update)
    
    def test_tenant_mixin(self):
        """Test TenantMixin"""
        symbol = Symbol.objects.create(
            client=self.client,
            name="TESTUSDT",
            description="Test",
            base_asset="TEST",
            quote_asset="USDT"
        )
        
        self.assertEqual(symbol.client, self.client)
        self.assertEqual(symbol.client_name, self.client.name)
    
    def test_status_mixin(self):
        """Test StatusMixin"""
        symbol = Symbol.objects.create(
            client=self.client,
            name="TESTUSDT",
            description="Test",
            base_asset="TEST", 
            quote_asset="USDT"
        )
        
        self.assertTrue(symbol.is_active)
        self.assertEqual(symbol.status_icon, '✅')
        
        symbol.is_active = False
        symbol.save()
        
        self.assertEqual(symbol.status_icon, '❌')
    
    def test_managers(self):
        """Test custom managers"""
        # Create active and inactive symbols
        active_symbol = Symbol.objects.create(
            client=self.client,
            name="ACTIVE",
            description="Active symbol",
            base_asset="ACT",
            quote_asset="USDT",
            is_active=True
        )
        
        inactive_symbol = Symbol.objects.create(
            client=self.client,
            name="INACTIVE",
            description="Inactive symbol",
            base_asset="INACT",
            quote_asset="USDT",
            is_active=False
        )
        
        # Test ActiveManager
        active_symbols = Symbol.active.all()
        self.assertIn(active_symbol, active_symbols)
        self.assertNotIn(inactive_symbol, active_symbols)
        
        # Test TenantManager
        client_symbols = Symbol.objects.for_client(self.client.id)
        self.assertEqual(client_symbols.count(), 3)  # Including self.symbol from setUp
        
        active_client_symbols = Symbol.objects.active_for_client(self.client.id)
        self.assertEqual(active_client_symbols.count(), 2)  # Only active ones

class TestModelChoices(TestCase):
    """Tests for ModelChoices"""
    
    def test_choices_availability(self):
        """Test if all choices are available"""
        from api.models.base import ModelChoices
        
        # Test ORDER_SIDES
        self.assertIn(('BUY', 'Buy'), ModelChoices.ORDER_SIDES)
        self.assertIn(('SELL', 'Sell'), ModelChoices.ORDER_SIDES)
        
        # Test ORDER_STATUS
        self.assertIn(('PENDING', 'Pending'), ModelChoices.ORDER_STATUS)
        self.assertIn(('FILLED', 'Filled'), ModelChoices.ORDER_STATUS)
        
        # Test TIMEFRAMES
        self.assertIn(('1m', '1 Minute'), ModelChoices.TIMEFRAMES)
        self.assertIn(('1d', '1 Day'), ModelChoices.TIMEFRAMES)
        
        # Test ORDER_TYPES
        self.assertIn(('MARKET', 'Market'), ModelChoices.ORDER_TYPES)
        self.assertIn(('LIMIT', 'Limit'), ModelChoices.ORDER_TYPES)

class TestOrderModel(BaseTestCase):
    """Testes para o model Order"""
    
    def test_order_creation(self):
        """Testa criação básica de ordem"""
        order = Order.objects.create(
            client=self.client,
            symbol=self.symbol,
            strategy=self.strategy,
            side='BUY',
            order_type='MARKET',
            quantity=Decimal('0.1'),
            price=Decimal('50000.0')
        )
        
        self.assertEqual(order.side, 'BUY')
        self.assertEqual(order.quantity, Decimal('0.1'))
        self.assertEqual(order.status, 'PENDING')
        self.assertEqual(order.remaining_quantity, Decimal('0.1'))
        self.assertTrue(order.is_active)
        self.assertFalse(order.is_filled)
    
    def test_order_str_method(self):
        """Testa método __str__ da Order"""
        order = Order.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            price=Decimal('50000.0')
        )
        
        expected = "BUY 0.1 BTCUSDT @ 50000.0"
        self.assertEqual(str(order), expected)
    
    def test_order_fill_tracking(self):
        """Testa rastreamento de execução da ordem"""
        order = Order.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            price=Decimal('50000.0')
        )
        
        # Marcar como executada parcialmente
        order.mark_as_filled(Decimal('49000.0'), Decimal('0.05'))
        
        self.assertEqual(order.filled_quantity, Decimal('0.05'))
        self.assertEqual(order.avg_fill_price, Decimal('49000.0'))
        self.assertEqual(order.status, 'PARTIALLY_FILLED')
        self.assertEqual(order.remaining_quantity, Decimal('0.05'))
        self.assertEqual(order.fill_percentage, 50.0)
        
        # Executar completamente
        order.mark_as_filled(Decimal('49000.0'), Decimal('0.1'))
        
        self.assertEqual(order.status, 'FILLED')
        self.assertTrue(order.is_filled)
        self.assertFalse(order.is_active)
        self.assertIsNotNone(order.filled_at)
    
    def test_order_validation(self):
        """Testa validações da ordem"""
        # Ordem com stop loss inválido para compra
        order = Order(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            price=Decimal('50000.0'),
            stop_loss_price=Decimal('55000.0')  # Stop loss maior que preço = inválido
        )
        
        with self.assertRaises(ValidationError):
            order.clean()
    
    def test_order_pnl_calculation(self):
        """Testa cálculo de P&L"""
        order = Order.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            price=Decimal('50000.0')
        )
        
        # Marcar como executada
        order.mark_as_filled(Decimal('49000.0'))
        
        # Calcular P&L com preço atual maior
        pnl = order.calculate_pnl(Decimal('51000.0'))
        expected = (Decimal('51000.0') - Decimal('49000.0')) * Decimal('0.1')
        self.assertEqual(pnl, expected)

class TestOperationModel(BaseTestCase):
    """Testes para o model Operation"""
    
    def test_operation_creation(self):
        """Testa criação básica de operação"""
        operation = Operation.objects.create(
            client=self.client,
            strategy=self.strategy,
            symbol=self.symbol,
            side='BUY',
            stop_gain_percent=Decimal('5.0'),
            stop_loss_percent=Decimal('2.0')
        )
        
        self.assertEqual(operation.side, 'BUY')
        self.assertEqual(operation.status, 'PLANNED')
        self.assertEqual(operation.stop_gain_percent, Decimal('5.0'))
        self.assertFalse(operation.is_complete)
    
    def test_operation_with_orders(self):
        """Testa operação com ordens associadas"""
        operation = Operation.objects.create(
            client=self.client,
            strategy=self.strategy,
            symbol=self.symbol,
            side='BUY'
        )
        
        # Criar ordem de entrada
        entry_order = Order.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            price=Decimal('50000.0')
        )
        entry_order.mark_as_filled(Decimal('49000.0'))
        
        # Criar ordem de saída
        exit_order = Order.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='SELL',
            quantity=Decimal('0.1'),
            price=Decimal('52000.0')
        )
        exit_order.mark_as_filled(Decimal('52000.0'))
        
        # Associar ordens à operação
        operation.entry_orders.add(entry_order)
        operation.exit_orders.add(exit_order)
        
        # Testar propriedades calculadas
        self.assertEqual(operation.total_entry_quantity, Decimal('0.1'))
        self.assertEqual(operation.total_exit_quantity, Decimal('0.1'))
        self.assertEqual(operation.average_entry_price, Decimal('49000.0'))
        self.assertEqual(operation.average_exit_price, Decimal('52000.0'))
    
    def test_operation_pnl_calculation(self):
        """Testa cálculo de P&L da operação"""
        operation = Operation.objects.create(
            client=self.client,
            strategy=self.strategy,
            symbol=self.symbol,
            side='BUY'
        )
        
        # Simular entrada
        entry_order = Order.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1')
        )
        entry_order.mark_as_filled(Decimal('50000.0'))
        operation.entry_orders.add(entry_order)
        
        # Calcular P&L não realizado
        unrealized = operation.calculate_unrealized_pnl(Decimal('55000.0'))
        expected = (Decimal('55000.0') - Decimal('50000.0')) * Decimal('0.1')
        self.assertEqual(unrealized, expected)

class TestPositionModel(BaseTestCase):
    """Testes para o model Position"""
    
    def test_position_creation(self):
        """Testa criação básica de posição"""
        position = Position.objects.create(
            client=self.client,
            symbol=self.symbol,
            strategy=self.strategy,
            side='BUY',
            quantity=Decimal('0.1'),
            average_price=Decimal('50000.0')
        )
        
        self.assertEqual(position.side, 'BUY')
        self.assertTrue(position.is_long)
        self.assertFalse(position.is_short)
        self.assertTrue(position.is_open)
        self.assertEqual(position.cost_basis, Decimal('5000.0'))
    
    def test_position_pnl_update(self):
        """Testa atualização de P&L não realizado"""
        position = Position.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            average_price=Decimal('50000.0')
        )
        
        # Atualizar P&L com preço atual
        position.update_unrealized_pnl(Decimal('55000.0'))
        
        expected_pnl = (Decimal('55000.0') - Decimal('50000.0')) * Decimal('0.1')
        self.assertEqual(position.unrealized_pnl, expected_pnl)
    
    def test_position_add_order(self):
        """Testa adição de ordem à posição"""
        position = Position.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            average_price=Decimal('50000.0')
        )
        
        # Criar ordem que aumenta a posição
        order = Order.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1')
        )
        order.mark_as_filled(Decimal('52000.0'))
        
        # Adicionar ordem à posição
        position.add_order(order)
        
        # Verificar se preço médio foi recalculado
        expected_avg = (Decimal('50000.0') + Decimal('52000.0')) / 2
        self.assertEqual(position.average_price, expected_avg)
        self.assertEqual(position.quantity, Decimal('0.2'))
    
    def test_position_close(self):
        """Testa fechamento de posição"""
        position = Position.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            average_price=Decimal('50000.0')
        )
        
        # Fechar posição
        final_pnl = position.close_position(Decimal('55000.0'))
        
        self.assertEqual(position.status, 'CLOSED')
        self.assertIsNotNone(position.closed_at)
        expected_pnl = (Decimal('55000.0') - Decimal('50000.0')) * Decimal('0.1')
        self.assertEqual(final_pnl, expected_pnl)

class TestTradeModel(BaseTestCase):
    """Testes para o model Trade"""
    
    def test_trade_creation(self):
        """Testa criação básica de trade"""
        trade = Trade.objects.create(
            client=self.client,
            symbol=self.symbol,
            strategy=self.strategy,
            side='BUY',
            quantity=Decimal('0.1'),
            entry_price=Decimal('50000.0'),
            exit_price=Decimal('55000.0'),
            entry_time=timezone.now(),
            exit_time=timezone.now()
        )
        
        self.assertEqual(trade.side, 'BUY')
        self.assertTrue(trade.is_closed)
        self.assertTrue(trade.is_winner)
        self.assertIsNotNone(trade.duration)
    
    def test_trade_pnl_calculation(self):
        """Testa cálculo automático de P&L no save"""
        entry_time = timezone.now()
        exit_time = entry_time + timezone.timedelta(hours=2)
        
        trade = Trade.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            entry_price=Decimal('50000.0'),
            exit_price=Decimal('55000.0'),
            entry_fee=Decimal('5.0'),
            exit_fee=Decimal('5.0'),
            entry_time=entry_time,
            exit_time=exit_time
        )
        
        # P&L deve ser calculado automaticamente no save
        expected_gross = (Decimal('55000.0') - Decimal('50000.0')) * Decimal('0.1')
        expected_net = expected_gross - Decimal('10.0')  # Descontando fees
        
        self.assertEqual(trade.pnl, expected_net)
        self.assertEqual(trade.total_fees, Decimal('10.0'))
        self.assertEqual(trade.duration_hours, 2.0)
        
        # P&L percentage
        cost_basis = Decimal('50000.0') * Decimal('0.1')
        expected_pct = (expected_net / cost_basis) * 100
        self.assertEqual(trade.pnl_percentage, expected_pct)
    
    def test_trade_properties(self):
        """Testa propriedades do trade"""
        # Trade vencedor
        winning_trade = Trade.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            entry_price=Decimal('50000.0'),
            exit_price=Decimal('55000.0'),
            entry_time=timezone.now()
        )
        
        self.assertTrue(winning_trade.is_winner)
        self.assertTrue(winning_trade.is_closed)
        
        # Trade perdedor
        losing_trade = Trade.objects.create(
            client=self.client,
            symbol=self.symbol,
            side='BUY',
            quantity=Decimal('0.1'),
            entry_price=Decimal('50000.0'),
            exit_price=Decimal('45000.0'),
            entry_time=timezone.now()
        )
        
        self.assertFalse(losing_trade.is_winner)

class TestMixinsAndBaseClasses(BaseTestCase):
    """Testes para mixins e classes base"""
    
    def test_timestamp_mixin(self):
        """Testa TimestampMixin"""
        symbol = Symbol.objects.create(
            client=self.client,
            name="TESTUSDT",
            description="Test",
            base_asset="TEST",
            quote_asset="USDT"
        )
        
        self.assertIsNotNone(symbol.created_at)
        self.assertIsNotNone(symbol.updated_at)
        self.assertIsNotNone(symbol.age)
        self.assertIsNotNone(symbol.last_updated_ago)
    
    def test_tenant_mixin(self):
        """Testa TenantMixin"""
        symbol = Symbol.objects.create(
            client=self.client,
            name="TESTUSDT",
            description="Test",
            base_asset="TEST",
            quote_asset="USDT"
        )
        
        self.assertEqual(symbol.client, self.client)
        self.assertEqual(symbol.client_name, self.client.name)
    
    def test_status_mixin(self):
        """Testa StatusMixin"""
        symbol = Symbol.objects.create(
            client=self.client,
            name="TESTUSDT",
            description="Test",
            base_asset="TEST", 
            quote_asset="USDT"
        )
        
        self.assertTrue(symbol.is_active)
        self.assertEqual(symbol.status_icon, '✅')
        
        symbol.is_active = False
        symbol.save()
        
        self.assertEqual(symbol.status_icon, '❌')
    
    def test_managers(self):
        """Testa managers customizados"""
        # Criar símbolos ativos e inativos
        active_symbol = Symbol.objects.create(
            client=self.client,
            name="ACTIVE",
            description="Active symbol",
            base_asset="ACT",
            quote_asset="USDT",
            is_active=True
        )
        
        inactive_symbol = Symbol.objects.create(
            client=self.client,
            name="INACTIVE",
            description="Inactive symbol",
            base_asset="INACT",
            quote_asset="USDT",
            is_active=False
        )
        
        # Testar ActiveManager
        active_symbols = Symbol.active.all()
        self.assertIn(active_symbol, active_symbols)
        self.assertNotIn(inactive_symbol, active_symbols)
        
        # Testar TenantManager
        client_symbols = Symbol.objects.for_client(self.client.id)
        self.assertEqual(client_symbols.count(), 3)  # Incluindo self.symbol do setUp
        
        active_client_symbols = Symbol.objects.active_for_client(self.client.id)
        self.assertEqual(active_client_symbols.count(), 2)  # Apenas os ativos

class TestModelChoices(TestCase):
    """Testes para ModelChoices"""
    
    def test_choices_availability(self):
        """Testa se todas as choices estão disponíveis"""
        from api.models.base import ModelChoices
        
        # Testar ORDER_SIDES
        self.assertIn(('BUY', 'Buy'), ModelChoices.ORDER_SIDES)
        self.assertIn(('SELL', 'Sell'), ModelChoices.ORDER_SIDES)
        
        # Testar ORDER_STATUS
        self.assertIn(('PENDING', 'Pending'), ModelChoices.ORDER_STATUS)
        self.assertIn(('FILLED', 'Filled'), ModelChoices.ORDER_STATUS)
        
        # Testar TIMEFRAMES
        self.assertIn(('1m', '1 Minute'), ModelChoices.TIMEFRAMES)
        self.assertIn(('1d', '1 Day'), ModelChoices.TIMEFRAMES)
        
        # Testar ORDER_TYPES
        self.assertIn(('MARKET', 'Market'), ModelChoices.ORDER_TYPES)
        self.assertIn(('LIMIT', 'Limit'), ModelChoices.ORDER_TYPES)
        
        
    
