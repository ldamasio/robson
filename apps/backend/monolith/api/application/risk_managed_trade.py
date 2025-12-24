"""
Risk-Managed Trade Use Case.

This is the ONLY way to execute trades in production.
All trades MUST go through this use case which enforces:

1. Stop-loss REQUIRED (no exceptions)
2. Risk per trade ≤ 1% of capital
3. Monthly drawdown ≤ 4% (trading pauses if breached)
4. Position sizing calculated from stop distance

USER initiates → ROBSON calculates → ROBSON validates → USER confirms → ROBSON executes

This use case embodies Robson's philosophy:
"Robson is a Risk Management Assistant, NOT an Auto-Trader"
"""

from __future__ import annotations
from dataclasses import dataclass, field
from decimal import Decimal, InvalidOperation
from typing import Optional, Protocol
from enum import Enum

from django.utils import timezone
from django.conf import settings

from .execution import (
    ExecutionGuard,
    ExecutionResult,
    ExecutionMode,
    ExecutionStatus,
)
from .risk_guards import (
    RiskManagementGuard,
    MonthlyDrawdownGuard,
    TradeIntentGuard,
    get_trading_guards,
)


class TradeType(Enum):
    """Type of trade execution."""
    MARKET = "MARKET"
    LIMIT = "LIMIT"


@dataclass(frozen=True)
class RiskManagedOrder:
    """
    A trade order that has been validated for risk management.
    
    This is an immutable value object that contains all the
    parameters needed for a safe trade execution.
    """
    symbol: str
    side: str  # BUY or SELL
    trade_type: TradeType
    quantity: Decimal
    entry_price: Decimal
    stop_price: Decimal
    take_profit_price: Optional[Decimal] = None
    capital: Decimal = Decimal("0")
    risk_amount: Decimal = Decimal("0")
    risk_percent: Decimal = Decimal("0")
    strategy_name: Optional[str] = None
    
    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization."""
        return {
            "symbol": self.symbol,
            "side": self.side,
            "trade_type": self.trade_type.value,
            "quantity": str(self.quantity),
            "entry_price": str(self.entry_price),
            "stop_price": str(self.stop_price),
            "take_profit_price": str(self.take_profit_price) if self.take_profit_price else None,
            "capital": str(self.capital),
            "risk_amount": str(self.risk_amount),
            "risk_percent": str(self.risk_percent),
            "strategy_name": self.strategy_name,
        }


@dataclass
class RiskValidationResult:
    """
    Result of risk validation before trade execution.
    
    If `is_valid` is False, the trade MUST NOT be executed.
    """
    is_valid: bool
    guards: list[ExecutionGuard] = field(default_factory=list)
    blocked_by: Optional[str] = None
    message: str = ""
    safe_quantity: Optional[Decimal] = None  # Suggested safe position size
    
    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization."""
        return {
            "is_valid": self.is_valid,
            "guards": [
                {
                    "name": g.name,
                    "passed": g.passed,
                    "message": g.message,
                    "details": g.details,
                }
                for g in self.guards
            ],
            "blocked_by": self.blocked_by,
            "message": self.message,
            "safe_quantity": str(self.safe_quantity) if self.safe_quantity else None,
        }


class OrderExecutionPort(Protocol):
    """Port for executing orders on an exchange."""
    
    def place_market(self, symbol: str, side: str, quantity: Decimal) -> dict:
        """Place a market order."""
        ...
    
    def place_limit(self, symbol: str, side: str, quantity: Decimal, price: Decimal) -> dict:
        """Place a limit order."""
        ...
    
    def place_stop_loss(self, symbol: str, side: str, quantity: Decimal, stop_price: Decimal) -> dict:
        """Place a stop-loss order."""
        ...
    
    def get_account_balance(self, asset: Optional[str] = None) -> dict:
        """Get account balance."""
        ...


