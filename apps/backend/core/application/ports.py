"""
Port Definitions (Interfaces)

Ports are contracts that define how the application core interacts with external systems.
They are implemented by adapters in the adapters/ directory.

Following the Dependency Inversion Principle:
- Application core defines WHAT it needs (ports)
- Adapters implement HOW to provide it (concrete implementations)
- Core NEVER imports from adapters (dependency points inward)

All ports use Protocol (PEP 544) for structural subtyping.
"""

from typing import Protocol, Any, Callable, Optional
from decimal import Decimal
from datetime import datetime
from dataclasses import dataclass, field


# ============================================================================
# Event Bus Ports (Message-Driven Architecture)
# ============================================================================

@dataclass(frozen=True)
class DomainEvent:
    """
    Base class for all domain events.

    Domain events represent facts that have happened in the system.
    They are immutable and carry all necessary context.
    """
    event_id: str
    event_type: str
    timestamp: datetime
    aggregate_id: str  # ID of the entity that generated this event
    correlation_id: Optional[str] = None  # For tracing related events
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class IntentCreatedEvent(DomainEvent):
    """Event: Trading intent was created by decision engine."""
    intent_id: str = ""
    client_id: int = 0
    symbol: str = ""
    side: str = ""
    strategy_name: str = ""
    reason: str = ""  # Why this intent was created


@dataclass(frozen=True)
class OrderPlacedEvent(DomainEvent):
    """Event: Order was placed on exchange."""
    order_id: str = ""
    intent_id: Optional[str] = None
    client_id: int = 0
    symbol: str = ""
    side: str = ""
    quantity: Decimal = Decimal("0")
    price: Optional[Decimal] = None
    exchange_order_id: str = ""


@dataclass(frozen=True)
class OrderFilledEvent(DomainEvent):
    """Event: Order was filled on exchange."""
    order_id: str = ""
    exchange_order_id: str = ""
    filled_quantity: Decimal = Decimal("0")
    avg_fill_price: Decimal = Decimal("0")


@dataclass(frozen=True)
class PolicyPausedEvent(DomainEvent):
    """Event: Risk policy paused trading (e.g., drawdown limit hit)."""
    client_id: int = 0
    reason: str = ""
    drawdown_percent: Decimal = Decimal("0")
    limit_percent: Decimal = Decimal("0")


class MessageBusPort(Protocol):
    """
    Port for event-driven communication.

    Implementations:
    - InMemoryMessageBus: For development and testing (synchronous)
    - RabbitMQMessageBus: For production (asynchronous, durable)

    Design:
    - Fire-and-forget (no return value)
    - At-most-once delivery semantic (trading context)
    - Idempotent message handling (consumers must handle duplicates)
    """

    def publish(
        self,
        event: DomainEvent,
        routing_key: Optional[str] = None,
    ) -> None:
        """
        Publish a domain event to the message bus.

        Args:
            event: Domain event to publish
            routing_key: Optional routing key (for topic exchanges)
                        Default format: "{event_type}.{client_id}"

        Raises:
            MessageBusError: If publication fails
        """
        ...

    def subscribe(
        self,
        event_type: str,
        handler: Callable[[DomainEvent], None],
        routing_pattern: Optional[str] = None,
    ) -> None:
        """
        Subscribe to events of a specific type.

        Args:
            event_type: Type of event to subscribe to
            handler: Callback function to handle events
            routing_pattern: Optional pattern for filtering (e.g., "trading.*.1")

        Note:
            Handlers must be idempotent (may receive duplicates).
        """
        ...


# ============================================================================
# Risk & Policy Ports
# ============================================================================

@dataclass(frozen=True)
class RiskCheckResult:
    """Result of a risk check."""
    passed: bool
    reason: str
    details: dict[str, Any]


