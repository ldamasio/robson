"""
Isolated Margin Trading Domain Model

Pure business entities for margin trading.
No framework dependencies.

Key Principles:
- Each position has ISOLATED margin (risk limited to allocated margin only)
- Position sizing follows 1% risk rule
- Stop-loss is MANDATORY for all positions
- Monthly drawdown limit of 4% enforced by PolicyState
"""

from dataclasses import dataclass, field
from decimal import Decimal
from datetime import datetime
from typing import Optional
from enum import Enum


# ============================================================================
# Enums
# ============================================================================

class MarginPositionStatus(str, Enum):
    """Status of a margin position."""
    PENDING = "PENDING"          # Order placed, awaiting fill
    OPEN = "OPEN"                # Position is active
    CLOSING = "CLOSING"          # Close order placed
    CLOSED = "CLOSED"            # Position closed normally
    STOPPED_OUT = "STOPPED_OUT"  # Closed by stop-loss
    TAKE_PROFIT = "TAKE_PROFIT"  # Closed by take-profit
    LIQUIDATED = "LIQUIDATED"    # Forced liquidation by exchange


class MarginSide(str, Enum):
    """Position side."""
    LONG = "LONG"    # Buy first, sell to close (profit when price goes up)
    SHORT = "SHORT"  # Sell first, buy to close (profit when price goes down)


class MarginLevel(str, Enum):
    """Margin health level classifications."""
    SAFE = "SAFE"          # margin_level >= 2.0
    CAUTION = "CAUTION"    # 1.5 <= margin_level < 2.0
    WARNING = "WARNING"    # 1.3 <= margin_level < 1.5
    CRITICAL = "CRITICAL"  # 1.1 <= margin_level < 1.3
    DANGER = "DANGER"      # margin_level < 1.1


# ============================================================================
# Value Objects
# ============================================================================

@dataclass(frozen=True)
class MarginAccountInfo:
    """
    Snapshot of Isolated Margin account status for a symbol.
    
    Immutable value object retrieved from exchange.
    Used for position sizing and risk calculations.
    """
    symbol: str
    
    # Base asset (e.g., BTC)
    base_asset: str
    base_free: Decimal
    base_locked: Decimal
    base_borrowed: Decimal
    base_interest: Decimal
    
    # Quote asset (e.g., USDC)
    quote_asset: str
    quote_free: Decimal
    quote_locked: Decimal
    quote_borrowed: Decimal
    quote_interest: Decimal
    
    # Margin status
    margin_level: Decimal
    liquidation_price: Decimal
    
    # Trading enabled
    is_margin_trade_enabled: bool
    
    @property
    def total_base(self) -> Decimal:
        """Total base asset value."""
        return self.base_free + self.base_locked
    
    @property
    def total_quote(self) -> Decimal:
        """Total quote asset value."""
        return self.quote_free + self.quote_locked
    
    @property
    def available_quote(self) -> Decimal:
        """Available quote for new positions."""
        return self.quote_free
    
    @property
    def total_borrowed(self) -> Decimal:
        """Total borrowed amount (base + quote equivalent)."""
        return self.base_borrowed + self.quote_borrowed
    
    @property
    def total_interest(self) -> Decimal:
        """Total interest owed."""
        return self.base_interest + self.quote_interest
    
    @property
    def health_level(self) -> MarginLevel:
        """Classify margin health level."""
        if self.margin_level >= Decimal("2.0"):
            return MarginLevel.SAFE
        elif self.margin_level >= Decimal("1.5"):
            return MarginLevel.CAUTION
        elif self.margin_level >= Decimal("1.3"):
            return MarginLevel.WARNING
        elif self.margin_level >= Decimal("1.1"):
            return MarginLevel.CRITICAL
        else:
            return MarginLevel.DANGER
    
    @property
    def is_healthy(self) -> bool:
        """Check if margin level is healthy (>= 1.5)."""
        return self.margin_level >= Decimal("1.5")
    
    @property
    def can_open_position(self) -> bool:
        """Check if new positions can be opened."""
        return self.is_margin_trade_enabled and self.margin_level >= Decimal("2.0")


