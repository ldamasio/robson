"""
Tests for Hand-Span Trailing Stop module.

Test coverage:
1. Domain entity validation
2. Calculator logic (step thresholds)
3. Monotonic property (stop never loosens)
4. Edge cases (zero span, exact thresholds, etc.)
5. Integration with use cases
"""

import pytest
from decimal import Decimal
from datetime import datetime

from api.application.trailing_stop.domain import (
    TrailingStopState,
    StopAdjustment,
    PositionSide,
    AdjustmentReason,
    FeeConfig,
)
from api.application.trailing_stop.calculator import HandSpanCalculator


class TestTrailingStopState:
    """Test TrailingStopState domain entity."""

    def test_valid_long_position(self):
        """Test creating valid LONG position state."""
        state = TrailingStopState(
            position_id="123",
            symbol="BTCUSDT",
            side=PositionSide.LONG,
            entry_price=Decimal("50000"),
            initial_stop=Decimal("49000"),  # $1000 below entry
            current_stop=Decimal("49000"),
            current_price=Decimal("51000"),
            quantity=Decimal("1.0"),
        )

        assert state.span == Decimal("1000")
        assert state.spans_in_profit == 1  # Moved $1000 in profit
        assert state.is_at_profit is True

    def test_valid_short_position(self):
        """Test creating valid SHORT position state."""
        state = TrailingStopState(
            position_id="456",
            symbol="ETHUSDT",
            side=PositionSide.SHORT,
            entry_price=Decimal("3000"),
            initial_stop=Decimal("3100"),  # $100 above entry
            current_stop=Decimal("3100"),
            current_price=Decimal("2900"),
            quantity=Decimal("10.0"),
        )

        assert state.span == Decimal("100")
        assert state.spans_in_profit == 1  # Moved $100 in profit
        assert state.is_at_profit is True

    def test_invalid_long_stop_above_entry(self):
        """Test that LONG initial stop cannot be above entry."""
        with pytest.raises(ValueError, match="LONG: initial stop must be below entry price"):
            TrailingStopState(
                position_id="789",
                symbol="BTCUSDT",
                side=PositionSide.LONG,
                entry_price=Decimal("50000"),
                initial_stop=Decimal("51000"),  # Invalid: above entry
                current_stop=Decimal("51000"),
                current_price=Decimal("52000"),
                quantity=Decimal("1.0"),
            )

    def test_invalid_short_stop_below_entry(self):
        """Test that SHORT initial stop cannot be below entry."""
        with pytest.raises(ValueError, match="SHORT: initial stop must be above entry price"):
            TrailingStopState(
                position_id="101",
                symbol="ETHUSDT",
                side=PositionSide.SHORT,
                entry_price=Decimal("3000"),
                initial_stop=Decimal("2900"),  # Invalid: below entry
                current_stop=Decimal("2900"),
                current_price=Decimal("2800"),
                quantity=Decimal("10.0"),
            )

    def test_spans_in_profit_zero_when_at_loss(self):
        """Test that spans_in_profit is 0 when position is at loss."""
        state = TrailingStopState(
            position_id="111",
            symbol="BTCUSDT",
            side=PositionSide.LONG,
            entry_price=Decimal("50000"),
            initial_stop=Decimal("49000"),
            current_stop=Decimal("49000"),
            current_price=Decimal("49500"),  # At loss (below entry)
            quantity=Decimal("1.0"),
        )

        assert state.spans_in_profit == 0
        assert state.is_at_profit is False

    def test_spans_in_profit_multiple_spans(self):
        """Test correct calculation of multiple spans crossed."""
        state = TrailingStopState(
            position_id="222",
            symbol="BTCUSDT",
            side=PositionSide.LONG,
            entry_price=Decimal("50000"),
            initial_stop=Decimal("49000"),  # Span = $1000
            current_stop=Decimal("49000"),
            current_price=Decimal("53500"),  # $3500 profit = 3 complete spans
            quantity=Decimal("1.0"),
        )

        assert state.span == Decimal("1000")
        assert state.spans_in_profit == 3