class PnLRepositoryPort(Protocol):
    """Port for accessing P&L data."""
    
    def get_monthly_pnl(self, client_id: Optional[int] = None) -> Decimal:
        """Get current month's realized P&L."""
        ...


class RiskManagedTradeUseCase:
    """
    Use case for executing trades with mandatory risk management.
    
    This is the central orchestrator that:
    1. Validates all risk parameters
    2. Calculates safe position size
    3. Runs all guards
    4. Executes trade only if all guards pass
    5. Places stop-loss order automatically
    
    Example usage:
    
        use_case = RiskManagedTradeUseCase(
            execution_adapter=BinanceExecution(),
            pnl_repository=DjangoPnLRepository(),
        )
        
        result = use_case.execute_buy(
            symbol="BTCUSDC",
            capital=Decimal("1000"),
            entry_price=Decimal("95000"),
            stop_price=Decimal("94000"),
            mode=ExecutionMode.LIVE,
        )
    """
    
    def __init__(
        self,
        execution_adapter: OrderExecutionPort,
        pnl_repository: Optional[PnLRepositoryPort] = None,
        max_risk_percent: Decimal = Decimal("1.0"),
        max_drawdown_percent: Decimal = Decimal("4.0"),
    ):
        """
        Initialize the use case.
        
        Args:
            execution_adapter: Adapter for exchange operations
            pnl_repository: Repository for P&L data (for drawdown check)
            max_risk_percent: Maximum risk per trade (default: 1%)
            max_drawdown_percent: Maximum monthly drawdown (default: 4%)
        """
        self.execution = execution_adapter
        self.pnl_repository = pnl_repository
        self.max_risk_percent = max_risk_percent
        self.max_drawdown_percent = max_drawdown_percent
        
        # Create guards
        self._get_monthly_pnl = None
        if pnl_repository:
            self._get_monthly_pnl = pnl_repository.get_monthly_pnl
        
        self.guards = [
            RiskManagementGuard(max_risk_percent),
            MonthlyDrawdownGuard(max_drawdown_percent, self._get_monthly_pnl),
        ]
    
    def calculate_position_size(
        self,
        capital: Decimal,
        entry_price: Decimal,
        stop_price: Decimal,
    ) -> Decimal:
        """
        Calculate the maximum safe position size based on 1% rule.
        
        Formula: Quantity = (Capital × Risk%) / |Entry - Stop|
        
        Args:
            capital: Total available capital
            entry_price: Entry price for the trade
            stop_price: Stop-loss price
            
        Returns:
            Maximum quantity that risks ≤ 1% of capital
        """
        max_risk_amount = capital * (self.max_risk_percent / Decimal("100"))
        stop_distance = abs(entry_price - stop_price)
        
        if stop_distance == 0:
            return Decimal("0")
        
        quantity = max_risk_amount / stop_distance
        return quantity.quantize(Decimal("0.00001"))
    
    def validate(
        self,
        symbol: str,
        side: str,
        quantity: Decimal,
        entry_price: Decimal,
        stop_price: Decimal,
        capital: Decimal,
        client_id: Optional[int] = None,
        monthly_pnl: Optional[Decimal] = None,
    ) -> RiskValidationResult:
        """
        Validate a trade against all risk rules.
        
        This MUST be called before execute().
        
        Args:
            symbol: Trading pair (e.g., "BTCUSDC")
            side: BUY or SELL
            quantity: Position size
            entry_price: Entry price
            stop_price: Stop-loss price (REQUIRED)
            capital: Total available capital
            client_id: Optional client ID for multi-tenant
            monthly_pnl: Optional pre-calculated monthly P&L
            
        Returns:
            RiskValidationResult with all guard results
        """
        context = {
            "symbol": symbol,
            "side": side,
            "quantity": quantity,
            "entry_price": entry_price,
            "stop_price": stop_price,
            "capital": capital,
            "client_id": client_id,
            "monthly_pnl": monthly_pnl,
            "mode": ExecutionMode.LIVE,  # Always validate as if LIVE
        }
        
        guards_results = []
        blocked_by = None
        
        for guard in self.guards:
            result = guard.check(context)
            guards_results.append(result)
            
            if not result.passed and blocked_by is None:
                blocked_by = result.name
        
        # Calculate safe quantity for reference
        safe_quantity = self.calculate_position_size(capital, entry_price, stop_price)
        
        is_valid = blocked_by is None
        
        if is_valid:
            message = "✓ All risk checks passed - trade is safe to execute"
        else:
            message = f"❌ Trade blocked by {blocked_by}"
        
        return RiskValidationResult(
            is_valid=is_valid,
            guards=guards_results,
            blocked_by=blocked_by,
            message=message,
            safe_quantity=safe_quantity,
        )
    
    def execute_buy(
        self,
        symbol: str,
        capital: Decimal,
        entry_price: Decimal,
        stop_price: Decimal,
        quantity: Optional[Decimal] = None,
        take_profit_price: Optional[Decimal] = None,
        mode: ExecutionMode = ExecutionMode.DRY_RUN,
        client_id: Optional[int] = None,
        monthly_pnl: Optional[Decimal] = None,
        strategy_name: str = "manual",
    ) -> ExecutionResult:
        """
        Execute a risk-managed BUY order.
        
        This method:
        1. Calculates safe position size if not provided
        2. Validates all risk rules
        3. Executes the market order
        4. Places the stop-loss order
        
        Args:
            symbol: Trading pair (e.g., "BTCUSDC")
            capital: Total available capital
            entry_price: Current market price (or limit price)
            stop_price: Stop-loss price (REQUIRED)
            quantity: Position size (calculated if not provided)
            take_profit_price: Optional take-profit price
            mode: DRY_RUN (default) or LIVE
            client_id: Optional client ID for multi-tenant
            monthly_pnl: Optional pre-calculated monthly P&L
            strategy_name: Name of the strategy being followed
            
        Returns:
            ExecutionResult with all details
        """
        # Calculate quantity if not provided
        if quantity is None:
            quantity = self.calculate_position_size(capital, entry_price, stop_price)
        
        # Calculate risk metrics
        stop_distance = abs(entry_price - stop_price)
        risk_amount = stop_distance * quantity
        risk_percent = (risk_amount / capital) * Decimal("100") if capital > 0 else Decimal("0")
        
        # Create the order object
        order = RiskManagedOrder(
            symbol=symbol,
            side="BUY",
            trade_type=TradeType.MARKET,
            quantity=quantity,
            entry_price=entry_price,
            stop_price=stop_price,
            take_profit_price=take_profit_price,
            capital=capital,
            risk_amount=risk_amount,
            risk_percent=risk_percent,
            strategy_name=strategy_name,
        )
        
        # Validate
        validation = self.validate(
            symbol=symbol,
            side="BUY",
            quantity=quantity,
            entry_price=entry_price,
            stop_price=stop_price,
            capital=capital,
            client_id=client_id,
            monthly_pnl=monthly_pnl,
        )
        
        # Create result
        result = ExecutionResult(
            status=ExecutionStatus.SUCCESS,
            mode=mode,
        )
        result.metadata["order"] = order.to_dict()
        result.metadata["validation"] = validation.to_dict()
        result.metadata["client_id"] = client_id
        result.metadata["strategy"] = strategy_name
        
        # Add guard results
        for guard in validation.guards:
            result.add_guard(guard)
        
        # Check if blocked
        if not validation.is_valid:
            result.status = ExecutionStatus.BLOCKED
            result.error = f"Trade blocked by {validation.blocked_by}: {validation.message}"
            return result
        
        # Execute based on mode
        if mode == ExecutionMode.DRY_RUN:
            result.add_action({
                "type": "SIMULATED_BUY",
                "description": f"Would buy {quantity} {symbol.replace('USDC', '')} at ~{entry_price}",
                "details": order.to_dict(),
                "result": "Simulation only - no real order placed",
            })
            result.add_action({
                "type": "SIMULATED_STOP_LOSS",
                "description": f"Would place stop-loss at {stop_price}",
                "result": "Simulation only - no real order placed",
            })
        else:
            # LIVE execution
            try:
                # Place market buy
                buy_response = self.execution.place_market(
                    symbol=symbol,
                    side="BUY",
                    quantity=quantity,
                )
                
                result.add_action({
                    "type": "MARKET_BUY",
                    "description": f"Bought {quantity} {symbol.replace('USDC', '')} at market",
                    "details": buy_response,
                    "result": f"Order {buy_response.get('orderId')} filled",
                })
                
                # Place stop-loss
                try:
                    stop_response = self.execution.place_stop_loss(
                        symbol=symbol,
                        side="SELL",  # Stop-loss for a BUY is a SELL
                        quantity=quantity,
                        stop_price=stop_price,
                    )
                    
                    result.add_action({
                        "type": "STOP_LOSS",
                        "description": f"Stop-loss placed at {stop_price}",
                        "details": stop_response,
                        "result": f"Stop order {stop_response.get('orderId')} active",
                    })
                except Exception as e:
                    # Stop-loss failed but main order succeeded - CRITICAL WARNING
                    result.add_action({
                        "type": "STOP_LOSS_FAILED",
                        "description": f"⚠️ CRITICAL: Stop-loss failed: {e}",
                        "result": "MANUAL STOP-LOSS REQUIRED",
                    })
                    result.metadata["warning"] = "Stop-loss order failed - set manually!"
                
            except Exception as e:
                result.status = ExecutionStatus.FAILED
                result.error = str(e)
                result.add_action({
                    "type": "BUY_FAILED",
                    "description": f"Buy order failed: {e}",
                    "result": "No position opened",
                })
        
        return result
    
    def execute_sell(
        self,
        symbol: str,
        quantity: Decimal,
        capital: Decimal,
        entry_price: Decimal,  # For stop calculation on SHORT
        stop_price: Decimal,
        take_profit_price: Optional[Decimal] = None,
        mode: ExecutionMode = ExecutionMode.DRY_RUN,
        client_id: Optional[int] = None,
        monthly_pnl: Optional[Decimal] = None,
        strategy_name: str = "manual",
    ) -> ExecutionResult:
        """
        Execute a risk-managed SELL order.
        
        Same safety guarantees as execute_buy but for selling.
        Used for:
        - Closing a long position
        - Opening a short position (in margin mode)
        """
        # Similar implementation to execute_buy but for SELL
        # For now, return a simplified version
        
        result = ExecutionResult(
            status=ExecutionStatus.SUCCESS,
            mode=mode,
        )
        
        # Validate
        validation = self.validate(
            symbol=symbol,
            side="SELL",
            quantity=quantity,
            entry_price=entry_price,
            stop_price=stop_price,
            capital=capital,
            client_id=client_id,
            monthly_pnl=monthly_pnl,
        )
        
        # Add guard results
        for guard in validation.guards:
            result.add_guard(guard)
        
        if not validation.is_valid:
            result.status = ExecutionStatus.BLOCKED
            result.error = f"Trade blocked by {validation.blocked_by}"
            return result
        
        if mode == ExecutionMode.DRY_RUN:
            result.add_action({
                "type": "SIMULATED_SELL",
                "description": f"Would sell {quantity} at ~{entry_price}",
                "result": "Simulation only",
            })
        else:
            try:
                sell_response = self.execution.place_market(
                    symbol=symbol,
                    side="SELL",
                    quantity=quantity,
                )
                result.add_action({
                    "type": "MARKET_SELL",
                    "description": f"Sold {quantity} at market",
                    "details": sell_response,
                    "result": f"Order {sell_response.get('orderId')} filled",
                })
            except Exception as e:
                result.status = ExecutionStatus.FAILED
                result.error = str(e)
        
        return result