@dataclass(frozen=True)
class TransferResult:
    """Result of a transfer operation between Spot and Margin."""
    success: bool
    transaction_id: Optional[str]
    asset: str
    amount: Decimal
    from_account: str
    to_account: str
    timestamp: datetime = field(default_factory=datetime.utcnow)
    error_message: Optional[str] = None
    
    @property
    def is_to_margin(self) -> bool:
        """Check if transfer was to margin account."""
        return "MARGIN" in self.to_account
    
    @property
    def is_from_margin(self) -> bool:
        """Check if transfer was from margin account."""
        return "MARGIN" in self.from_account


@dataclass(frozen=True)
class MarginOrderResult:
    """Result of a margin order placement."""
    success: bool
    order_id: Optional[str]
    binance_order_id: Optional[str]
    symbol: str
    side: str
    order_type: str
    quantity: Decimal
    price: Optional[Decimal] = None
    stop_price: Optional[Decimal] = None
    filled_quantity: Decimal = Decimal("0")
    avg_fill_price: Optional[Decimal] = None
    status: str = "NEW"
    timestamp: datetime = field(default_factory=datetime.utcnow)
    error_message: Optional[str] = None
    
    @property
    def is_filled(self) -> bool:
        """Check if order is fully filled."""
        return self.filled_quantity >= self.quantity
    
    @property
    def is_partially_filled(self) -> bool:
        """Check if order is partially filled."""
        return Decimal("0") < self.filled_quantity < self.quantity
    
    @property
    def fill_percent(self) -> Decimal:
        """Percentage of order filled."""
        if self.quantity == 0:
            return Decimal("0")
        return (self.filled_quantity / self.quantity) * Decimal("100")


# ============================================================================
# Entities
# ============================================================================

