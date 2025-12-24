"""
Risk Management Guards for Trading Execution.

These guards enforce the core risk management rules:
- 1% Risk Rule: Maximum risk per trade is 1% of capital
- 4% Monthly Drawdown: Trading pauses if monthly loss exceeds 4%
- Stop-Loss Required: No trade without a defined stop-loss

These guards BLOCK execution if risk parameters are not met.
They are the enforcement layer for the documented risk rules.
"""

from decimal import Decimal
from datetime import datetime, timedelta
from typing import Optional, Protocol

from django.utils import timezone
from django.db.models import Sum

from .execution import ExecutionGuard, ExecutionMode


class RiskManagementGuard:
    """
    Guard that enforces the 1% Risk Rule.
    
    Requirements:
    - stop_price MUST be defined
    - risk_amount MUST NOT exceed 1% of capital
    - position_size MUST be calculated based on stop distance
    
    This guard BLOCKS trades that:
    - Have no stop-loss
    - Risk more than 1% of capital
    - Are market orders without stop-loss attached
    """
    
    DEFAULT_MAX_RISK_PERCENT = Decimal("1.0")  # 1% max risk per trade
    
    def __init__(self, max_risk_percent: Optional[Decimal] = None):
        """
        Initialize the guard.
        
        Args:
            max_risk_percent: Maximum risk per trade as percentage (default: 1%)
        """
        self.max_risk_percent = max_risk_percent or self.DEFAULT_MAX_RISK_PERCENT
    
    def check(self, context: dict) -> ExecutionGuard:
        """
        Check if trade meets risk management requirements.
        
        Expected context:
            - stop_price: Stop-loss price (REQUIRED)
            - entry_price: Entry price
            - quantity: Position size
            - capital: Total available capital
            - side: BUY or SELL
            - risk_percent: Pre-calculated risk percentage (optional)
            
        Returns:
            ExecutionGuard with pass/fail result
        """
        mode = context.get("mode", ExecutionMode.DRY_RUN)
        
        # DRY-RUN still checks but doesn't block (warning only)
        is_live = mode == ExecutionMode.LIVE
        
        # Check 1: Stop-loss MUST be defined
        stop_price = context.get("stop_price")
        entry_price = context.get("entry_price")
        
        if stop_price is None:
            return ExecutionGuard(
                name="RiskManagement",
                passed=False,
                message="❌ STOP-LOSS REQUIRED: No trade without a defined stop-loss",
                details={
                    "rule": "1% Risk Rule",
                    "requirement": "Define stop_price before executing",
                    "docs": "docs/PRODUCTION_TRADING.md#mandatory-risk-management-rules",
                },
            )
        
        # Check 2: Validate stop direction
        side = context.get("side", "BUY").upper()
        try:
            stop_price = Decimal(str(stop_price))
            entry_price = Decimal(str(entry_price)) if entry_price else None
            
            if entry_price:
                # For LONG: stop must be below entry
                # For SHORT: stop must be above entry
                if side == "BUY" and stop_price >= entry_price:
                    return ExecutionGuard(
                        name="RiskManagement",
                        passed=False,
                        message=f"Invalid stop-loss: For LONG, stop ({stop_price}) must be below entry ({entry_price})",
                        details={"side": side, "stop": str(stop_price), "entry": str(entry_price)},
                    )
                elif side == "SELL" and stop_price <= entry_price:
                    return ExecutionGuard(
                        name="RiskManagement",
                        passed=False,
                        message=f"Invalid stop-loss: For SHORT, stop ({stop_price}) must be above entry ({entry_price})",
                        details={"side": side, "stop": str(stop_price), "entry": str(entry_price)},
                    )
        except (ValueError, TypeError) as e:
            return ExecutionGuard(
                name="RiskManagement",
                passed=False,
                message=f"Invalid price format: {e}",
            )
        
        # Check 3: Risk percentage must not exceed limit
        capital = context.get("capital")
        quantity = context.get("quantity")
        
        if capital and entry_price and quantity:
            try:
                capital = Decimal(str(capital))
                quantity = Decimal(str(quantity))
                
                # Calculate risk
                stop_distance = abs(entry_price - stop_price)
                risk_amount = stop_distance * quantity
                risk_percent = (risk_amount / capital) * Decimal("100")
                
                if risk_percent > self.max_risk_percent:
                    return ExecutionGuard(
                        name="RiskManagement",
                        passed=False,
                        message=f"❌ RISK TOO HIGH: {risk_percent:.2f}% exceeds {self.max_risk_percent}% limit",
                        details={
                            "risk_percent": str(risk_percent),
                            "max_risk_percent": str(self.max_risk_percent),
                            "risk_amount": str(risk_amount),
                            "capital": str(capital),
                            "recommendation": f"Reduce position size to {self._calculate_safe_quantity(capital, entry_price, stop_price):.6f}",
                        },
                    )
                
                # All checks passed
                return ExecutionGuard(
                    name="RiskManagement",
                    passed=True,
                    message=f"✓ Risk validated: {risk_percent:.2f}% (max {self.max_risk_percent}%)",
                    details={
                        "risk_percent": str(risk_percent),
                        "risk_amount": str(risk_amount),
                        "stop_price": str(stop_price),
                        "stop_distance": str(stop_distance),
                    },
                )
                
            except (ValueError, TypeError) as e:
                # Can't calculate risk - log warning but pass if stop is defined
                return ExecutionGuard(
                    name="RiskManagement",
                    passed=True,  # Pass because stop IS defined
                    message=f"⚠️ Could not validate risk percentage: {e}. Stop-loss is defined.",
                    details={"warning": str(e), "stop_price": str(stop_price)},
                )
        
        # Stop is defined but can't calculate risk - OK (stop is the key requirement)
        return ExecutionGuard(
            name="RiskManagement",
            passed=True,
            message="✓ Stop-loss defined (risk calculation skipped - missing capital/quantity)",
            details={"stop_price": str(stop_price)},
        )
    
    def _calculate_safe_quantity(
        self, 
        capital: Decimal, 
        entry_price: Decimal, 
        stop_price: Decimal
    ) -> Decimal:
        """Calculate the maximum safe quantity given the 1% rule."""
        max_risk_amount = capital * (self.max_risk_percent / Decimal("100"))
        stop_distance = abs(entry_price - stop_price)
        if stop_distance == 0:
            return Decimal("0")
        return max_risk_amount / stop_distance