class TestHandSpanCalculator:
    """Test HandSpanCalculator logic."""

    @pytest.fixture
    def calculator(self):
        """Create calculator with default fee config."""
        return HandSpanCalculator(FeeConfig(
            trading_fee_percent=Decimal("0.1"),
            slippage_buffer_percent=Decimal("0.05"),
        ))

    def test_no_adjustment_when_no_profit_long(self, calculator):
        """Test that no adjustment occurs when LONG position has no profit."""
        state = TrailingStopState(
            position_id="300",
            symbol="BTCUSDT",
            side=PositionSide.LONG,
            entry_price=Decimal("50000"),
            initial_stop=Decimal("49000"),
            current_stop=Decimal("49000"),
            current_price=Decimal("49500"),  # Still at loss
            quantity=Decimal("1.0"),
        )

        adjustment = calculator.calculate_adjustment(state)

        assert adjustment.old_stop == Decimal("49000")
        assert adjustment.new_stop == Decimal("49000")
        assert adjustment.reason == AdjustmentReason.NO_ADJUSTMENT
        assert adjustment.is_adjusted is False

    def test_break_even_adjustment_at_one_span_long(self, calculator):
        """Test break-even adjustment when LONG crosses 1 span."""
        state = TrailingStopState(
            position_id="400",
            symbol="BTCUSDT",
            side=PositionSide.LONG,
            entry_price=Decimal("50000"),
            initial_stop=Decimal("49000"),  # Span = $1000
            current_stop=Decimal("49000"),
            current_price=Decimal("51000"),  # Crossed 1 span
            quantity=Decimal("1.0"),
        )

        adjustment = calculator.calculate_adjustment(state)

        # Break-even should be entry + fees (0.15% = $75)
        expected_break_even = Decimal("50000") * Decimal("1.0015")

        assert adjustment.reason == AdjustmentReason.BREAK_EVEN
        assert adjustment.new_stop == expected_break_even
        assert adjustment.is_adjusted is True
        assert adjustment.step_index == 1

    def test_trailing_adjustment_at_two_spans_long(self, calculator):
        """Test trailing adjustment when LONG crosses 2 spans."""
        state = TrailingStopState(
            position_id="500",
            symbol="BTCUSDT",
            side=PositionSide.LONG,
            entry_price=Decimal("50000"),
            initial_stop=Decimal("49000"),  # Span = $1000
            current_stop=Decimal("49000"),
            current_price=Decimal("52000"),  # Crossed 2 spans
            quantity=Decimal("1.0"),
        )

        adjustment = calculator.calculate_adjustment(state)

        # At 2 spans: stop should be entry + 1 span = $51,000
        expected_stop = Decimal("50000") + Decimal("1000")

        assert adjustment.reason == AdjustmentReason.TRAILING
        assert adjustment.new_stop == expected_stop
        assert adjustment.is_adjusted is True
        assert adjustment.step_index == 2

    def test_trailing_adjustment_at_three_spans_long(self, calculator):
        """Test trailing adjustment when LONG crosses 3 spans."""
        state = TrailingStopState(
            position_id="600",
            symbol="BTCUSDT",
            side=PositionSide.LONG,
            entry_price=Decimal("50000"),
            initial_stop=Decimal("49000"),  # Span = $1000
            current_stop=Decimal("49000"),
            current_price=Decimal("53000"),  # Crossed 3 spans
            quantity=Decimal("1.0"),
        )

        adjustment = calculator.calculate_adjustment(state)

        # At 3 spans: stop should be entry + 2 spans = $52,000
        expected_stop = Decimal("50000") + (Decimal("2") * Decimal("1000"))

        assert adjustment.reason == AdjustmentReason.TRAILING
        assert adjustment.new_stop == expected_stop
        assert adjustment.is_adjusted is True
        assert adjustment.step_index == 3

    def test_break_even_adjustment_short(self, calculator):
        """Test break-even adjustment for SHORT position."""
        state = TrailingStopState(
            position_id="700",
            symbol="ETHUSDT",
            side=PositionSide.SHORT,
            entry_price=Decimal("3000"),
            initial_stop=Decimal("3100"),  # Span = $100
            current_stop=Decimal("3100"),
            current_price=Decimal("2900"),  # Crossed 1 span down
            quantity=Decimal("10.0"),
        )

        adjustment = calculator.calculate_adjustment(state)

        # Break-even for SHORT: entry / (1 + fees) = 3000 / 1.0015
        expected_break_even = Decimal("3000") / Decimal("1.0015")

        assert adjustment.reason == AdjustmentReason.BREAK_EVEN
        assert adjustment.new_stop == expected_break_even
        assert adjustment.is_adjusted is True

    def test_trailing_adjustment_short(self, calculator):
        """Test trailing adjustment for SHORT position at 2 spans."""
        state = TrailingStopState(
            position_id="800",
            symbol="ETHUSDT",
            side=PositionSide.SHORT,
            entry_price=Decimal("3000"),
            initial_stop=Decimal("3100"),  # Span = $100
            current_stop=Decimal("3100"),
            current_price=Decimal("2800"),  # Crossed 2 spans down
            quantity=Decimal("10.0"),
        )

        adjustment = calculator.calculate_adjustment(state)

        # At 2 spans: stop should be entry - 1 span = $2900
        expected_stop = Decimal("3000") - Decimal("100")

        assert adjustment.reason == AdjustmentReason.TRAILING
        assert adjustment.new_stop == expected_stop
        assert adjustment.is_adjusted is True
        assert adjustment.step_index == 2

    def test_monotonic_property_long_never_decreases(self, calculator):
        """Test that LONG stop never decreases (monotonic property)."""
        # Start with stop already at break-even
        break_even = Decimal("50000") * Decimal("1.0015")

        state = TrailingStopState(
            position_id="900",
            symbol="BTCUSDT",
            side=PositionSide.LONG,
            entry_price=Decimal("50000"),
            initial_stop=Decimal("49000"),
            current_stop=break_even,  # Already at break-even
            current_price=Decimal("50500"),  # Less than 1 span profit
            quantity=Decimal("1.0"),
        )

        adjustment = calculator.calculate_adjustment(state)

        # Stop should stay at break-even (not decrease)
        assert adjustment.new_stop == break_even
        assert adjustment.new_stop >= state.current_stop

    def test_monotonic_property_short_never_increases(self, calculator):
        """Test that SHORT stop never increases (monotonic property)."""
        # Start with stop already at break-even
        break_even = Decimal("3000") / Decimal("1.0015")

        state = TrailingStopState(
            position_id="1000",
            symbol="ETHUSDT",
            side=PositionSide.SHORT,
            entry_price=Decimal("3000"),
            initial_stop=Decimal("3100"),
            current_stop=break_even,  # Already at break-even
            current_price=Decimal("2950"),  # Less than 1 span profit
            quantity=Decimal("10.0"),
        )

        adjustment = calculator.calculate_adjustment(state)

        # Stop should stay at break-even (not increase)
        assert adjustment.new_stop == break_even
        assert adjustment.new_stop <= state.current_stop


