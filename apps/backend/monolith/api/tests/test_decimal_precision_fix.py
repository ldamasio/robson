"""
Unit test for decimal precision fix.

Tests that CreateTradingIntentUseCase properly quantizes decimal values
to match TradingIntent model field constraints.

Bug: ValidationError when calculated values exceed max_digits/decimal_places.
Fix: Quantize decimals before persisting to database.
"""
import pytest
from decimal import Decimal
from unittest.mock import Mock
from api.application.use_cases.trading_intent import CreateTradingIntentUseCase, CreateTradingIntentCommand


class TestDecimalPrecisionFix:
    """Test decimal precision quantization."""

    def test_quantize_decimal_rounds_correctly(self):
        """Test that _quantize_decimal rounds to correct precision."""
        use_case = CreateTradingIntentUseCase(
            symbol_repo=Mock(),
            strategy_repo=Mock(),
            intent_repo=Mock(),
        )

        # Test 8 decimal places
        value_8dp = Decimal("0.123456789012345")  # Excessive precision
        quantized_8dp = use_case._quantize_decimal(value_8dp, decimal_places=8)
        assert quantized_8dp == Decimal("0.12345679")  # Rounded to 8 places

        # Test 2 decimal places
        value_2dp = Decimal("1.5789")
        quantized_2dp = use_case._quantize_decimal(value_2dp, decimal_places=2)
        assert quantized_2dp == Decimal("1.58")  # Rounded to 2 places

    def test_position_calculation_returns_quantized_values(self):
        """Test that position calculation returns properly quantized decimals."""
        use_case = CreateTradingIntentUseCase(
            symbol_repo=Mock(),
            strategy_repo=Mock(),
            intent_repo=Mock(),
        )

        # Use values that would produce excessive precision
        capital = Decimal("100")
        entry_price = Decimal("95000")
        stop_price = Decimal("93500")
        side = "BUY"

        calculations = use_case._calculate_position_and_risk(
            capital=capital,
            entry_price=entry_price,
            stop_price=stop_price,
            side=side,
        )

        # Verify all values are quantized
        quantity = calculations["quantity"]
        risk_amount = calculations["risk_amount"]
        risk_percent = calculations["risk_percent"]

        # Check that values don't exceed decimal_places
        assert quantity == quantity.quantize(Decimal("0.00000001"))  # 8 decimal places
        assert risk_amount == risk_amount.quantize(Decimal("0.00000001"))  # 8 decimal places
        assert risk_percent == risk_percent.quantize(Decimal("0.01"))  # 2 decimal places

        # Expected values (1% risk rule)
        # risk_amount = 100 * 0.01 = 1.00
        # stop_distance = |95000 - 93500| = 1500
        # quantity = 1.00 / 1500 = 0.00066667
        # risk_percent = (1500 / 95000) * 100 = 1.58
        assert risk_amount == Decimal("1.00000000")
        assert quantity == Decimal("0.00066667")
        assert risk_percent == Decimal("1.58")  # Quantized to 2 places

    def test_values_fit_model_constraints(self):
        """Test that quantized values satisfy TradingIntent model constraints."""
        use_case = CreateTradingIntentUseCase(
            symbol_repo=Mock(),
            strategy_repo=Mock(),
            intent_repo=Mock(),
        )

        # Calculate with realistic trading values
        calculations = use_case._calculate_position_and_risk(
            capital=Decimal("10000"),
            entry_price=Decimal("50000.123456"),
            stop_price=Decimal("49000.654321"),
            side="BUY",
        )

        # Model constraints:
        # - quantity: max_digits=20, decimal_places=8
        # - risk_amount: max_digits=20, decimal_places=8
        # - risk_percent: max_digits=10, decimal_places=2

        quantity = calculations["quantity"]
        risk_amount = calculations["risk_amount"]
        risk_percent = calculations["risk_percent"]

        # Check decimal places
        assert len(str(quantity).split(".")[-1]) <= 8
        assert len(str(risk_amount).split(".")[-1]) <= 8
        assert len(str(risk_percent).split(".")[-1]) <= 2

        # Check total digits (max_digits includes decimal places)
        # For quantity and risk_amount: max 20 total digits, 8 after decimal → max 12 before decimal
        # For risk_percent: max 10 total digits, 2 after decimal → max 8 before decimal
        quantity_str = str(quantity).replace(".", "")
        risk_amount_str = str(risk_amount).replace(".", "")
        risk_percent_str = str(risk_percent).replace(".", "")

        assert len(quantity_str) <= 20
        assert len(risk_amount_str) <= 20
        assert len(risk_percent_str) <= 10
