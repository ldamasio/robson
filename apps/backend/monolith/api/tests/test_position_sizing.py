"""
Tests for Position Sizing Calculator.

These tests verify the 1% risk rule implementation.
"""

from decimal import Decimal
from django.test import TestCase

from api.application.risk import (
    PositionSizingCalculator,
    PositionSizeResult,
    calculate_position_size,
)


class TestPositionSizingCalculator(TestCase):
    """Tests for PositionSizingCalculator class."""
    
    def setUp(self):
        """Set up test fixtures."""
        self.calculator = PositionSizingCalculator()
    
    def test_basic_calculation(self):
        """Test basic position sizing with 1% risk rule."""
        result = self.calculator.calculate(
            capital=Decimal("1000"),
            entry_price=Decimal("90000"),
            stop_loss_percent=Decimal("2"),
            take_profit_percent=Decimal("4"),
            side="BUY",
        )
        
        # With $1000 capital and 1% risk = $10 max loss
        # Entry: $90,000, Stop: $88,200 (2% below)
        # Distance: $1,800
        # Position = $10 / $1,800 = 0.00556 BTC
        
        self.assertIsInstance(result, PositionSizeResult)
        self.assertGreater(result.quantity, Decimal("0"))
        self.assertEqual(result.risk_percent, Decimal("1.00"))
        self.assertFalse(result.is_capped)
    
    def test_one_percent_rule_enforcement(self):
        """Verify that risk never exceeds 1% of capital."""
        result = self.calculator.calculate(
            capital=Decimal("10000"),
            entry_price=Decimal("50000"),
            stop_loss_percent=Decimal("2"),
            side="BUY",
        )
        
        # Risk should be exactly 1% of capital = $100
        self.assertLessEqual(result.risk_amount, Decimal("100.01"))
        self.assertLessEqual(result.risk_percent, Decimal("1.01"))
    
    def test_position_size_capping(self):
        """Test that position is capped at max_position_percent."""
        # With very tight stop loss, position could be huge
        # It should be capped at 50% of capital
        result = self.calculator.calculate(
            capital=Decimal("1000"),
            entry_price=Decimal("100"),
            stop_loss_percent=Decimal("0.1"),  # Very tight stop
            side="BUY",
        )
        
        # Position should be capped
        self.assertTrue(result.is_capped)
        self.assertLessEqual(result.position_percent, Decimal("50.01"))
    
    def test_stop_loss_below_entry_for_buy(self):
        """Stop loss must be below entry for BUY orders."""
        result = self.calculator.calculate(
            capital=Decimal("1000"),
            entry_price=Decimal("100"),
            stop_loss_percent=Decimal("5"),
            side="BUY",
        )
        
        self.assertLess(result.stop_loss_price, result.entry_price)
    
    def test_stop_loss_above_entry_for_sell(self):
        """Stop loss must be above entry for SELL orders."""
        result = self.calculator.calculate(
            capital=Decimal("1000"),
            entry_price=Decimal("100"),
            stop_loss_percent=Decimal("5"),
            side="SELL",
        )
        
        self.assertGreater(result.stop_loss_price, result.entry_price)
    
    def test_risk_reward_ratio(self):
        """Test risk/reward ratio calculation."""
        result = self.calculator.calculate(
            capital=Decimal("1000"),
            entry_price=Decimal("100"),
            stop_loss_percent=Decimal("2"),
            take_profit_percent=Decimal("4"),
            side="BUY",
        )
        
        # Stop: 2%, Target: 4% => R:R = 2:1
        self.assertEqual(result.risk_reward_ratio, Decimal("2.00"))
    
    def test_invalid_capital_raises_error(self):
        """Negative or zero capital should raise ValueError."""
        with self.assertRaises(ValueError):
            self.calculator.calculate(
                capital=Decimal("0"),
                entry_price=Decimal("100"),
            )
        
        with self.assertRaises(ValueError):
            self.calculator.calculate(
                capital=Decimal("-1000"),
                entry_price=Decimal("100"),
            )
    
    def test_invalid_entry_price_raises_error(self):
        """Negative or zero entry price should raise ValueError."""
        with self.assertRaises(ValueError):
            self.calculator.calculate(
                capital=Decimal("1000"),
                entry_price=Decimal("0"),
            )
    
    def test_invalid_side_raises_error(self):
        """Invalid side should raise ValueError."""
        with self.assertRaises(ValueError):
            self.calculator.calculate(
                capital=Decimal("1000"),
                entry_price=Decimal("100"),
                side="INVALID",
            )
    
    def test_invalid_stop_loss_direction_raises_error(self):
        """Stop loss in wrong direction should raise ValueError."""
        # Stop loss above entry for BUY
        with self.assertRaises(ValueError):
            self.calculator.calculate(
                capital=Decimal("1000"),
                entry_price=Decimal("100"),
                stop_loss_price=Decimal("110"),  # Above entry
                side="BUY",
            )
        
        # Stop loss below entry for SELL
        with self.assertRaises(ValueError):
            self.calculator.calculate(
                capital=Decimal("1000"),
                entry_price=Decimal("100"),
                stop_loss_price=Decimal("90"),  # Below entry
                side="SELL",
            )
    
    def test_calculate_from_risk_config(self):
        """Test calculation from risk_config dictionary."""
        risk_config = {
            "max_risk_per_trade_percent": 1,
            "stop_loss_percent": 2,
            "take_profit_percent": 4,
            "max_position_size_percent": 50,
        }
        
        result = self.calculator.calculate_from_risk_config(
            capital=Decimal("1000"),
            entry_price=Decimal("90000"),
            risk_config=risk_config,
            side="BUY",
        )
        
        self.assertEqual(result.risk_percent, Decimal("1.00"))
        self.assertEqual(result.stop_distance_percent, Decimal("2.00"))
        self.assertEqual(result.target_distance_percent, Decimal("4.00"))
    
    def test_to_dict(self):
        """Test PositionSizeResult to_dict method."""
        result = self.calculator.calculate(
            capital=Decimal("1000"),
            entry_price=Decimal("100"),
            stop_loss_percent=Decimal("2"),
            take_profit_percent=Decimal("4"),
            side="BUY",
        )
        
        d = result.to_dict()
        
        self.assertIn("quantity", d)
        self.assertIn("position_value", d)
        self.assertIn("risk_amount", d)
        self.assertIn("risk_percent", d)
        self.assertIn("position_percent", d)
        self.assertIn("is_capped", d)
        self.assertIn("entry_price", d)
        self.assertIn("stop_loss_price", d)
        self.assertIn("take_profit_price", d)
        self.assertIn("risk_reward_ratio", d)


