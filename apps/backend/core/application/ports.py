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
from dataclasses import dataclass


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
    metadata: dict[str, Any] = None

    def __post_init__(self):
        if self.metadata is None:
            object.__setattr__(self, 'metadata', {})


@dataclass(frozen=True)
class IntentCreatedEvent(DomainEvent):
    """Event: Trading intent was created by decision engine."""
    intent_id: str
    client_id: int
    symbol: str
    side: str
    strategy_name: str
    reason: str  # Why this intent was created


@dataclass(frozen=True)
class OrderPlacedEvent(DomainEvent):
    """Event: Order was placed on exchange."""
    order_id: str
    intent_id: Optional[str]
    client_id: int
    symbol: str
    side: str
    quantity: Decimal
    price: Optional[Decimal]
    exchange_order_id: str


@dataclass(frozen=True)
class OrderFilledEvent(DomainEvent):
    """Event: Order was filled on exchange."""
    order_id: str
    exchange_order_id: str
    filled_quantity: Decimal
    avg_fill_price: Decimal


@dataclass(frozen=True)
class PolicyPausedEvent(DomainEvent):
    """Event: Risk policy paused trading (e.g., drawdown limit hit)."""
    client_id: int
    reason: str
    drawdown_percent: Decimal
    limit_percent: Decimal


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