class TestMonotonicProperty:
    """
    Property-based tests for monotonic guarantee.

    These test that the stop NEVER loosens, regardless of price movements.
    """

    @pytest.fixture
    def calculator(self):
        """Create calculator with default fee config."""
        return HandSpanCalculator()

    def test_long_stop_never_decreases_with_increasing_profit(self, calculator):
        """Test LONG stop only increases as profit increases."""
        entry = Decimal("50000")
        initial_stop = Decimal("49000")
        span = entry - initial_stop

        previous_stop = initial_stop

        # Simulate price moving from entry to 5 spans of profit
        for price_increment in range(0, 6000, 100):
            current_price = entry + Decimal(price_increment)

            state = TrailingStopState(
                position_id="prop_test_long",
                symbol="BTCUSDT",
                side=PositionSide.LONG,
                entry_price=entry,
                initial_stop=initial_stop,
                current_stop=previous_stop,
                current_price=current_price,
                quantity=Decimal("1.0"),
            )

            adjustment = calculator.calculate_adjustment(state)
            new_stop = adjustment.new_stop

            # CRITICAL: stop must NEVER decrease
            assert new_stop >= previous_stop, \
                f"Stop decreased from {previous_stop} to {new_stop} at price {current_price}"

            previous_stop = new_stop

    def test_short_stop_never_increases_with_increasing_profit(self, calculator):
        """Test SHORT stop only decreases as profit increases."""
        entry = Decimal("3000")
        initial_stop = Decimal("3100")
        span = initial_stop - entry

        previous_stop = initial_stop

        # Simulate price moving from entry to 5 spans of profit (downward)
        for price_decrement in range(0, 600, 10):
            current_price = entry - Decimal(price_decrement)

            state = TrailingStopState(
                position_id="prop_test_short",
                symbol="ETHUSDT",
                side=PositionSide.SHORT,
                entry_price=entry,
                initial_stop=initial_stop,
                current_stop=previous_stop,
                current_price=current_price,
                quantity=Decimal("10.0"),
            )

            adjustment = calculator.calculate_adjustment(state)
            new_stop = adjustment.new_stop

            # CRITICAL: stop must NEVER increase
            assert new_stop <= previous_stop, \
                f"Stop increased from {previous_stop} to {new_stop} at price {current_price}"

            previous_stop = new_stop


