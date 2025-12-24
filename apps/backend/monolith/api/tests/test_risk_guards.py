"""
Tests for Risk Management Guards.

These tests verify that the risk management rules are properly enforced:
- 1% Risk Rule: Maximum risk per trade
- 4% Monthly Drawdown: Trading pause when limit is breached
- Stop-Loss Required: No trade without defined stop-loss

These guards are CRITICAL for protecting user capital.
"""

from django.test import TestCase
from decimal import Decimal

from api.application.risk_guards import (
    RiskManagementGuard,
    MonthlyDrawdownGuard,
    TradeIntentGuard,
    get_trading_guards,
)
from api.application.execution import ExecutionMode


class TestRiskManagementGuard(TestCase):
    """Tests for the 1% Risk Rule enforcement."""
    
    def test_blocks_trade_without_stop_loss(self):
        """A trade without stop-loss MUST be blocked."""
        guard = RiskManagementGuard()
        
        context = {
            "entry_price": Decimal("95000"),
            "quantity": Decimal("0.01"),
            "capital": Decimal("1000"),
            "side": "BUY",
            # No stop_price!
        }
        
        result = guard.check(context)
        
        self.assertFalse(result.passed)
        self.assertIn("STOP-LOSS REQUIRED", result.message)
    
    def test_blocks_trade_with_excessive_risk(self):
        """A trade risking more than 1% MUST be blocked."""
        guard = RiskManagementGuard(max_risk_percent=Decimal("1.0"))
        
        context = {
            "entry_price": Decimal("95000"),
            "stop_price": Decimal("90000"),  # 5.26% stop distance
            "quantity": Decimal("0.01"),  # $50 risk (5% of $1000)
            "capital": Decimal("1000"),
            "side": "BUY",
        }
        
        result = guard.check(context)
        
        self.assertFalse(result.passed)
        self.assertIn("RISK TOO HIGH", result.message)
        self.assertIn("recommendation", result.details)
    
    def test_passes_trade_with_acceptable_risk(self):
        """A trade risking less than 1% should pass."""
        guard = RiskManagementGuard(max_risk_percent=Decimal("1.0"))
        
        # Risk = |95000 - 94050| * 0.001 = $9.50 = 0.95% of $1000
        context = {
            "entry_price": Decimal("95000"),
            "stop_price": Decimal("94050"),  # ~1% stop distance
            "quantity": Decimal("0.001"),  # Small position
            "capital": Decimal("1000"),
            "side": "BUY",
        }
        
        result = guard.check(context)
        
        self.assertTrue(result.passed)
        self.assertIn("Risk validated", result.message)
    
    def test_validates_stop_direction_for_long(self):
        """For LONG, stop must be below entry."""
        guard = RiskManagementGuard()
        
        context = {
            "entry_price": Decimal("95000"),
            "stop_price": Decimal("96000"),  # WRONG: above entry for LONG
            "side": "BUY",
        }
        
        result = guard.check(context)
        
        self.assertFalse(result.passed)
        self.assertIn("Invalid stop-loss", result.message)
    
    def test_validates_stop_direction_for_short(self):
        """For SHORT, stop must be above entry."""
        guard = RiskManagementGuard()
        
        context = {
            "entry_price": Decimal("95000"),
            "stop_price": Decimal("94000"),  # WRONG: below entry for SHORT
            "side": "SELL",
        }
        
        result = guard.check(context)
        
        self.assertFalse(result.passed)
        self.assertIn("Invalid stop-loss", result.message)
    
    def test_calculates_safe_quantity(self):
        """Should recommend safe quantity that respects 1% rule."""
        guard = RiskManagementGuard(max_risk_percent=Decimal("1.0"))
        
        # Capital: $1000, Max risk: $10
        # Stop distance: $950 (1%)
        # Safe quantity: $10 / $950 = 0.0105263
        safe_qty = guard._calculate_safe_quantity(
            capital=Decimal("1000"),
            entry_price=Decimal("95000"),
            stop_price=Decimal("94050"),  # ~$950 stop distance
        )
        
        # Should be approximately 0.0105
        self.assertGreater(safe_qty, Decimal("0.01"))
        self.assertLess(safe_qty, Decimal("0.011"))