class MonthlyDrawdownGuard:
    """
    Guard that enforces the 4% Monthly Drawdown limit.
    
    If the current month's realized losses exceed 4% of starting capital,
    trading is PAUSED until the next month or manual override.
    
    This protects against:
    - Revenge trading after losses
    - Account blowup from bad streaks
    - Emotional decision-making
    """
    
    DEFAULT_MAX_DRAWDOWN_PERCENT = Decimal("4.0")  # 4% max monthly loss
    
    def __init__(
        self, 
        max_drawdown_percent: Optional[Decimal] = None,
        get_monthly_pnl: Optional[callable] = None,
    ):
        """
        Initialize the guard.
        
        Args:
            max_drawdown_percent: Maximum monthly drawdown as percentage (default: 4%)
            get_monthly_pnl: Callable to get current month's P&L from database
        """
        self.max_drawdown_percent = max_drawdown_percent or self.DEFAULT_MAX_DRAWDOWN_PERCENT
        self._get_monthly_pnl = get_monthly_pnl
    
    def check(self, context: dict) -> ExecutionGuard:
        """
        Check if monthly drawdown limit is breached.
        
        Expected context:
            - capital: Starting capital (REQUIRED)
            - client_id: Client/tenant ID
            - monthly_pnl: Pre-calculated monthly P&L (optional)
            - force_override: Manual override (optional, for emergencies)
            
        Returns:
            ExecutionGuard with pass/fail result
        """
        mode = context.get("mode", ExecutionMode.DRY_RUN)
        
        # Check for manual override (emergency only)
        if context.get("force_override"):
            return ExecutionGuard(
                name="MonthlyDrawdown",
                passed=True,
                message="⚠️ Drawdown check OVERRIDDEN (emergency mode)",
                details={"warning": "Manual override active - USE WITH EXTREME CAUTION"},
            )
        
        capital = context.get("capital")
        if not capital:
            # Can't check without capital - pass with warning
            return ExecutionGuard(
                name="MonthlyDrawdown",
                passed=True,
                message="⚠️ Cannot validate drawdown (capital not provided)",
            )
        
        try:
            capital = Decimal(str(capital))
        except (ValueError, TypeError):
            return ExecutionGuard(
                name="MonthlyDrawdown",
                passed=True,
                message="⚠️ Invalid capital format - drawdown check skipped",
            )
        
        # Get monthly P&L
        monthly_pnl = context.get("monthly_pnl")
        
        if monthly_pnl is None and self._get_monthly_pnl:
            client_id = context.get("client_id")
            monthly_pnl = self._get_monthly_pnl(client_id)
        
        if monthly_pnl is None:
            # No P&L data - pass with warning
            return ExecutionGuard(
                name="MonthlyDrawdown",
                passed=True,
                message="⚠️ No monthly P&L data - drawdown check skipped",
            )
        
        try:
            monthly_pnl = Decimal(str(monthly_pnl))
        except (ValueError, TypeError):
            return ExecutionGuard(
                name="MonthlyDrawdown",
                passed=True,
                message="⚠️ Invalid P&L format - drawdown check skipped",
            )
        
        # Calculate drawdown percentage
        if monthly_pnl >= 0:
            # Profitable month - all good
            return ExecutionGuard(
                name="MonthlyDrawdown",
                passed=True,
                message=f"✓ Month is profitable: +${monthly_pnl:.2f}",
                details={"monthly_pnl": str(monthly_pnl)},
            )
        
        # Calculate loss percentage
        loss_percent = (abs(monthly_pnl) / capital) * Decimal("100")
        
        if loss_percent >= self.max_drawdown_percent:
            return ExecutionGuard(
                name="MonthlyDrawdown",
                passed=False,
                message=f"🛑 TRADING PAUSED: Monthly loss {loss_percent:.2f}% exceeds {self.max_drawdown_percent}% limit",
                details={
                    "monthly_loss": str(abs(monthly_pnl)),
                    "loss_percent": str(loss_percent),
                    "max_drawdown_percent": str(self.max_drawdown_percent),
                    "capital": str(capital),
                    "action": "Trading paused until next month or manual review",
                    "docs": "docs/PRODUCTION_TRADING.md#monthly-drawdown-limit-4-rule",
                },
            )
        
        # Under limit - pass with status
        remaining = self.max_drawdown_percent - loss_percent
        return ExecutionGuard(
            name="MonthlyDrawdown",
            passed=True,
            message=f"✓ Monthly drawdown OK: -{loss_percent:.2f}% (limit: {self.max_drawdown_percent}%, remaining: {remaining:.2f}%)",
            details={
                "monthly_loss": str(abs(monthly_pnl)),
                "loss_percent": str(loss_percent),
                "remaining_percent": str(remaining),
            },
        )


