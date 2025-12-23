"""
Use Cases - Business Logic Orchestration

Use cases orchestrate domain logic and coordinate between ports.
They represent the application's operations (what the system can do).

Key principles:
- Each use case is a single business operation
- Use cases depend on ports (interfaces), not adapters (implementations)
- Use cases are framework-agnostic (no Django, no HTTP, no database)
- Use cases publish domain events for auditability
"""

from decimal import Decimal
from datetime import datetime
from typing import Optional
import uuid

from apps.backend.core.domain.trading import (
    TradingIntent,
    IntentStatus,
    PolicyState,
    PolicyStatus,
)
from apps.backend.core.application.ports import (
    TradingSignal,
    MessageBusPort,
    RiskPolicyPort,
    OrderExecutionPort,
    TradingIntentRepository,
    PolicyStateRepository,
    ClockPort,
    AuditTrailPort,
    IntentCreatedEvent,
    OrderPlacedEvent,
    PolicyPausedEvent,
    RiskCheckResult,
)


# ============================================================================
# Generate Intent Use Case
# ============================================================================

class GenerateIntentUseCase:
    """
    Create a trading intent from a trading signal.

    This use case:
    1. Receives a trading signal (from decision engine)
    2. Calculates position size based on 1% risk rule
    3. Creates TradingIntent entity
    4. Publishes IntentCreated event
    5. Saves intent to repository

    Does NOT:
    - Validate risk limits (that's ValidateIntentUseCase)
    - Execute orders (that's ExecuteIntentUseCase)
    """

    def __init__(
        self,
        intent_repo: TradingIntentRepository,
        message_bus: MessageBusPort,
        clock: ClockPort,
        audit_trail: AuditTrailPort,
    ):
        self._intent_repo = intent_repo
        self._message_bus = message_bus
        self._clock = clock
        self._audit_trail = audit_trail

    def execute(
        self,
        signal: TradingSignal,
        client_id: int,
        capital: Decimal,
        max_risk_percent: Decimal = Decimal("1.0"),
    ) -> TradingIntent:
        """
        Generate trading intent from signal.

        Args:
            signal: Trading signal from decision engine
            client_id: Client ID
            capital: Total capital available
            max_risk_percent: Maximum risk per trade (default 1%)

        Returns:
            Created TradingIntent

        Raises:
            ValueError: If signal is invalid or calculation fails
        """
        # Validate inputs
        if capital <= 0:
            raise ValueError("Capital must be positive")
        if max_risk_percent <= 0 or max_risk_percent > 100:
            raise ValueError("Risk percent must be between 0 and 100")

        # Calculate position size using 1% risk rule
        # Risk per trade = capital * max_risk_percent / 100
        # Position size = risk / (entry_price - stop_price)
        risk_amount = capital * (max_risk_percent / Decimal("100"))
        stop_distance = abs(signal.entry_price - signal.stop_price)

        if stop_distance == 0:
            raise ValueError("Stop distance cannot be zero")

        quantity = risk_amount / stop_distance

        # Quantize to 8 decimal places (Binance precision)
        quantity = quantity.quantize(Decimal("0.00000001"))
        risk_amount = risk_amount.quantize(Decimal("0.00000001"))

        # Create intent
        intent_id = f"intent-{uuid.uuid4()}"
        correlation_id = f"corr-{uuid.uuid4()}"
        now = self._clock.now()

        intent = TradingIntent(
            intent_id=intent_id,
            client_id=client_id,
            symbol=signal.symbol,
            side=signal.side,
            status=IntentStatus.PENDING,
            quantity=quantity,
            entry_price=signal.entry_price,
            stop_price=signal.stop_price,
            target_price=signal.target_price,
            strategy_name=signal.strategy_name,
            regime=signal.regime.regime,
            confidence=signal.confidence,
            reason=signal.reason,
            created_at=now,
            risk_amount=risk_amount,
            risk_percent=max_risk_percent,
            correlation_id=correlation_id,
        )

        # Save to repository
        saved_intent = self._intent_repo.save(intent)

        # Publish event
        event = IntentCreatedEvent(
            event_id=f"evt-{uuid.uuid4()}",
            event_type="IntentCreated",
            timestamp=now,
            aggregate_id=intent_id,
            intent_id=intent_id,
            client_id=client_id,
            symbol=signal.symbol,
            side=signal.side,
            strategy_name=signal.strategy_name,
            reason=signal.reason,
            correlation_id=correlation_id,
        )
        self._message_bus.publish(event)

        # Audit trail
        self._audit_trail.record(
            event_type="intent_created",
            aggregate_id=intent_id,
            data={
                "client_id": client_id,
                "symbol": signal.symbol,
                "side": signal.side,
                "quantity": str(quantity),
                "entry_price": str(signal.entry_price),
                "stop_price": str(signal.stop_price),
                "risk_amount": str(risk_amount),
                "confidence": signal.confidence,
            },
            reason=f"Generated from signal: {signal.reason}",
        )

        return saved_intent


