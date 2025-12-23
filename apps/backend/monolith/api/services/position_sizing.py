# api/services/position_sizing.py
"""
Position Sizing Calculator Service

This service implements Robson's PRIMARY INTELLIGENCE:
Calculating optimal position sizes based on the 1% risk rule.

This is NOT about generating trading signals.
This IS about precise risk management for USER-INITIATED trades.

Key Principle: User decides WHAT to trade, Robson calculates HOW MUCH.
"""

from decimal import Decimal
from typing import TypedDict


class PositionSizingResult(TypedDict):
    """Result of position sizing calculation."""
    quantity: Decimal
    position_value: Decimal
    risk_amount: Decimal
    risk_percent: Decimal
    stop_distance: Decimal
    stop_distance_percent: Decimal
    is_capped: bool


class PositionSizingCalculator:
    """
    Calculate optimal position size based on 1% risk rule.

    This is the core intelligence of Robson Bot.

    Formula:
        Risk Amount = Capital × (Risk % / 100)
        Quantity = Risk Amount / |Entry Price - Stop Price|

    Example:
        Capital: $1,000
        Entry: $90,000
        Stop: $88,200 (2% below entry)
        Risk: 1%

        Risk Amount = $1,000 × 0.01 = $10
        Stop Distance = $90,000 - $88,200 = $1,800
        Quantity = $10 / $1,800 = 0.00555556 BTC
        Position Value = 0.00555556 × $90,000 = $500

        If stopped: Loss = 0.00555556 × $1,800 = $10 = 1% ✓
    """

    @staticmethod
    def calculate(
        capital: Decimal,
        entry_price: Decimal,
        stop_price: Decimal,
        side: str,
        max_risk_percent: Decimal = Decimal("1.0"),
        max_position_percent: Decimal = Decimal("50.0"),
    ) -> PositionSizingResult:
        """
        Calculate position size using 1% risk rule.

        Args:
            capital: Total capital available (in quote currency, e.g., USDC)
            entry_price: Intended entry price
            stop_price: Stop-loss price (user's risk level)
            side: "BUY" or "SELL"
            max_risk_percent: Maximum risk per trade as % of capital (default 1%)
            max_position_percent: Maximum position as % of capital (default 50%)

        Returns:
            PositionSizingResult with:
            - quantity: Calculated position size in base currency
            - position_value: Total value of position in quote currency
            - risk_amount: Maximum loss if stopped (in quote currency)
            - risk_percent: Actual risk as % of capital
            - stop_distance: Distance from entry to stop (absolute)
            - stop_distance_percent: Stop distance as % of entry price
            - is_capped: True if position was limited by max_position_percent

        Raises:
            ValueError: If inputs are invalid or violate constraints
        """
        # ====================================================================
        # Input Validation
        # ====================================================================

        if capital <= 0:
            raise ValueError("Capital must be positive")

        if entry_price <= 0:
            raise ValueError("Entry price must be positive")

        if stop_price <= 0:
            raise ValueError("Stop price must be positive")

        if side not in ["BUY", "SELL"]:
            raise ValueError("Side must be BUY or SELL")

        if max_risk_percent <= 0 or max_risk_percent > 100:
            raise ValueError("Risk percent must be between 0 and 100")

        if max_position_percent <= 0 or max_position_percent > 100:
            raise ValueError("Position percent must be between 0 and 100")

        # Validate stop is on correct side of entry
        if side == "BUY" and stop_price >= entry_price:
            raise ValueError(
                f"For BUY orders, stop must be BELOW entry "
                f"(stop: ${stop_price} >= entry: ${entry_price})"
            )

        if side == "SELL" and stop_price <= entry_price:
            raise ValueError(
                f"For SELL orders, stop must be ABOVE entry "
                f"(stop: ${stop_price} <= entry: ${entry_price})"
            )

        # ====================================================================
        # Step 1: Calculate Risk Amount (1% of capital by default)
        # ====================================================================

        risk_amount = capital * (max_risk_percent / Decimal("100"))
        risk_amount = risk_amount.quantize(Decimal("0.01"))  # 2 decimals for USDC

        # ====================================================================
        # Step 2: Calculate Stop Distance (always positive)
        # ====================================================================

        stop_distance = abs(entry_price - stop_price)
        stop_distance_percent = (stop_distance / entry_price) * Decimal("100")

        # ====================================================================
        # Step 3: Calculate Position Size
        # Formula: Quantity = Risk Amount / Stop Distance
        # ====================================================================

        quantity = risk_amount / stop_distance
        quantity = quantity.quantize(Decimal("0.00000001"))  # 8 decimals (Binance)

        # ====================================================================
        # Step 4: Calculate Position Value
        # ====================================================================

        position_value = quantity * entry_price
        position_value = position_value.quantize(Decimal("0.01"))

        # ====================================================================
        # Step 5: Check Position Size Cap (50% of capital by default)
        # ====================================================================

        max_position_value = capital * (max_position_percent / Decimal("100"))
        is_capped = False

        if position_value > max_position_value:
            # Position exceeds maximum allowed percentage
            # Cap the position size and recalculate risk
            is_capped = True

            # Recalculate quantity based on cap
            quantity = max_position_value / entry_price
            quantity = quantity.quantize(Decimal("0.00000001"))

            # Recalculate position value with capped quantity
            position_value = quantity * entry_price
            position_value = position_value.quantize(Decimal("0.01"))

            # Recalculate actual risk with capped quantity
            risk_amount = quantity * stop_distance
            risk_amount = risk_amount.quantize(Decimal("0.01"))

        # ====================================================================
        # Step 6: Calculate Actual Risk Percentage
        # ====================================================================

        risk_percent = (risk_amount / capital) * Decimal("100")
        risk_percent = risk_percent.quantize(Decimal("0.01"))

        # ====================================================================
        # Return Results
        # ====================================================================

        return PositionSizingResult(
            quantity=quantity,
            position_value=position_value,
            risk_amount=risk_amount,
            risk_percent=risk_percent,
            stop_distance=stop_distance,
            stop_distance_percent=stop_distance_percent.quantize(Decimal("0.01")),
            is_capped=is_capped,
        )

    @staticmethod
    def validate_inputs(
        capital: Decimal,
        entry_price: Decimal,
        stop_price: Decimal,
        side: str,
    ) -> tuple[bool, str]:
        """
        Validate inputs without calculating.

        Returns:
            Tuple of (is_valid, error_message)
        """
        try:
            # Run calculation with minimal params to trigger validations
            PositionSizingCalculator.calculate(
                capital=capital,
                entry_price=entry_price,
                stop_price=stop_price,
                side=side,
            )
            return True, ""
        except ValueError as e:
            return False, str(e)