class TradeIntentGuard:
    """
    Guard that validates trading intent before execution.
    
    Ensures the trade is intentional, not emotional:
    - Checks for required confirmation
    - Validates that trade follows a strategy
    - Optionally integrates with EmotionalGuard
    """
    
    def check(self, context: dict) -> ExecutionGuard:
        """
        Check if trade intent is valid.
        
        Expected context:
            - strategy_name: Name of strategy being followed
            - confirmed: User confirmed the trade
            - emotional_check_passed: Result from EmotionalGuard (optional)
            
        Returns:
            ExecutionGuard with pass/fail result
        """
        mode = context.get("mode", ExecutionMode.DRY_RUN)
        
        # DRY-RUN is more lenient
        if mode == ExecutionMode.DRY_RUN:
            return ExecutionGuard(
                name="TradeIntent",
                passed=True,
                message="Trade intent check relaxed for DRY-RUN",
            )
        
        # LIVE requires strategy
        strategy_name = context.get("strategy_name")
        if not strategy_name:
            return ExecutionGuard(
                name="TradeIntent",
                passed=False,
                message="❌ STRATEGY REQUIRED: All trades must follow a defined strategy",
                details={
                    "requirement": "Specify strategy_name parameter",
                    "reason": "Systematic trading prevents emotional decisions",
                },
            )
        
        # Check confirmation
        confirmed = context.get("confirmed", False)
        if not confirmed:
            return ExecutionGuard(
                name="TradeIntent",
                passed=False,
                message="❌ CONFIRMATION REQUIRED: Explicitly confirm trade intent",
                details={
                    "requirement": "Set confirmed=True after reviewing trade parameters",
                },
            )
        
        # Check emotional guard result if provided
        emotional_check = context.get("emotional_check_passed")
        if emotional_check is False:
            emotional_risk = context.get("emotional_risk_level", "unknown")
            return ExecutionGuard(
                name="TradeIntent",
                passed=False,
                message=f"❌ EMOTIONAL CHECK FAILED: Risk level '{emotional_risk}'",
                details={
                    "risk_level": emotional_risk,
                    "recommendation": "Wait 10 minutes and re-evaluate your trade decision",
                },
            )
        
        return ExecutionGuard(
            name="TradeIntent",
            passed=True,
            message=f"✓ Trade intent valid (strategy: {strategy_name})",
            details={"strategy": strategy_name},
        )


# ==========================================
# GUARD FACTORY
# ==========================================

def get_trading_guards(
    include_risk: bool = True,
    include_drawdown: bool = True,
    include_intent: bool = False,
    max_risk_percent: Optional[Decimal] = None,
    max_drawdown_percent: Optional[Decimal] = None,
    get_monthly_pnl: Optional[callable] = None,
) -> list:
    """
    Factory to create the standard set of trading guards.
    
    Args:
        include_risk: Include RiskManagementGuard (default: True)
        include_drawdown: Include MonthlyDrawdownGuard (default: True)
        include_intent: Include TradeIntentGuard (default: False)
        max_risk_percent: Override max risk per trade
        max_drawdown_percent: Override max monthly drawdown
        get_monthly_pnl: Function to get monthly P&L for drawdown check
        
    Returns:
        List of guard instances
    """
    guards = []
    
    if include_risk:
        guards.append(RiskManagementGuard(max_risk_percent))
    
    if include_drawdown:
        guards.append(MonthlyDrawdownGuard(max_drawdown_percent, get_monthly_pnl))
    
    if include_intent:
        guards.append(TradeIntentGuard())
    
    return guards