class TestMonthlyDrawdownGuard(TestCase):
    """Tests for the 4% Monthly Drawdown enforcement."""
    
    def test_blocks_trading_when_drawdown_exceeded(self):
        """Trading MUST be paused when monthly loss exceeds 4%."""
        guard = MonthlyDrawdownGuard(max_drawdown_percent=Decimal("4.0"))
        
        context = {
            "capital": Decimal("10000"),
            "monthly_pnl": Decimal("-500"),  # -5% loss (exceeds 4%)
        }
        
        result = guard.check(context)
        
        self.assertFalse(result.passed)
        self.assertIn("TRADING PAUSED", result.message)
        self.assertIn("5.00%", result.message)
    
    def test_allows_trading_when_under_limit(self):
        """Trading should be allowed when under drawdown limit."""
        guard = MonthlyDrawdownGuard(max_drawdown_percent=Decimal("4.0"))
        
        context = {
            "capital": Decimal("10000"),
            "monthly_pnl": Decimal("-200"),  # -2% loss (under 4%)
        }
        
        result = guard.check(context)
        
        self.assertTrue(result.passed)
        self.assertIn("remaining", result.details)
    
    def test_allows_trading_when_profitable(self):
        """Trading should definitely be allowed when profitable."""
        guard = MonthlyDrawdownGuard()
        
        context = {
            "capital": Decimal("10000"),
            "monthly_pnl": Decimal("500"),  # +5% profit
        }
        
        result = guard.check(context)
        
        self.assertTrue(result.passed)
        self.assertIn("profitable", result.message)
    
    def test_force_override_bypasses_check(self):
        """Emergency override should bypass drawdown check (use with caution!)."""
        guard = MonthlyDrawdownGuard()
        
        context = {
            "capital": Decimal("10000"),
            "monthly_pnl": Decimal("-1000"),  # -10% loss (way over limit)
            "force_override": True,
        }
        
        result = guard.check(context)
        
        self.assertTrue(result.passed)
        self.assertIn("OVERRIDDEN", result.message)
        self.assertIn("warning", result.details)
    
    def test_handles_missing_capital_gracefully(self):
        """Should pass with warning if capital is not provided."""
        guard = MonthlyDrawdownGuard()
        
        context = {
            "monthly_pnl": Decimal("-500"),
            # No capital
        }
        
        result = guard.check(context)
        
        # Passes because we can't calculate without capital
        self.assertTrue(result.passed)
        self.assertIn("Cannot validate", result.message)


class TestTradeIntentGuard(TestCase):
    """Tests for trade intent validation."""
    
    def test_requires_strategy_for_live_trades(self):
        """LIVE trades MUST specify a strategy."""
        guard = TradeIntentGuard()
        
        context = {
            "mode": ExecutionMode.LIVE,
            "confirmed": True,
            # No strategy_name
        }
        
        result = guard.check(context)
        
        self.assertFalse(result.passed)
        self.assertIn("STRATEGY REQUIRED", result.message)
    
    def test_requires_confirmation_for_live_trades(self):
        """LIVE trades MUST be explicitly confirmed."""
        guard = TradeIntentGuard()
        
        context = {
            "mode": ExecutionMode.LIVE,
            "strategy_name": "scalping",
            # Not confirmed
        }
        
        result = guard.check(context)
        
        self.assertFalse(result.passed)
        self.assertIn("CONFIRMATION REQUIRED", result.message)
    
    def test_passes_when_all_requirements_met(self):
        """Should pass when strategy is specified and confirmed."""
        guard = TradeIntentGuard()
        
        context = {
            "mode": ExecutionMode.LIVE,
            "strategy_name": "scalping",
            "confirmed": True,
        }
        
        result = guard.check(context)
        
        self.assertTrue(result.passed)
        self.assertIn("scalping", result.message)
    
    def test_dry_run_is_lenient(self):
        """DRY-RUN should be more lenient about requirements."""
        guard = TradeIntentGuard()
        
        context = {
            "mode": ExecutionMode.DRY_RUN,
            # No strategy, no confirmation
        }
        
        result = guard.check(context)
        
        self.assertTrue(result.passed)
        self.assertIn("DRY-RUN", result.message)
    
    def test_blocks_on_failed_emotional_check(self):
        """Should block if emotional guard failed."""
        guard = TradeIntentGuard()
        
        context = {
            "mode": ExecutionMode.LIVE,
            "strategy_name": "scalping",
            "confirmed": True,
            "emotional_check_passed": False,
            "emotional_risk_level": "high",
        }
        
        result = guard.check(context)
        
        self.assertFalse(result.passed)
        self.assertIn("EMOTIONAL CHECK FAILED", result.message)