@dataclass
class MarginPosition:
    """
    Represents an Isolated Margin trading position.
    
    This entity tracks a leveraged position from open to close,
    including all risk parameters and P&L calculations.
    
    Key Principles:
    - Risk is LIMITED to margin allocated for this position only
    - Stop-loss is MANDATORY (enforced in constructor)
    - Position size calculated from 1% risk rule
    - Immutable updates via factory methods
    
    Lifecycle:
    1. PENDING: Entry order placed
    2. OPEN: Entry filled, stop-loss active
    3. CLOSING: Close order placed (manual or stop triggered)
    4. CLOSED/STOPPED_OUT/LIQUIDATED: Position closed
    """
    position_id: str
    client_id: int
    symbol: str
    side: MarginSide
    status: MarginPositionStatus
    
    # Position details
    entry_price: Decimal
    quantity: Decimal
    leverage: int
    
    # Risk parameters (USER provides these based on technical analysis)
    stop_price: Decimal
    target_price: Optional[Decimal] = None
    
    # Margin details
    margin_allocated: Decimal = Decimal("0")
    borrowed_amount: Decimal = Decimal("0")
    interest_accrued: Decimal = Decimal("0")
    
    # Calculated at entry
    position_value: Decimal = Decimal("0")
    risk_amount: Decimal = Decimal("0")
    risk_percent: Decimal = Decimal("0")
    
    # Current state (updated real-time)
    current_price: Decimal = Decimal("0")
    margin_level: Decimal = Decimal("999")  # High = safe
    
    # P&L (updated real-time)
    unrealized_pnl: Decimal = Decimal("0")
    realized_pnl: Decimal = Decimal("0")
    fees_paid: Decimal = Decimal("0")
    
    # Order IDs (internal)
    entry_order_id: Optional[str] = None
    stop_order_id: Optional[str] = None
    target_order_id: Optional[str] = None
    close_order_id: Optional[str] = None
    
    # Binance references (external)
    binance_entry_order_id: Optional[str] = None
    binance_stop_order_id: Optional[str] = None
    binance_target_order_id: Optional[str] = None
    binance_close_order_id: Optional[str] = None
    
    # Timestamps
    created_at: datetime = field(default_factory=datetime.utcnow)
    opened_at: Optional[datetime] = None
    closed_at: Optional[datetime] = None
    
    # Audit
    close_reason: Optional[str] = None
    correlation_id: Optional[str] = None
    
    def __post_init__(self):
        """Validate invariants."""
        if self.quantity <= 0:
            raise ValueError("Quantity must be positive")
        if self.entry_price <= 0:
            raise ValueError("Entry price must be positive")
        if self.stop_price <= 0:
            raise ValueError("Stop price must be positive")
        if self.leverage < 1:
            raise ValueError("Leverage must be at least 1")
        if self.leverage > 10:
            raise ValueError("Maximum leverage is 10x for safety")
        
        # Validate stop is on correct side of entry
        if self.side == MarginSide.LONG and self.stop_price >= self.entry_price:
            raise ValueError(
                f"LONG stop must be below entry price "
                f"(stop: {self.stop_price} >= entry: {self.entry_price})"
            )
        if self.side == MarginSide.SHORT and self.stop_price <= self.entry_price:
            raise ValueError(
                f"SHORT stop must be above entry price "
                f"(stop: {self.stop_price} <= entry: {self.entry_price})"
            )
        
        # Validate target is on correct side (if provided)
        if self.target_price is not None:
            if self.side == MarginSide.LONG and self.target_price <= self.entry_price:
                raise ValueError("LONG target must be above entry price")
            if self.side == MarginSide.SHORT and self.target_price >= self.entry_price:
                raise ValueError("SHORT target must be below entry price")
    
    # ========================================================================
    # Properties: Status checks
    # ========================================================================
    
    @property
    def is_pending(self) -> bool:
        """Check if position is pending entry fill."""
        return self.status == MarginPositionStatus.PENDING
    
    @property
    def is_open(self) -> bool:
        """Check if position is currently open."""
        return self.status == MarginPositionStatus.OPEN
    
    @property
    def is_closing(self) -> bool:
        """Check if position is being closed."""
        return self.status == MarginPositionStatus.CLOSING
    
    @property
    def is_closed(self) -> bool:
        """Check if position is closed (any reason)."""
        return self.status in {
            MarginPositionStatus.CLOSED,
            MarginPositionStatus.STOPPED_OUT,
            MarginPositionStatus.TAKE_PROFIT,
            MarginPositionStatus.LIQUIDATED,
        }
    
    @property
    def was_stopped(self) -> bool:
        """Check if position was closed by stop-loss."""
        return self.status == MarginPositionStatus.STOPPED_OUT
    
    @property
    def was_liquidated(self) -> bool:
        """Check if position was liquidated."""
        return self.status == MarginPositionStatus.LIQUIDATED
    
    # ========================================================================
    # Properties: Distance calculations
    # ========================================================================
    
    @property
    def stop_distance(self) -> Decimal:
        """Distance from entry to stop (always positive)."""
        return abs(self.entry_price - self.stop_price)
    
    @property
    def stop_distance_percent(self) -> Decimal:
        """Stop distance as percentage of entry price."""
        if self.entry_price == 0:
            return Decimal("0")
        return (self.stop_distance / self.entry_price) * Decimal("100")
    
    @property
    def target_distance(self) -> Optional[Decimal]:
        """Distance from entry to target (always positive)."""
        if self.target_price is None:
            return None
        return abs(self.target_price - self.entry_price)
    
    @property
    def risk_reward_ratio(self) -> Optional[Decimal]:
        """Risk/Reward ratio (target_distance / stop_distance)."""
        if self.target_distance is None or self.stop_distance == 0:
            return None
        return self.target_distance / self.stop_distance
    
    # ========================================================================
    # Properties: P&L calculations
    # ========================================================================
    
    @property
    def gross_pnl(self) -> Decimal:
        """Gross P&L before fees and interest."""
        return self.realized_pnl + self.unrealized_pnl
    
    @property
    def total_costs(self) -> Decimal:
        """Total costs (fees + interest)."""
        return self.fees_paid + self.interest_accrued
    
    @property
    def net_pnl(self) -> Decimal:
        """Net P&L after all costs."""
        return self.gross_pnl - self.total_costs
    
    @property
    def is_profitable(self) -> bool:
        """Check if position is currently profitable (net)."""
        return self.net_pnl > 0
    
    @property
    def pnl_percent(self) -> Decimal:
        """P&L as percentage of margin allocated."""
        if self.margin_allocated == 0:
            return Decimal("0")
        return (self.net_pnl / self.margin_allocated) * Decimal("100")
    
    # ========================================================================
    # Properties: Risk checks
    # ========================================================================
    
    @property
    def margin_health(self) -> MarginLevel:
        """Current margin health level."""
        if self.margin_level >= Decimal("2.0"):
            return MarginLevel.SAFE
        elif self.margin_level >= Decimal("1.5"):
            return MarginLevel.CAUTION
        elif self.margin_level >= Decimal("1.3"):
            return MarginLevel.WARNING
        elif self.margin_level >= Decimal("1.1"):
            return MarginLevel.CRITICAL
        else:
            return MarginLevel.DANGER
    
    @property
    def is_at_risk(self) -> bool:
        """Check if margin level is in warning zone."""
        return self.margin_level < Decimal("1.3")
    
    @property
    def is_critical(self) -> bool:
        """Check if margin level is critical (near liquidation)."""
        return self.margin_level < Decimal("1.1")
    
    # ========================================================================
    # Factory methods: State transitions
    # ========================================================================
    
    def update_price(self, current_price: Decimal) -> "MarginPosition":
        """
        Update current price and recalculate unrealized P&L.
        
        Returns new instance (immutable update pattern).
        """
        if current_price <= 0:
            raise ValueError("Current price must be positive")
        
        if self.side == MarginSide.LONG:
            # LONG: profit when price goes up
            unrealized = (current_price - self.entry_price) * self.quantity
        else:
            # SHORT: profit when price goes down
            unrealized = (self.entry_price - current_price) * self.quantity
        
        return _replace_position(
            self,
            current_price=current_price,
            unrealized_pnl=unrealized.quantize(Decimal("0.00000001")),
        )
    
    def update_margin_level(self, margin_level: Decimal) -> "MarginPosition":
        """Update margin level from exchange."""
        return _replace_position(self, margin_level=margin_level)
    
    def add_interest(self, interest: Decimal) -> "MarginPosition":
        """Add accrued interest."""
        return _replace_position(
            self,
            interest_accrued=self.interest_accrued + interest,
        )
    
    def add_fee(self, fee: Decimal) -> "MarginPosition":
        """Add trading fee."""
        return _replace_position(
            self,
            fees_paid=self.fees_paid + fee,
        )
    
    def mark_as_open(
        self,
        timestamp: datetime,
        binance_order_id: str,
        fill_price: Decimal,
        fill_quantity: Decimal,
        fee: Decimal = Decimal("0"),
    ) -> "MarginPosition":
        """
        Mark position as opened with fill details.
        
        Called when entry order is filled.
        """
        position_value = fill_price * fill_quantity
        
        return _replace_position(
            self,
            status=MarginPositionStatus.OPEN,
            opened_at=timestamp,
            binance_entry_order_id=binance_order_id,
            entry_price=fill_price,
            quantity=fill_quantity,
            position_value=position_value.quantize(Decimal("0.00000001")),
            current_price=fill_price,
            fees_paid=self.fees_paid + fee,
        )
    
    def mark_as_closing(self, close_order_id: str) -> "MarginPosition":
        """Mark position as closing (close order placed)."""
        return _replace_position(
            self,
            status=MarginPositionStatus.CLOSING,
            close_order_id=close_order_id,
        )
    
    def mark_as_stopped(
        self,
        timestamp: datetime,
        fill_price: Decimal,
        fee: Decimal = Decimal("0"),
    ) -> "MarginPosition":
        """
        Mark position as stopped out.
        
        Called when stop-loss order is triggered and filled.
        """
        if self.side == MarginSide.LONG:
            realized = (fill_price - self.entry_price) * self.quantity
        else:
            realized = (self.entry_price - fill_price) * self.quantity
        
        return _replace_position(
            self,
            status=MarginPositionStatus.STOPPED_OUT,
            closed_at=timestamp,
            realized_pnl=realized.quantize(Decimal("0.00000001")),
            unrealized_pnl=Decimal("0"),
            fees_paid=self.fees_paid + fee,
            close_reason="Stop-loss triggered",
        )
    
    def mark_as_take_profit(
        self,
        timestamp: datetime,
        fill_price: Decimal,
        fee: Decimal = Decimal("0"),
    ) -> "MarginPosition":
        """
        Mark position as closed by take-profit.
        
        Called when take-profit order is triggered and filled.
        """
        if self.side == MarginSide.LONG:
            realized = (fill_price - self.entry_price) * self.quantity
        else:
            realized = (self.entry_price - fill_price) * self.quantity
        
        return _replace_position(
            self,
            status=MarginPositionStatus.TAKE_PROFIT,
            closed_at=timestamp,
            realized_pnl=realized.quantize(Decimal("0.00000001")),
            unrealized_pnl=Decimal("0"),
            fees_paid=self.fees_paid + fee,
            close_reason="Take-profit triggered",
        )
    
    def mark_as_closed(
        self,
        timestamp: datetime,
        fill_price: Decimal,
        reason: str = "Manual close",
        fee: Decimal = Decimal("0"),
    ) -> "MarginPosition":
        """
        Mark position as manually closed.
        
        Called when user closes position or market closes it.
        """
        if self.side == MarginSide.LONG:
            realized = (fill_price - self.entry_price) * self.quantity
        else:
            realized = (self.entry_price - fill_price) * self.quantity
        
        return _replace_position(
            self,
            status=MarginPositionStatus.CLOSED,
            closed_at=timestamp,
            realized_pnl=realized.quantize(Decimal("0.00000001")),
            unrealized_pnl=Decimal("0"),
            fees_paid=self.fees_paid + fee,
            close_reason=reason,
        )
    
    def mark_as_liquidated(
        self,
        timestamp: datetime,
        fill_price: Decimal,
    ) -> "MarginPosition":
        """
        Mark position as liquidated by exchange.
        
        Called when margin level drops too low and exchange forces closure.
        """
        if self.side == MarginSide.LONG:
            realized = (fill_price - self.entry_price) * self.quantity
        else:
            realized = (self.entry_price - fill_price) * self.quantity
        
        return _replace_position(
            self,
            status=MarginPositionStatus.LIQUIDATED,
            closed_at=timestamp,
            realized_pnl=realized.quantize(Decimal("0.00000001")),
            unrealized_pnl=Decimal("0"),
            margin_level=Decimal("1"),
            close_reason="LIQUIDATED - Margin level too low",
        )