class RiskPolicyPort(Protocol):
    """
    Port for risk policy checks.

    Implements deterministic risk guards:
    - Per-trade risk limit (1% default)
    - Monthly drawdown limit (4% default)
    - Position concentration limits
    - Correlation limits (future)
    """

    def check_trade_risk(
        self,
        client_id: int,
        symbol: str,
        side: str,
        quantity: Decimal,
        entry_price: Decimal,
        stop_price: Decimal,
    ) -> RiskCheckResult:
        """
        Check if proposed trade passes risk limits.

        Validates:
        - Position size vs capital (max 1% risk)
        - Total exposure limit
        - Position concentration

        Returns:
            RiskCheckResult with pass/fail and reason
        """
        ...

    def check_monthly_drawdown(self, client_id: int) -> RiskCheckResult:
        """
        Check if client is within monthly drawdown limit.

        Returns:
            RiskCheckResult with pass/fail and current drawdown
        """
        ...

    def pause_trading(self, client_id: int, reason: str) -> None:
        """
        Pause all trading for a client (emergency stop).

        Args:
            client_id: Client to pause
            reason: Why trading was paused
        """
        ...


# ============================================================================
# Decision Engine Ports (Systematic Trading)
# ============================================================================

@dataclass(frozen=True)
class MarketRegime:
    """Current market regime classification."""
    regime: str  # "bull", "bear", "sideways"
    confidence: float  # 0.0 to 1.0
    indicators: dict[str, Any]


@dataclass(frozen=True)
class TradingSignal:
    """Trading signal generated by strategy."""
    signal_id: str
    timestamp: datetime
    symbol: str
    side: str  # "BUY" or "SELL"
    confidence: float  # 0.0 to 1.0
    entry_price: Decimal
    stop_price: Decimal
    target_price: Decimal
    strategy_name: str
    reason: str
    regime: MarketRegime


class DecisionEnginePort(Protocol):
    """
    Port for systematic trading decision engine.

    Responsibilities:
    - Detect market regime (bull/bear/sideways)
    - Generate trading signals based on strategies
    - Calculate technical stops and position sizes

    NOT responsible for:
    - Risk validation (handled by RiskPolicyPort)
    - Order execution (handled by OrderExecutionPort)
    """

    def detect_regime(self, symbol: str, timeframe: str = "15m") -> MarketRegime:
        """
        Detect current market regime for symbol.

        Args:
            symbol: Trading pair (e.g., "BTCUSDC")
            timeframe: Candle timeframe (default: 15m)

        Returns:
            MarketRegime with classification and confidence
        """
        ...

    def generate_signals(
        self,
        client_id: int,
        symbols: list[str],
        regime: Optional[MarketRegime] = None,
    ) -> list[TradingSignal]:
        """
        Generate trading signals for given symbols.

        Args:
            client_id: Client to generate signals for
            symbols: List of symbols to analyze
            regime: Optional pre-detected regime (to avoid re-detection)

        Returns:
            List of trading signals (may be empty if no opportunities)
        """
        ...


# ============================================================================
# Order Execution Ports
# ============================================================================

@dataclass(frozen=True)
class OrderExecutionResult:
    """Result of order execution attempt."""
    success: bool
    order_id: Optional[str]
    exchange_order_id: Optional[str]
    filled_quantity: Decimal
    avg_fill_price: Optional[Decimal]
    error_message: Optional[str] = None


class OrderExecutionPort(Protocol):
    """
    Port for executing orders on exchange.

    Implementations:
    - BinanceOrderExecutor: Live execution on Binance
    - MockOrderExecutor: Paper trading / testing
    """

    def place_order(
        self,
        symbol: str,
        side: str,
        quantity: Decimal,
        order_type: str = "MARKET",
        price: Optional[Decimal] = None,
    ) -> OrderExecutionResult:
        """
        Place order on exchange.

        Args:
            symbol: Trading pair
            side: "BUY" or "SELL"
            quantity: Order quantity
            order_type: "MARKET" or "LIMIT"
            price: Limit price (required for LIMIT orders)

        Returns:
            OrderExecutionResult with order details or error
        """
        ...

    def cancel_order(self, symbol: str, exchange_order_id: str) -> bool:
        """
        Cancel open order.

        Returns:
            True if canceled, False if failed
        """
        ...


