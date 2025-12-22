"""
Risk management domain logic.

This module contains the core risk management calculations:
- Position sizing based on the 1% rule
- Stop loss and take profit calculation
- Risk/reward analysis

These are pure functions with no framework dependencies.
"""

from __future__ import annotations
from dataclasses import dataclass
from decimal import Decimal, ROUND_DOWN
from typing import Optional


@dataclass(frozen=True)
class PositionSizeResult:
    """
    Result of position sizing calculation.
    
    Contains all information needed to understand the risk profile
    of a potential trade.
    """
    
    # Calculated position
    quantity: Decimal
    position_value: Decimal
    
    # Risk metrics
    risk_amount: Decimal  # Maximum loss in quote currency
    risk_percent: Decimal  # Risk as % of capital
    
    # Position metrics
    position_percent: Decimal  # Position as % of capital
    is_capped: bool  # True if limited by max_position_percent
    
    # Price levels
    entry_price: Decimal
    stop_loss_price: Decimal
    take_profit_price: Optional[Decimal]
    
    # Risk/Reward
    stop_distance_percent: Decimal
    target_distance_percent: Optional[Decimal]
    risk_reward_ratio: Optional[Decimal]
    
    def to_dict(self) -> dict:
        """Convert to dictionary for API responses."""
        return {
            "quantity": str(self.quantity),
            "position_value": str(self.position_value),
            "risk_amount": str(self.risk_amount),
            "risk_percent": str(self.risk_percent),
            "position_percent": str(self.position_percent),
            "is_capped": self.is_capped,
            "entry_price": str(self.entry_price),
            "stop_loss_price": str(self.stop_loss_price),
            "take_profit_price": str(self.take_profit_price) if self.take_profit_price else None,
            "stop_distance_percent": str(self.stop_distance_percent),
            "target_distance_percent": str(self.target_distance_percent) if self.target_distance_percent else None,
            "risk_reward_ratio": str(self.risk_reward_ratio) if self.risk_reward_ratio else None,
        }