# ============================================================================
# Validate Intent Use Case
# ============================================================================

class ValidateIntentUseCase:
    """
    Validate a trading intent against risk policies.

    This use case:
    1. Retrieves trading intent
    2. Checks per-trade risk limit
    3. Checks monthly drawdown limit
    4. Checks position concentration (future)
    5. Updates intent status to VALIDATED or FAILED
    6. Publishes events

    Does NOT:
    - Execute orders (that's ExecuteIntentUseCase)
    """

    def __init__(
        self,
        intent_repo: TradingIntentRepository,
        policy_state_repo: PolicyStateRepository,
        risk_policy: RiskPolicyPort,
        message_bus: MessageBusPort,
        clock: ClockPort,
        audit_trail: AuditTrailPort,
    ):
        self._intent_repo = intent_repo
        self._policy_state_repo = policy_state_repo
        self._risk_policy = risk_policy
        self._message_bus = message_bus
        self._clock = clock
        self._audit_trail = audit_trail

    def execute(self, intent_id: str) -> tuple[TradingIntent, RiskCheckResult]:
        """
        Validate trading intent.

        Args:
            intent_id: Intent to validate

        Returns:
            Tuple of (updated_intent, risk_check_result)

        Raises:
            ValueError: If intent not found or already validated
        """
        # Retrieve intent
        intent = self._intent_repo.find_by_id(intent_id)
        if not intent:
            raise ValueError(f"Intent {intent_id} not found")

        if intent.status != IntentStatus.PENDING:
            raise ValueError(f"Intent {intent_id} is not pending (status: {intent.status})")

        now = self._clock.now()

        # Check per-trade risk
        trade_risk_result = self._risk_policy.check_trade_risk(
            client_id=intent.client_id,
            symbol=intent.symbol,
            side=intent.side,
            quantity=intent.quantity,
            entry_price=intent.entry_price,
            stop_price=intent.stop_price,
        )

        if not trade_risk_result.passed:
            # Mark as failed
            updated_intent = intent.mark_as_failed(trade_risk_result.reason)
            self._intent_repo.save(updated_intent)

            self._audit_trail.record(
                event_type="intent_validation_failed",
                aggregate_id=intent_id,
                data=trade_risk_result.details,
                reason=trade_risk_result.reason,
            )

            return updated_intent, trade_risk_result

        # Check monthly drawdown
        drawdown_result = self._risk_policy.check_monthly_drawdown(intent.client_id)

        if not drawdown_result.passed:
            # Pause trading
            self._risk_policy.pause_trading(intent.client_id, drawdown_result.reason)

            # Mark intent as failed
            updated_intent = intent.mark_as_failed(drawdown_result.reason)
            self._intent_repo.save(updated_intent)

            # Publish pause event
            pause_event = PolicyPausedEvent(
                event_id=f"evt-{uuid.uuid4()}",
                event_type="PolicyPaused",
                timestamp=now,
                aggregate_id=f"policy-{intent.client_id}",
                client_id=intent.client_id,
                reason=drawdown_result.reason,
                drawdown_percent=drawdown_result.details.get("drawdown_percent", Decimal("0")),
                limit_percent=drawdown_result.details.get("limit_percent", Decimal("4")),
            )
            self._message_bus.publish(pause_event)

            self._audit_trail.record(
                event_type="trading_paused",
                aggregate_id=f"policy-{intent.client_id}",
                data=drawdown_result.details,
                reason=drawdown_result.reason,
            )

            return updated_intent, drawdown_result

        # All checks passed - mark as validated
        updated_intent = intent.mark_as_validated(now)
        self._intent_repo.save(updated_intent)

        self._audit_trail.record(
            event_type="intent_validated",
            aggregate_id=intent_id,
            data={
                "trade_risk": trade_risk_result.details,
                "drawdown": drawdown_result.details,
            },
            reason="All risk checks passed",
        )

        return updated_intent, RiskCheckResult(
            passed=True,
            reason="All risk checks passed",
            details={
                "trade_risk": trade_risk_result.details,
                "drawdown": drawdown_result.details,
            },
        )