# ============================================================================
# Repository Ports (Data Persistence)
# ============================================================================

class TradingIntentRepository(Protocol):
    """Repository for trading intents (systematic trading decisions)."""

    def save(self, intent: 'TradingIntent') -> 'TradingIntent':
        """Save trading intent."""
        ...

    def find_by_id(self, intent_id: str) -> Optional['TradingIntent']:
        """Find intent by ID."""
        ...

    def find_pending(self, client_id: int) -> list['TradingIntent']:
        """Find all pending (not yet executed) intents for client."""
        ...


class PolicyStateRepository(Protocol):
    """Repository for policy state (risk limits, pauses)."""

    def get_state(self, client_id: int, month: str) -> Optional['PolicyState']:
        """
        Get policy state for client and month.

        Args:
            client_id: Client ID
            month: Month in format "YYYY-MM"

        Returns:
            PolicyState or None if not found
        """
        ...

    def save_state(self, state: 'PolicyState') -> 'PolicyState':
        """Save or update policy state."""
        ...


# ============================================================================
# Time / Clock Port (for testing)
# ============================================================================

class ClockPort(Protocol):
    """
    Port for time operations (enables time travel in tests).

    Implementations:
    - SystemClock: Uses datetime.now()
    - FakeClock: Controllable time for testing
    """

    def now(self) -> datetime:
        """Get current time."""
        ...


# ============================================================================
# Audit Trail Port
# ============================================================================

class AuditTrailPort(Protocol):
    """
    Port for recording audit trail (execution events).

    Every state transition must be auditable:
    - Who triggered it (system or user)
    - When it happened
    - What changed
    - Why (reasoning)
    """

    def record(
        self,
        event_type: str,
        aggregate_id: str,
        data: dict[str, Any],
        reason: str,
    ) -> None:
        """
        Record audit event.

        Args:
            event_type: Type of event (e.g., "intent_created", "order_placed")
            aggregate_id: ID of entity affected
            data: Event data
            reason: Human-readable reason
        """
        ...

    def get_history(self, aggregate_id: str) -> list[dict[str, Any]]:
        """Get audit history for entity."""
        ...


# ============================================================================
# Margin Trading Ports
# ============================================================================

# Import margin domain types for type hints
# Note: These are imported at runtime to avoid circular imports
# In actual use, they come from apps.backend.core.domain.margin


@dataclass(frozen=True)
class MarginTransferResult:
    """Result of a margin transfer operation."""
    success: bool
    transaction_id: Optional[str]
    asset: str
    amount: Decimal
    from_account: str
    to_account: str
    error_message: Optional[str] = None


@dataclass(frozen=True)
class MarginAccountSnapshot:
    """Snapshot of Isolated Margin account for a symbol."""
    symbol: str
    base_asset: str
    base_free: Decimal
    base_locked: Decimal
    base_borrowed: Decimal
    quote_asset: str
    quote_free: Decimal
    quote_locked: Decimal
    quote_borrowed: Decimal
    margin_level: Decimal
    liquidation_price: Decimal
    is_margin_trade_enabled: bool


@dataclass(frozen=True)
class MarginOrderExecutionResult:
    """Result of margin order execution."""
    success: bool
    order_id: Optional[str]
    binance_order_id: Optional[str]
    symbol: str
    side: str
    order_type: str
    quantity: Decimal
    price: Optional[Decimal]
    filled_quantity: Decimal
    avg_fill_price: Optional[Decimal]
    status: str
    error_message: Optional[str] = None