class TestCalculatePositionSizeFunction(TestCase):
    """Tests for the convenience function."""
    
    def test_convenience_function(self):
        """Test calculate_position_size convenience function."""
        result = calculate_position_size(
            capital=Decimal("1000"),
            entry_price=Decimal("90000"),
            stop_loss_percent=Decimal("2"),
            take_profit_percent=Decimal("4"),
        )
        
        self.assertIsInstance(result, PositionSizeResult)
        self.assertEqual(result.risk_percent, Decimal("1.00"))


class TestRealWorldScenarios(TestCase):
    """Test real-world trading scenarios."""
    
    def test_btc_trade_scenario(self):
        """
        Real BTC trading scenario.
        
        Capital: $30 USDC (like our first trade)
        Entry: ~$89,000
        Stop: 2% below
        Target: 4% above
        """
        result = calculate_position_size(
            capital=Decimal("30"),
            entry_price=Decimal("89000"),
            stop_loss_percent=Decimal("2"),
            take_profit_percent=Decimal("4"),
            max_risk_percent=Decimal("1"),
        )
        
        # With $30 capital, 1% risk = $0.30 max loss
        # Stop distance: $1,780 (2% of $89,000)
        # Position = $0.30 / $1,780 = 0.00017 BTC
        
        self.assertGreater(result.quantity, Decimal("0"))
        self.assertLessEqual(result.risk_amount, Decimal("0.31"))
        
        # Verify if stopped out, we only lose ~1%
        simulated_loss = result.quantity * (result.entry_price - result.stop_loss_price)
        self.assertLessEqual(simulated_loss, Decimal("0.31"))
    
    def test_aggressive_trader_scenario(self):
        """
        Aggressive trader with 2% risk per trade.
        """
        calculator = PositionSizingCalculator(
            max_risk_percent=Decimal("2.0"),
            max_position_percent=Decimal("80.0"),
        )
        
        result = calculator.calculate(
            capital=Decimal("10000"),
            entry_price=Decimal("50000"),
            stop_loss_percent=Decimal("3"),
            side="BUY",
        )
        
        # 2% of $10,000 = $200 max risk
        self.assertLessEqual(result.risk_amount, Decimal("200.01"))
        self.assertLessEqual(result.risk_percent, Decimal("2.01"))
    
    def test_conservative_trader_scenario(self):
        """
        Conservative trader with 0.5% risk per trade.
        """
        calculator = PositionSizingCalculator(
            max_risk_percent=Decimal("0.5"),
            max_position_percent=Decimal("20.0"),
        )
        
        result = calculator.calculate(
            capital=Decimal("10000"),
            entry_price=Decimal("50000"),
            stop_loss_percent=Decimal("2"),
            side="BUY",
        )
        
        # 0.5% of $10,000 = $50 max risk
        self.assertLessEqual(result.risk_amount, Decimal("50.01"))
        self.assertLessEqual(result.risk_percent, Decimal("0.51"))