class PositionSizingCalculator:
    """
    Calculate optimal position size based on risk management rules.
    
    The 1% Rule:
    - Never risk more than 1% of capital on a single trade
    - Position size = Risk Amount / Stop Loss Distance
    
    Example:
        Capital: $10,000
        Max Risk: 1% = $100
        Entry: $50,000
        Stop Loss: $49,000 (2% below entry)
        Distance: $1,000
        
        Position Size = $100 / $1,000 = 0.1 BTC
        Position Value = 0.1 * $50,000 = $5,000 (50% of capital)
        
        If stopped out: 0.1 * $1,000 = $100 = 1% of capital ✓
    """
    
    def __init__(
        self,
        max_risk_percent: Decimal = Decimal("1.0"),
        max_position_percent: Decimal = Decimal("50.0"),
        default_stop_loss_percent: Decimal = Decimal("2.0"),
        default_take_profit_percent: Decimal = Decimal("4.0"),
        min_quantity: Decimal = Decimal("0.00001"),
    ):
        """
        Initialize calculator with risk parameters.
        
        Args:
            max_risk_percent: Maximum risk per trade as % of capital (default: 1%)
            max_position_percent: Maximum position size as % of capital (default: 50%)
            default_stop_loss_percent: Default stop loss distance (default: 2%)
            default_take_profit_percent: Default take profit distance (default: 4%)
            min_quantity: Minimum tradeable quantity (default: 0.00001)
        """
        self.max_risk_percent = max_risk_percent
        self.max_position_percent = max_position_percent
        self.default_stop_loss_percent = default_stop_loss_percent
        self.default_take_profit_percent = default_take_profit_percent
        self.min_quantity = min_quantity
    
    def calculate(
        self,
        capital: Decimal,
        entry_price: Decimal,
        stop_loss_price: Optional[Decimal] = None,
        stop_loss_percent: Optional[Decimal] = None,
        take_profit_price: Optional[Decimal] = None,
        take_profit_percent: Optional[Decimal] = None,
        side: str = "BUY",
        quantity_precision: int = 5,
    ) -> PositionSizeResult:
        """
        Calculate optimal position size based on risk parameters.
        
        Args:
            capital: Total available capital in quote currency
            entry_price: Entry price for the trade
            stop_loss_price: Absolute stop loss price (optional)
            stop_loss_percent: Stop loss as % below/above entry (optional)
            take_profit_price: Absolute take profit price (optional)
            take_profit_percent: Take profit as % above/below entry (optional)
            side: "BUY" (long) or "SELL" (short)
            quantity_precision: Decimal places for quantity (default: 5)
            
        Returns:
            PositionSizeResult with calculated position and risk metrics
            
        Raises:
            ValueError: If parameters are invalid
        """
        # Validate inputs
        if capital <= 0:
            raise ValueError("Capital must be positive")
        if entry_price <= 0:
            raise ValueError("Entry price must be positive")
        if side not in ("BUY", "SELL"):
            raise ValueError("Side must be 'BUY' or 'SELL'")
        
        # Calculate stop loss price if not provided
        if stop_loss_price is None:
            sl_percent = stop_loss_percent or self.default_stop_loss_percent
            if side == "BUY":
                stop_loss_price = entry_price * (1 - sl_percent / 100)
            else:
                stop_loss_price = entry_price * (1 + sl_percent / 100)
        
        # Calculate take profit price if not provided
        if take_profit_price is None and take_profit_percent is not None:
            if side == "BUY":
                take_profit_price = entry_price * (1 + take_profit_percent / 100)
            else:
                take_profit_price = entry_price * (1 - take_profit_percent / 100)
        elif take_profit_price is None:
            tp_percent = self.default_take_profit_percent
            if side == "BUY":
                take_profit_price = entry_price * (1 + tp_percent / 100)
            else:
                take_profit_price = entry_price * (1 - tp_percent / 100)
        
        # Validate stop loss direction
        if side == "BUY" and stop_loss_price >= entry_price:
            raise ValueError("Stop loss must be below entry price for BUY orders")
        if side == "SELL" and stop_loss_price <= entry_price:
            raise ValueError("Stop loss must be above entry price for SELL orders")
        
        # Calculate risk metrics
        stop_distance = abs(entry_price - stop_loss_price)
        stop_distance_percent = (stop_distance / entry_price) * 100
        
        target_distance = abs(take_profit_price - entry_price) if take_profit_price else None
        target_distance_percent = (target_distance / entry_price) * 100 if target_distance else None
        
        # Calculate risk amount (1% of capital by default)
        risk_amount = capital * (self.max_risk_percent / 100)
        
        # Calculate position size based on risk
        # Position Size = Risk Amount / Stop Distance
        quantity = risk_amount / stop_distance
        
        # Round down to precision
        precision_factor = Decimal(10) ** quantity_precision
        quantity = (quantity * precision_factor).to_integral_value(rounding=ROUND_DOWN) / precision_factor
        
        # Ensure minimum quantity
        if quantity < self.min_quantity:
            quantity = self.min_quantity
        
        # Calculate position value
        position_value = quantity * entry_price
        
        # Check if position exceeds max position size
        max_position_value = capital * (self.max_position_percent / 100)
        is_capped = False
        
        if position_value > max_position_value:
            # Cap at max position size
            is_capped = True
            quantity = (max_position_value / entry_price)
            quantity = (quantity * precision_factor).to_integral_value(rounding=ROUND_DOWN) / precision_factor
            position_value = quantity * entry_price
            
            # Recalculate actual risk with capped position
            risk_amount = quantity * stop_distance
        
        # Calculate position as % of capital
        position_percent = (position_value / capital) * 100
        
        # Recalculate actual risk percent
        actual_risk_percent = (risk_amount / capital) * 100
        
        # Calculate risk/reward ratio
        risk_reward_ratio = None
        if target_distance and stop_distance > 0:
            risk_reward_ratio = target_distance / stop_distance
        
        return PositionSizeResult(
            quantity=quantity,
            position_value=position_value.quantize(Decimal("0.01")),
            risk_amount=risk_amount.quantize(Decimal("0.01")),
            risk_percent=actual_risk_percent.quantize(Decimal("0.01")),
            position_percent=position_percent.quantize(Decimal("0.01")),
            is_capped=is_capped,
            entry_price=entry_price,
            stop_loss_price=stop_loss_price.quantize(Decimal("0.01")),
            take_profit_price=take_profit_price.quantize(Decimal("0.01")) if take_profit_price else None,
            stop_distance_percent=stop_distance_percent.quantize(Decimal("0.01")),
            target_distance_percent=target_distance_percent.quantize(Decimal("0.01")) if target_distance_percent else None,
            risk_reward_ratio=risk_reward_ratio.quantize(Decimal("0.01")) if risk_reward_ratio else None,
        )
    
    def calculate_from_risk_config(
        self,
        capital: Decimal,
        entry_price: Decimal,
        risk_config: dict,
        side: str = "BUY",
        quantity_precision: int = 5,
    ) -> PositionSizeResult:
        """
        Calculate position size using a risk configuration dictionary.
        
        Args:
            capital: Total available capital
            entry_price: Entry price for the trade
            risk_config: Dictionary with risk parameters
            side: "BUY" or "SELL"
            quantity_precision: Decimal places for quantity
            
        Expected risk_config keys:
            - max_risk_per_trade_percent: Max risk per trade (default: 1)
            - stop_loss_percent: Stop loss distance (default: 2)
            - take_profit_percent: Take profit distance (default: 4)
            - max_position_size_percent: Max position size (default: 50)
        """
        # Extract risk parameters with defaults
        max_risk = Decimal(str(risk_config.get("max_risk_per_trade_percent", 1)))
        stop_loss = Decimal(str(risk_config.get("stop_loss_percent", 2)))
        take_profit = Decimal(str(risk_config.get("take_profit_percent", 4)))
        max_position = Decimal(str(risk_config.get("max_position_size_percent", 50)))
        
        # Update calculator parameters
        self.max_risk_percent = max_risk
        self.max_position_percent = max_position
        
        return self.calculate(
            capital=capital,
            entry_price=entry_price,
            stop_loss_percent=stop_loss,
            take_profit_percent=take_profit,
            side=side,
            quantity_precision=quantity_precision,
        )


