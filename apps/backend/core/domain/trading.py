"""
Trading Domain Models

Pure business entities for systematic trading.
No framework dependencies.
"""

from dataclasses import dataclass, field
from decimal import Decimal
from datetime import datetime
from typing import Optional
from enum import Enum


# ============================================================================
# Enums
# ============================================================================

class IntentStatus(str, Enum):
    """Status of a trading intent."""
    PENDING = "PENDING"  # Created, awaiting validation
    VALIDATED = "VALIDATED"  # Passed risk checks
    EXECUTING = "EXECUTING"  # Order being placed
    EXECUTED = "EXECUTED"  # Order placed successfully
    FAILED = "FAILED"  # Execution failed
    CANCELLED = "CANCELLED"  # Manually cancelled


class PolicyStatus(str, Enum):
    """Status of trading policy."""
    ACTIVE = "ACTIVE"  # Trading allowed
    PAUSED = "PAUSED"  # Trading paused (risk limit hit)
    SUSPENDED = "SUSPENDED"  # Manually suspended


# ============================================================================
# Trading Intent (Systematic Trading Decision)
# ============================================================================

@dataclass
class TradingIntent:
    """
    A trading intent represents a systematic trading decision.

    This is the "machine decision record" - what the algorithm decided to do,
    before validation and execution. It's immutable for audit trail purposes.

    Lifecycle:
    1. PENDING: Created by DecisionEngine
    2. VALIDATED: Passed RiskPolicy checks
    3. EXECUTING: Order being placed
    4. EXECUTED: Order placed on exchange
    5. FAILED/CANCELLED: Did not execute

    Key Principle: Intent is separate from Operation.
    - Intent = what the SYSTEM decided
    - Operation = what the USER configured (may be systematic or manual)
    """
    intent_id: str
    client_id: int
    symbol: str
    side: str  # "BUY" or "SELL"
    status: IntentStatus

    # Trading parameters
    quantity: Decimal
    entry_price: Decimal
    stop_price: Decimal
    target_price: Optional[Decimal] = None

    # Decision context (WHY this intent was created)
    strategy_name: str
    regime: str  # "bull", "bear", "sideways"
    confidence: float  # 0.0 to 1.0
    reason: str  # Human-readable explanation

    # Timestamps
    created_at: datetime
    validated_at: Optional[datetime] = None
    executed_at: Optional[datetime] = None

    # Execution results (populated after execution)
    order_id: Optional[str] = None
    exchange_order_id: Optional[str] = None
    actual_fill_price: Optional[Decimal] = None
    actual_fill_quantity: Optional[Decimal] = None

    # Risk calculation (computed before validation)
    risk_amount: Decimal = Decimal("0")  # Max loss in quote currency
    risk_percent: Decimal = Decimal("0")  # % of capital at risk

    # Event correlation (for tracing)
    correlation_id: Optional[str] = None

    # Validation/execution errors
    error_message: Optional[str] = None

    def __post_init__(self):
        """Validate invariants."""
        if self.quantity <= 0:
            raise ValueError("Quantity must be positive")
        if self.entry_price <= 0:
            raise ValueError("Entry price must be positive")
        if self.side == "BUY" and self.stop_price >= self.entry_price:
            raise ValueError("Stop price for BUY must be below entry price")
        if self.side == "SELL" and self.stop_price <= self.entry_price:
            raise ValueError("Stop price for SELL must be above entry price")
        if not (0.0 <= self.confidence <= 1.0):
            raise ValueError("Confidence must be between 0.0 and 1.0")

    @property
    def is_pending(self) -> bool:
        return self.status == IntentStatus.PENDING

    @property
    def is_executed(self) -> bool:
        return self.status == IntentStatus.EXECUTED

    @property
    def is_failed(self) -> bool:
        return self.status in {IntentStatus.FAILED, IntentStatus.CANCELLED}

    @property
    def position_value(self) -> Decimal:
        """Total value of position (entry_price * quantity)."""
        return self.entry_price * self.quantity

    @property
    def stop_distance(self) -> Decimal:
        """Distance from entry to stop (always positive)."""
        return abs(self.entry_price - self.stop_price)

    @property
    def stop_distance_percent(self) -> Decimal:
        """Stop distance as % of entry price."""
        if self.entry_price == 0:
            return Decimal("0")
        return (self.stop_distance / self.entry_price) * Decimal("100")

    def mark_as_validated(self, timestamp: datetime) -> 'TradingIntent':
        """Return new intent with VALIDATED status."""
        return dataclass_replace(
            self,
            status=IntentStatus.VALIDATED,
            validated_at=timestamp,
        )

    def mark_as_executing(self) -> 'TradingIntent':
        """Return new intent with EXECUTING status."""
        return dataclass_replace(self, status=IntentStatus.EXECUTING)

    def mark_as_executed(
        self,
        timestamp: datetime,
        order_id: str,
        exchange_order_id: str,
        fill_price: Decimal,
        fill_quantity: Decimal,
    ) -> 'TradingIntent':
        """Return new intent with EXECUTED status and fill details."""
        return dataclass_replace(
            self,
            status=IntentStatus.EXECUTED,
            executed_at=timestamp,
            order_id=order_id,
            exchange_order_id=exchange_order_id,
            actual_fill_price=fill_price,
            actual_fill_quantity=fill_quantity,
        )

    def mark_as_failed(self, error_message: str) -> 'TradingIntent':
        """Return new intent with FAILED status."""
        return dataclass_replace(
            self,
            status=IntentStatus.FAILED,
            error_message=error_message,
        )