class TestEdgeCases:
    """Test edge cases and boundary conditions."""

    @pytest.fixture
    def calculator(self):
        """Create calculator with default fee config."""
        return HandSpanCalculator()

    def test_exact_span_boundary(self, calculator):
        """Test behavior at exact span boundary."""
        state = TrailingStopState(
            position_id="edge_1",
            symbol="BTCUSDT",
            side=PositionSide.LONG,
            entry_price=Decimal("50000"),
            initial_stop=Decimal("49000"),
            current_stop=Decimal("49000"),
            current_price=Decimal("51000"),  # Exactly 1 span
            quantity=Decimal("1.0"),
        )

        adjustment = calculator.calculate_adjustment(state)

        assert adjustment.spans_crossed == 1
        assert adjustment.reason == AdjustmentReason.BREAK_EVEN

    def test_just_below_span_boundary(self, calculator):
        """Test behavior just below span boundary."""
        state = TrailingStopState(
            position_id="edge_2",
            symbol="BTCUSDT",
            side=PositionSide.LONG,
            entry_price=Decimal("50000"),
            initial_stop=Decimal("49000"),
            current_stop=Decimal("49000"),
            current_price=Decimal("50999"),  # Just below 1 span
            quantity=Decimal("1.0"),
        )

        adjustment = calculator.calculate_adjustment(state)

        assert adjustment.spans_crossed == 0
        assert adjustment.reason == AdjustmentReason.NO_ADJUSTMENT

    def test_very_small_span(self, calculator):
        """Test with very small span (high precision)."""
        state = TrailingStopState(
            position_id="edge_3",
            symbol="BTCUSDT",
            side=PositionSide.LONG,
            entry_price=Decimal("50000.00"),
            initial_stop=Decimal("49999.50"),  # Tiny span: $0.50
            current_stop=Decimal("49999.50"),
            current_price=Decimal("50001.00"),  # 2 spans crossed
            quantity=Decimal("1.0"),
        )

        adjustment = calculator.calculate_adjustment(state)

        assert state.span == Decimal("0.50")
        assert state.spans_in_profit == 2
        assert adjustment.reason == AdjustmentReason.TRAILING


class TestFeeConfig:
    """Test fee configuration logic."""

    def test_default_fee_config(self):
        """Test default fee configuration."""
        config = FeeConfig()

        assert config.trading_fee_percent == Decimal("0.1")
        assert config.slippage_buffer_percent == Decimal("0.05")
        assert config.total_cost_percent == Decimal("0.15")

    def test_custom_fee_config(self):
        """Test custom fee configuration."""
        config = FeeConfig(
            trading_fee_percent=Decimal("0.2"),
            slippage_buffer_percent=Decimal("0.1"),
        )

        assert config.total_cost_percent == Decimal("0.3")

    def test_break_even_calculation_long(self):
        """Test break-even calculation for LONG."""
        config = FeeConfig(
            trading_fee_percent=Decimal("0.1"),
            slippage_buffer_percent=Decimal("0.05"),
        )

        break_even = config.calculate_break_even(
            entry_price=Decimal("50000"),
            side=PositionSide.LONG
        )

        # Should be entry * 1.0015
        expected = Decimal("50000") * Decimal("1.0015")
        assert break_even == expected

    def test_break_even_calculation_short(self):
        """Test break-even calculation for SHORT."""
        config = FeeConfig(
            trading_fee_percent=Decimal("0.1"),
            slippage_buffer_percent=Decimal("0.05"),
        )

        break_even = config.calculate_break_even(
            entry_price=Decimal("3000"),
            side=PositionSide.SHORT
        )

        # Should be entry / 1.0015
        expected = Decimal("3000") / Decimal("1.0015")
        assert break_even == expected


class TestAdjustmentSerialization:
    """Test adjustment record serialization."""

    def test_to_dict(self):
        """Test StopAdjustment.to_dict()."""
        adjustment = StopAdjustment(
            position_id="123",
            old_stop=Decimal("49000"),
            new_stop=Decimal("50075"),
            reason=AdjustmentReason.BREAK_EVEN,
            adjustment_token="123:adjust:1234567890",
            timestamp=datetime(2024, 1, 1, 12, 0, 0),
            current_price=Decimal("51000"),
            spans_crossed=1,
            step_index=1,
            metadata={"symbol": "BTCUSDT"},
        )

        data = adjustment.to_dict()

        assert data["position_id"] == "123"
        assert data["old_stop"] == "49000"
        assert data["new_stop"] == "50075"
        assert data["reason"] == "BREAK_EVEN"
        assert data["spans_crossed"] == 1
        assert data["step_index"] == 1