def calculate_position_size(
    capital: Decimal,
    entry_price: Decimal,
    stop_loss_percent: Decimal = Decimal("2.0"),
    take_profit_percent: Decimal = Decimal("4.0"),
    max_risk_percent: Decimal = Decimal("1.0"),
    max_position_percent: Decimal = Decimal("50.0"),
    side: str = "BUY",
    quantity_precision: int = 5,
) -> PositionSizeResult:
    """
    Convenience function for position sizing calculation.
    
    This is a simple wrapper around PositionSizingCalculator for quick calculations.
    
    Args:
        capital: Total available capital in quote currency
        entry_price: Entry price for the trade
        stop_loss_percent: Stop loss as % from entry (default: 2%)
        take_profit_percent: Take profit as % from entry (default: 4%)
        max_risk_percent: Maximum risk per trade (default: 1%)
        max_position_percent: Maximum position size (default: 50%)
        side: "BUY" or "SELL"
        quantity_precision: Decimal places for quantity
        
    Returns:
        PositionSizeResult with calculated position and risk metrics
        
    Example:
        >>> result = calculate_position_size(
        ...     capital=Decimal("1000"),
        ...     entry_price=Decimal("90000"),
        ...     stop_loss_percent=Decimal("2"),
        ...     take_profit_percent=Decimal("4"),
        ... )
        >>> print(f"Buy {result.quantity} BTC")
        >>> print(f"Risk: ${result.risk_amount} ({result.risk_percent}%)")
    """
    calculator = PositionSizingCalculator(
        max_risk_percent=max_risk_percent,
        max_position_percent=max_position_percent,
    )
    
    return calculator.calculate(
        capital=capital,
        entry_price=entry_price,
        stop_loss_percent=stop_loss_percent,
        take_profit_percent=take_profit_percent,
        side=side,
        quantity_precision=quantity_precision,
    )