class MarginExecutionPort(Protocol):
    """
    Port for Isolated Margin trading operations.
    
    Implementations:
    - BinanceMarginAdapter: Real execution on Binance
    - MockMarginAdapter: Paper trading / testing
    
    Key Principle: Isolated Margin means risk is LIMITED to the margin
    allocated for each specific position. No cross-contamination.
    """
    
    def transfer_to_margin(
        self,
        symbol: str,
        asset: str,
        amount: Decimal,
    ) -> MarginTransferResult:
        """
        Transfer asset from Spot wallet to Isolated Margin account.
        
        Args:
            symbol: Trading pair (e.g., "BTCUSDC")
            asset: Asset to transfer (e.g., "USDC")
            amount: Amount to transfer
            
        Returns:
            MarginTransferResult with success/failure and transaction ID
        """
        ...
    
    def transfer_from_margin(
        self,
        symbol: str,
        asset: str,
        amount: Decimal,
    ) -> MarginTransferResult:
        """
        Transfer asset from Isolated Margin account back to Spot wallet.
        
        Args:
            symbol: Trading pair (e.g., "BTCUSDC")
            asset: Asset to transfer (e.g., "USDC")
            amount: Amount to transfer
            
        Returns:
            MarginTransferResult with success/failure
            
        Note:
            Will fail if transfer would cause margin call.
        """
        ...
    
    def get_margin_account(self, symbol: str) -> MarginAccountSnapshot:
        """
        Get Isolated Margin account info for a symbol.
        
        Args:
            symbol: Trading pair (e.g., "BTCUSDC")
            
        Returns:
            MarginAccountSnapshot with balances and margin level
            
        Raises:
            ValueError: If symbol not found in margin account
        """
        ...
    
    def place_margin_order(
        self,
        symbol: str,
        side: str,
        order_type: str,
        quantity: Decimal,
        price: Optional[Decimal] = None,
        stop_price: Optional[Decimal] = None,
        side_effect_type: Optional[str] = None,
    ) -> MarginOrderExecutionResult:
        """
        Place an order on Isolated Margin account.
        
        Args:
            symbol: Trading pair (e.g., "BTCUSDC")
            side: "BUY" or "SELL"
            order_type: "MARKET", "LIMIT", "STOP_LOSS_LIMIT", "TAKE_PROFIT_LIMIT"
            quantity: Order quantity in base asset
            price: Limit price (required for LIMIT and STOP_LOSS_LIMIT)
            stop_price: Trigger price (required for STOP_LOSS_LIMIT)
            side_effect_type: "MARGIN_BUY" (auto-borrow) or "AUTO_REPAY" (auto-repay)
            
        Returns:
            MarginOrderExecutionResult with order details or error
        """
        ...
    
    def cancel_margin_order(
        self,
        symbol: str,
        order_id: str,
    ) -> bool:
        """
        Cancel an open Isolated Margin order.
        
        Args:
            symbol: Trading pair
            order_id: Binance order ID to cancel
            
        Returns:
            True if cancelled successfully, False otherwise
        """
        ...
    
    def get_margin_level(self, symbol: str) -> Decimal:
        """
        Get current margin level for symbol.
        
        Margin Level = Total Asset Value / (Total Borrowed + Total Interest)
        
        Returns:
            Margin level as Decimal:
            - >= 2.0: SAFE (can open new positions)
            - >= 1.5: CAUTION
            - >= 1.3: WARNING
            - >= 1.1: CRITICAL
            - < 1.1: DANGER (approaching liquidation)
        """
        ...
    
    def get_open_margin_orders(self, symbol: str) -> list[dict]:
        """
        Get all open margin orders for a symbol.
        
        Returns:
            List of open orders with details
        """
        ...


class MarginPositionRepository(Protocol):
    """Repository for margin positions."""
    
    def save(self, position: Any) -> Any:
        """Save or update a margin position."""
        ...
    
    def find_by_id(self, position_id: str) -> Optional[Any]:
        """Find position by ID."""
        ...
    
    def find_open_by_client(self, client_id: int) -> list[Any]:
        """Find all open positions for a client."""
        ...
    
    def find_by_symbol(self, client_id: int, symbol: str) -> list[Any]:
        """Find all positions for a symbol (open and closed)."""
        ...
    
    def find_open_by_symbol(self, client_id: int, symbol: str) -> Optional[Any]:
        """Find open position for a symbol (only one allowed per symbol)."""
        ...