# ============================================================================
# Execute Intent Use Case
# ============================================================================

class ExecuteIntentUseCase:
    """
    Execute a validated trading intent.

    This use case:
    1. Retrieves validated intent
    2. Places order on exchange
    3. Updates intent with execution results
    4. Publishes OrderPlaced event
    5. Records audit trail

    Does NOT:
    - Validate risk (must be pre-validated)
    - Generate intents (that's GenerateIntentUseCase)
    """

    def __init__(
        self,
        intent_repo: TradingIntentRepository,
        order_execution: OrderExecutionPort,
        message_bus: MessageBusPort,
        clock: ClockPort,
        audit_trail: AuditTrailPort,
    ):
        self._intent_repo = intent_repo
        self._order_execution = order_execution
        self._message_bus = message_bus
        self._clock = clock
        self._audit_trail = audit_trail

    def execute(
        self,
        intent_id: str,
        dry_run: bool = True,
    ) -> tuple[TradingIntent, Optional[str]]:
        """
        Execute trading intent.

        Args:
            intent_id: Intent to execute
            dry_run: If True, simulate execution (no real order)

        Returns:
            Tuple of (updated_intent, order_id or None)

        Raises:
            ValueError: If intent not found or not validated
        """
        # Retrieve intent
        intent = self._intent_repo.find_by_id(intent_id)
        if not intent:
            raise ValueError(f"Intent {intent_id} not found")

        if intent.status != IntentStatus.VALIDATED:
            raise ValueError(
                f"Intent {intent_id} is not validated (status: {intent.status})"
            )

        now = self._clock.now()

        # Mark as executing
        intent = intent.mark_as_executing()
        self._intent_repo.save(intent)

        if dry_run:
            # Simulate execution
            order_id = f"order-{uuid.uuid4()}-DRY"
            exchange_order_id = f"binance-{uuid.uuid4()}-DRY"
            fill_price = intent.entry_price
            fill_quantity = intent.quantity

            self._audit_trail.record(
                event_type="intent_executed_dry_run",
                aggregate_id=intent_id,
                data={
                    "order_id": order_id,
                    "exchange_order_id": exchange_order_id,
                    "fill_price": str(fill_price),
                    "fill_quantity": str(fill_quantity),
                },
                reason="DRY RUN - No real order placed",
            )
        else:
            # Execute on exchange
            result = self._order_execution.place_order(
                symbol=intent.symbol,
                side=intent.side,
                quantity=intent.quantity,
                order_type="MARKET",
            )

            if not result.success:
                # Execution failed
                updated_intent = intent.mark_as_failed(
                    result.error_message or "Order execution failed"
                )
                self._intent_repo.save(updated_intent)

                self._audit_trail.record(
                    event_type="intent_execution_failed",
                    aggregate_id=intent_id,
                    data={"error": result.error_message},
                    reason=result.error_message or "Unknown error",
                )

                return updated_intent, None

            order_id = result.order_id
            exchange_order_id = result.exchange_order_id
            fill_price = result.avg_fill_price or intent.entry_price
            fill_quantity = result.filled_quantity

        # Mark as executed
        updated_intent = intent.mark_as_executed(
            timestamp=now,
            order_id=order_id,
            exchange_order_id=exchange_order_id,
            fill_price=fill_price,
            fill_quantity=fill_quantity,
        )
        self._intent_repo.save(updated_intent)

        # Publish event
        event = OrderPlacedEvent(
            event_id=f"evt-{uuid.uuid4()}",
            event_type="OrderPlaced",
            timestamp=now,
            aggregate_id=order_id,
            order_id=order_id,
            intent_id=intent_id,
            client_id=intent.client_id,
            symbol=intent.symbol,
            side=intent.side,
            quantity=fill_quantity,
            price=fill_price,
            exchange_order_id=exchange_order_id,
            correlation_id=intent.correlation_id,
        )
        self._message_bus.publish(event)

        # Audit trail
        self._audit_trail.record(
            event_type="intent_executed",
            aggregate_id=intent_id,
            data={
                "order_id": order_id,
                "exchange_order_id": exchange_order_id,
                "fill_price": str(fill_price),
                "fill_quantity": str(fill_quantity),
                "dry_run": dry_run,
            },
            reason=f"Intent executed successfully ({'DRY RUN' if dry_run else 'LIVE'})",
        )

        return updated_intent, order_id