class TestGuardFactory(TestCase):
    """Tests for the guard factory function."""
    
    def test_creates_default_guards(self):
        """Should create risk and drawdown guards by default."""
        guards = get_trading_guards()
        
        self.assertEqual(len(guards), 2)
        self.assertIsInstance(guards[0], RiskManagementGuard)
        self.assertIsInstance(guards[1], MonthlyDrawdownGuard)
    
    def test_can_include_intent_guard(self):
        """Should add intent guard when requested."""
        guards = get_trading_guards(include_intent=True)
        
        self.assertEqual(len(guards), 3)
        self.assertIsInstance(guards[2], TradeIntentGuard)
    
    def test_can_exclude_guards(self):
        """Should allow excluding specific guards."""
        guards = get_trading_guards(include_risk=False)
        
        self.assertEqual(len(guards), 1)
        self.assertIsInstance(guards[0], MonthlyDrawdownGuard)
    
    def test_custom_risk_percent(self):
        """Should accept custom risk percentage."""
        guards = get_trading_guards(max_risk_percent=Decimal("0.5"))
        
        risk_guard = guards[0]
        self.assertEqual(risk_guard.max_risk_percent, Decimal("0.5"))
    
    def test_custom_drawdown_percent(self):
        """Should accept custom drawdown percentage."""
        guards = get_trading_guards(max_drawdown_percent=Decimal("3.0"))
        
        drawdown_guard = guards[1]
        self.assertEqual(drawdown_guard.max_drawdown_percent, Decimal("3.0"))


class TestRealWorldScenarios(TestCase):
    """Integration tests with realistic trading scenarios."""
    
    def test_safe_trade_passes_all_guards(self):
        """A properly sized trade with stop-loss should pass all guards."""
        guards = get_trading_guards(include_intent=True)
        
        # Scenario: $10,000 capital, buying BTC at $95,000 with 1% stop
        context = {
            "mode": ExecutionMode.LIVE,
            "capital": Decimal("10000"),
            "entry_price": Decimal("95000"),
            "stop_price": Decimal("94050"),  # ~1% stop
            "quantity": Decimal("0.00105"),  # ~$100 position, $1 risk
            "side": "BUY",
            "monthly_pnl": Decimal("100"),  # Profitable month
            "strategy_name": "breakout",
            "confirmed": True,
        }
        
        for guard in guards:
            result = guard.check(context)
            self.assertTrue(result.passed, f"{guard.__class__.__name__} failed: {result.message}")
    
    def test_overleveraged_trade_blocked(self):
        """An overleveraged trade should be blocked by risk guard."""
        guards = get_trading_guards()
        
        # Scenario: Trying to risk 5% of capital
        context = {
            "capital": Decimal("10000"),
            "entry_price": Decimal("95000"),
            "stop_price": Decimal("90000"),  # 5.26% stop (wide)
            "quantity": Decimal("0.01"),  # $950 position, $50 risk (5%)
            "side": "BUY",
            "monthly_pnl": Decimal("0"),
        }
        
        risk_guard = guards[0]
        result = risk_guard.check(context)
        
        self.assertFalse(result.passed)
        self.assertIn("RISK TOO HIGH", result.message)
    
    def test_drawdown_pauses_after_bad_month(self):
        """Trading should pause after hitting monthly drawdown limit."""
        guards = get_trading_guards()
        
        # Scenario: Lost 5% this month
        context = {
            "capital": Decimal("10000"),
            "entry_price": Decimal("95000"),
            "stop_price": Decimal("94050"),
            "quantity": Decimal("0.001"),
            "side": "BUY",
            "monthly_pnl": Decimal("-500"),  # -5% monthly loss
        }
        
        drawdown_guard = guards[1]
        result = drawdown_guard.check(context)
        
        self.assertFalse(result.passed)
        self.assertIn("TRADING PAUSED", result.message)