# ============================================================================
# Helper Functions
# ============================================================================

def _replace_position(position: MarginPosition, **changes) -> MarginPosition:
    """
    Create new MarginPosition with updated fields.
    
    Simulates dataclasses.replace() for mutable dataclass.
    """
    from dataclasses import asdict
    current = asdict(position)
    current.update(changes)
    return MarginPosition(**current)


# ============================================================================
# Position Sizing Calculator (Margin-aware)
# ============================================================================

@dataclass(frozen=True)
class MarginPositionSizingResult:
    """Result of margin position sizing calculation."""
    quantity: Decimal
    position_value: Decimal
    margin_required: Decimal
    risk_amount: Decimal
    risk_percent: Decimal
    stop_distance: Decimal
    stop_distance_percent: Decimal
    leverage: int
    is_capped: bool
    cap_reason: Optional[str] = None


def calculate_margin_position_size(
    capital: Decimal,
    entry_price: Decimal,
    stop_price: Decimal,
    side: str,
    leverage: int = 3,
    max_risk_percent: Decimal = Decimal("1.0"),
    max_margin_percent: Decimal = Decimal("50.0"),
    available_margin: Optional[Decimal] = None,
) -> MarginPositionSizingResult:
    """
    Calculate optimal position size for margin trading using 1% risk rule.
    
    This extends the spot position sizing to account for leverage.
    
    Formula:
        Risk Amount = Capital × (Risk % / 100)
        Stop Distance = |Entry Price - Stop Price|
        Base Quantity = Risk Amount / Stop Distance
        Position Value = Base Quantity × Entry Price
        Margin Required = Position Value / Leverage
    
    Args:
        capital: Total capital (Spot + Margin combined)
        entry_price: Intended entry price
        stop_price: Stop-loss price
        side: "LONG" or "SHORT"
        leverage: Leverage multiplier (1-10)
        max_risk_percent: Max risk per trade (default 1%)
        max_margin_percent: Max margin to allocate (default 50%)
        available_margin: Available margin in account (for validation)
        
    Returns:
        MarginPositionSizingResult with quantities and margin requirements
        
    Raises:
        ValueError: If inputs are invalid
    """
    # Input validation
    if capital <= 0:
        raise ValueError("Capital must be positive")
    if entry_price <= 0:
        raise ValueError("Entry price must be positive")
    if stop_price <= 0:
        raise ValueError("Stop price must be positive")
    if side not in ("LONG", "SHORT"):
        raise ValueError("Side must be LONG or SHORT")
    if leverage < 1 or leverage > 10:
        raise ValueError("Leverage must be between 1 and 10")
    if max_risk_percent <= 0 or max_risk_percent > 100:
        raise ValueError("Risk percent must be between 0 and 100")
    
    # Validate stop is on correct side
    if side == "LONG" and stop_price >= entry_price:
        raise ValueError("LONG stop must be below entry price")
    if side == "SHORT" and stop_price <= entry_price:
        raise ValueError("SHORT stop must be above entry price")
    
    # Step 1: Calculate risk amount (1% of capital)
    risk_amount = capital * (max_risk_percent / Decimal("100"))
    risk_amount = risk_amount.quantize(Decimal("0.01"))
    
    # Step 2: Calculate stop distance
    stop_distance = abs(entry_price - stop_price)
    stop_distance_percent = (stop_distance / entry_price) * Decimal("100")
    
    # Step 3: Calculate position size based on risk
    quantity = risk_amount / stop_distance
    quantity = quantity.quantize(Decimal("0.00000001"))
    
    # Step 4: Calculate position value and margin required
    position_value = quantity * entry_price
    margin_required = position_value / Decimal(leverage)
    
    # Step 5: Check margin cap
    max_margin = capital * (max_margin_percent / Decimal("100"))
    is_capped = False
    cap_reason = None
    
    if margin_required > max_margin:
        # Cap by max margin percent
        is_capped = True
        cap_reason = f"Capped by {max_margin_percent}% margin limit"
        
        margin_required = max_margin
        position_value = margin_required * Decimal(leverage)
        quantity = position_value / entry_price
        quantity = quantity.quantize(Decimal("0.00000001"))
        
        # Recalculate actual risk
        risk_amount = quantity * stop_distance
    
    if available_margin is not None and margin_required > available_margin:
        # Cap by available margin
        is_capped = True
        cap_reason = f"Capped by available margin ({available_margin})"
        
        margin_required = available_margin
        position_value = margin_required * Decimal(leverage)
        quantity = position_value / entry_price
        quantity = quantity.quantize(Decimal("0.00000001"))
        
        # Recalculate actual risk
        risk_amount = quantity * stop_distance
    
    # Step 6: Calculate actual risk percent
    risk_percent = (risk_amount / capital) * Decimal("100")
    
    return MarginPositionSizingResult(
        quantity=quantity,
        position_value=position_value.quantize(Decimal("0.01")),
        margin_required=margin_required.quantize(Decimal("0.01")),
        risk_amount=risk_amount.quantize(Decimal("0.01")),
        risk_percent=risk_percent.quantize(Decimal("0.01")),
        stop_distance=stop_distance,
        stop_distance_percent=stop_distance_percent.quantize(Decimal("0.01")),
        leverage=leverage,
        is_capped=is_capped,
        cap_reason=cap_reason,
    )