# Helper for immutable updates (simulates dataclasses.replace for older Python)
def dataclass_replace(obj, **changes):
    """Create new dataclass instance with updated fields."""
    from dataclasses import asdict, fields
    current = asdict(obj)
    current.update(changes)
    return type(obj)(**current)


# ============================================================================
# Policy State (Risk Limits Tracking)
# ============================================================================

@dataclass
class PolicyState:
    """
    Tracks risk policy state for a client in a given month.

    This is the "high-water mark" for risk limits:
    - Monthly drawdown (max 4% loss per month)
    - Daily trade count
    - Pause states

    Key Principle: One PolicyState per client per month.
    Each month resets limits (fresh start).
    """
    client_id: int
    month: str  # Format: "YYYY-MM"
    status: PolicyStatus

    # Monthly P&L tracking
    starting_capital: Decimal  # Capital at start of month
    current_capital: Decimal  # Current capital (includes unrealized P&L)
    realized_pnl: Decimal = Decimal("0")  # Realized P&L for month
    unrealized_pnl: Decimal = Decimal("0")  # Unrealized P&L (open positions)

    # Trade statistics
    total_trades: int = 0
    winning_trades: int = 0
    losing_trades: int = 0

    # Risk limits
    max_drawdown_percent: Decimal = Decimal("4.0")  # Max monthly drawdown
    max_trades_per_day: int = 50  # Limit for medium-frequency

    # Pause tracking
    paused_at: Optional[datetime] = None
    pause_reason: Optional[str] = None

    # Timestamps
    created_at: Optional[datetime] = None
    updated_at: Optional[datetime] = None

    @property
    def total_pnl(self) -> Decimal:
        """Total P&L (realized + unrealized)."""
        return self.realized_pnl + self.unrealized_pnl

    @property
    def drawdown_percent(self) -> Decimal:
        """Current drawdown as % of starting capital."""
        if self.starting_capital == 0:
            return Decimal("0")
        drawdown = self.starting_capital - self.current_capital
        return (drawdown / self.starting_capital) * Decimal("100")

    @property
    def win_rate(self) -> float:
        """Win rate as percentage."""
        if self.total_trades == 0:
            return 0.0
        return (self.winning_trades / self.total_trades) * 100.0

    @property
    def is_paused(self) -> bool:
        """Check if trading is paused."""
        return self.status == PolicyStatus.PAUSED

    @property
    def is_drawdown_limit_exceeded(self) -> bool:
        """Check if drawdown limit is exceeded."""
        return self.drawdown_percent >= self.max_drawdown_percent

    def pause(self, reason: str, timestamp: datetime) -> 'PolicyState':
        """Pause trading."""
        return dataclass_replace(
            self,
            status=PolicyStatus.PAUSED,
            paused_at=timestamp,
            pause_reason=reason,
            updated_at=timestamp,
        )

    def resume(self, timestamp: datetime) -> 'PolicyState':
        """Resume trading."""
        return dataclass_replace(
            self,
            status=PolicyStatus.ACTIVE,
            paused_at=None,
            pause_reason=None,
            updated_at=timestamp,
        )

    def update_pnl(
        self,
        realized_pnl_delta: Decimal,
        unrealized_pnl: Decimal,
        timestamp: datetime,
    ) -> 'PolicyState':
        """Update P&L and check limits."""
        new_realized = self.realized_pnl + realized_pnl_delta
        new_current_capital = self.starting_capital + new_realized + unrealized_pnl

        new_state = dataclass_replace(
            self,
            realized_pnl=new_realized,
            unrealized_pnl=unrealized_pnl,
            current_capital=new_current_capital,
            updated_at=timestamp,
        )

        # Auto-pause if drawdown limit exceeded
        if new_state.is_drawdown_limit_exceeded and not new_state.is_paused:
            return new_state.pause(
                reason=f"Monthly drawdown limit exceeded ({new_state.drawdown_percent:.2f}% >= {new_state.max_drawdown_percent}%)",
                timestamp=timestamp,
            )

        return new_state


# ============================================================================
# Execution Event (Audit Trail)
# ============================================================================

@dataclass(frozen=True)
class ExecutionEvent:
    """
    Immutable record of a state transition.

    Every action in the system generates an ExecutionEvent for auditability.

    Examples:
    - IntentCreated: Decision engine created trading intent
    - IntentValidated: Risk policy approved intent
    - OrderPlaced: Order was placed on exchange
    - OrderFilled: Order was filled
    - PolicyPaused: Trading was paused due to risk limit
    """
    event_id: str
    event_type: str  # "intent_created", "order_placed", etc.
    timestamp: datetime

    # What was affected
    aggregate_type: str  # "TradingIntent", "Order", "PolicyState"
    aggregate_id: str  # ID of the entity

    # Who triggered it
    actor: str  # "system" or "user:{user_id}"

    # What happened
    data: dict  # Event-specific data
    reason: str  # Human-readable explanation

    # Correlation (for tracing related events)
    correlation_id: Optional[str] = None
    causation_id: Optional[str] = None  # ID of event that caused this one

    @property
    def is_system_event(self) -> bool:
        """Check if event was triggered by system."""
        return self.actor == "system"

    @property
    def is_user_event(self) -> bool:
        """Check if event was triggered by user."""
        return self.actor.startswith("user:")
