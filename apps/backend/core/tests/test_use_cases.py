"""
Tests for Use Cases

Run with: pytest apps/backend/core/tests/test_use_cases.py -v
"""

import pytest
from decimal import Decimal
from datetime import datetime
from typing import Optional, List
import uuid

from apps.backend.core.domain.trading import (
    TradingIntent,
    IntentStatus,
    PolicyState,
    PolicyStatus,
)
from apps.backend.core.application.ports import (
    TradingSignal,
    MarketRegime,
    RiskCheckResult,
    OrderExecutionResult,
    DomainEvent,
)
from apps.backend.core.application.use_cases import (
    GenerateIntentUseCase,
    ValidateIntentUseCase,
    ExecuteIntentUseCase,
)


# ============================================================================
# Test Doubles (Mocks/Stubs)
# ============================================================================

class InMemoryIntentRepository:
    """In-memory intent repository for testing."""

    def __init__(self):
        self._intents = {}

    def save(self, intent: TradingIntent) -> TradingIntent:
        self._intents[intent.intent_id] = intent
        return intent

    def find_by_id(self, intent_id: str) -> Optional[TradingIntent]:
        return self._intents.get(intent_id)

    def find_pending(self, client_id: int) -> List[TradingIntent]:
        return [i for i in self._intents.values() if i.client_id == client_id and i.is_pending]


class InMemoryPolicyStateRepository:
    """In-memory policy state repository for testing."""

    def __init__(self):
        self._states = {}

    def get_state(self, client_id: int, month: str) -> Optional[PolicyState]:
        key = f"{client_id}:{month}"
        return self._states.get(key)

    def save_state(self, state: PolicyState) -> PolicyState:
        key = f"{state.client_id}:{state.month}"
        self._states[key] = state
        return state


class FakeRiskPolicy:
    """Fake risk policy for testing."""

    def __init__(self, trade_check_passes: bool = True, drawdown_check_passes: bool = True):
        self.trade_check_passes = trade_check_passes
        self.drawdown_check_passes = drawdown_check_passes
        self.paused_clients = set()

    def check_trade_risk(self, **kwargs) -> RiskCheckResult:
        if self.trade_check_passes:
            return RiskCheckResult(
                passed=True,
                reason="Trade risk check passed",
                details={"risk_percent": "1.0"},
            )
        return RiskCheckResult(
            passed=False,
            reason="Trade risk exceeds limit",
            details={"risk_percent": "2.0", "limit": "1.0"},
        )

    def check_monthly_drawdown(self, client_id: int) -> RiskCheckResult:
        if self.drawdown_check_passes:
            return RiskCheckResult(
                passed=True,
                reason="Drawdown within limits",
                details={"drawdown_percent": Decimal("1.5"), "limit_percent": Decimal("4.0")},
            )
        return RiskCheckResult(
            passed=False,
            reason="Monthly drawdown limit exceeded",
            details={"drawdown_percent": Decimal("5.0"), "limit_percent": Decimal("4.0")},
        )

    def pause_trading(self, client_id: int, reason: str) -> None:
        self.paused_clients.add(client_id)


class FakeOrderExecution:
    """Fake order execution for testing."""

    def __init__(self, should_succeed: bool = True):
        self.should_succeed = should_succeed
        self.placed_orders = []

    def place_order(self, **kwargs) -> OrderExecutionResult:
        self.placed_orders.append(kwargs)

        if self.should_succeed:
            return OrderExecutionResult(
                success=True,
                order_id=f"order-{uuid.uuid4()}",
                exchange_order_id=f"binance-{uuid.uuid4()}",
                filled_quantity=kwargs['quantity'],
                avg_fill_price=Decimal("90000.00"),
            )
        return OrderExecutionResult(
            success=False,
            order_id=None,
            exchange_order_id=None,
            filled_quantity=Decimal("0"),
            avg_fill_price=None,
            error_message="Exchange API error",
        )

    def cancel_order(self, **kwargs) -> bool:
        return True


class FakeMessageBus:
    """Fake message bus for testing."""

    def __init__(self):
        self.published_events = []

    def publish(self, event: DomainEvent, routing_key: Optional[str] = None) -> None:
        self.published_events.append(event)

    def subscribe(self, event_type: str, handler, routing_pattern: Optional[str] = None) -> None:
        pass


class FakeClock:
    """Fake clock for testing."""

    def __init__(self, fixed_time: Optional[datetime] = None):
        self.fixed_time = fixed_time or datetime(2025, 12, 23, 12, 0, 0)

    def now(self) -> datetime:
        return self.fixed_time


class FakeAuditTrail:
    """Fake audit trail for testing."""

    def __init__(self):
        self.records = []

    def record(self, event_type: str, aggregate_id: str, data: dict, reason: str) -> None:
        self.records.append({
            'event_type': event_type,
            'aggregate_id': aggregate_id,
            'data': data,
            'reason': reason,
        })

    def get_history(self, aggregate_id: str) -> list:
        return [r for r in self.records if r['aggregate_id'] == aggregate_id]


# ============================================================================
# Test GenerateIntentUseCase
# ============================================================================

class TestGenerateIntentUseCase:
    """Tests for GenerateIntentUseCase."""

    @pytest.fixture
    def dependencies(self):
        return {
            'intent_repo': InMemoryIntentRepository(),
            'message_bus': FakeMessageBus(),
            'clock': FakeClock(),
            'audit_trail': FakeAuditTrail(),
        }

    @pytest.fixture
    def use_case(self, dependencies):
        return GenerateIntentUseCase(**dependencies)

    @pytest.fixture
    def sample_signal(self):
        return TradingSignal(
            signal_id="sig-001",
            timestamp=datetime(2025, 12, 23, 12, 0, 0),
            symbol="BTCUSDC",
            side="BUY",
            confidence=0.85,
            entry_price=Decimal("90000.00"),
            stop_price=Decimal("88200.00"),  # 2% stop
            target_price=Decimal("93600.00"),  # 4% target
            strategy_name="Mean Reversion",
            reason="Bounce from MA99 support",
            regime=MarketRegime(
                regime="sideways",
                confidence=0.7,
                indicators={"MA99": 89000},
            ),
        )

    def test_generate_intent_success(self, use_case, sample_signal, dependencies):
        """Test successful intent generation."""
        capital = Decimal("1000.00")
        max_risk = Decimal("1.0")

        intent = use_case.execute(
            signal=sample_signal,
            client_id=1,
            capital=capital,
            max_risk_percent=max_risk,
        )

        # Check intent properties
        assert intent.client_id == 1
        assert intent.symbol == "BTCUSDC"
        assert intent.side == "BUY"
        assert intent.status == IntentStatus.PENDING
        assert intent.entry_price == Decimal("90000.00")
        assert intent.stop_price == Decimal("88200.00")

        # Check position sizing (1% of $1000 = $10 risk)
        # Stop distance = $90000 - $88200 = $1800
        # Quantity = $10 / $1800 = 0.00555555...
        expected_quantity = (capital * max_risk / Decimal("100")) / Decimal("1800")
        expected_quantity = expected_quantity.quantize(Decimal("0.00000001"))
        assert intent.quantity == expected_quantity

        # Check risk tracking
        assert intent.risk_amount == Decimal("10.00")
        assert intent.risk_percent == Decimal("1.0")

        # Check event was published
        assert len(dependencies['message_bus'].published_events) == 1
        event = dependencies['message_bus'].published_events[0]
        assert event.event_type == "IntentCreated"
        assert event.client_id == 1

        # Check audit trail
        assert len(dependencies['audit_trail'].records) == 1

    def test_generate_intent_zero_capital_raises(self, use_case, sample_signal):
        """Test that zero capital raises ValueError."""
        with pytest.raises(ValueError, match="Capital must be positive"):
            use_case.execute(
                signal=sample_signal,
                client_id=1,
                capital=Decimal("0"),
            )

    def test_generate_intent_invalid_risk_percent_raises(self, use_case, sample_signal):
        """Test that invalid risk percent raises ValueError."""
        with pytest.raises(ValueError, match="Risk percent must be between"):
            use_case.execute(
                signal=sample_signal,
                client_id=1,
                capital=Decimal("1000"),
                max_risk_percent=Decimal("150"),  # > 100%
            )


# ============================================================================
# Test ValidateIntentUseCase
# ============================================================================

class TestValidateIntentUseCase:
    """Tests for ValidateIntentUseCase."""

    @pytest.fixture
    def dependencies(self):
        return {
            'intent_repo': InMemoryIntentRepository(),
            'policy_state_repo': InMemoryPolicyStateRepository(),
            'risk_policy': FakeRiskPolicy(),
            'message_bus': FakeMessageBus(),
            'clock': FakeClock(),
            'audit_trail': FakeAuditTrail(),
        }

    @pytest.fixture
    def use_case(self, dependencies):
        return ValidateIntentUseCase(**dependencies)

    @pytest.fixture
    def pending_intent(self, dependencies):
        intent = TradingIntent(
            intent_id="intent-001",
            client_id=1,
            symbol="BTCUSDC",
            side="BUY",
            status=IntentStatus.PENDING,
            quantity=Decimal("0.00555556"),
            entry_price=Decimal("90000.00"),
            stop_price=Decimal("88200.00"),
            target_price=Decimal("93600.00"),
            strategy_name="Mean Reversion",
            regime="sideways",
            confidence=0.85,
            reason="Test intent",
            created_at=datetime(2025, 12, 23, 12, 0, 0),
            risk_amount=Decimal("10.00"),
            risk_percent=Decimal("1.0"),
        )
        dependencies['intent_repo'].save(intent)
        return intent

    def test_validate_intent_success(self, use_case, pending_intent, dependencies):
        """Test successful intent validation."""
        updated_intent, result = use_case.execute("intent-001")

        assert updated_intent.status == IntentStatus.VALIDATED
        assert updated_intent.validated_at is not None
        assert result.passed is True
        assert "risk checks passed" in result.reason.lower()

        # Check audit trail
        audit_records = dependencies['audit_trail'].get_history("intent-001")
        assert len(audit_records) == 1
        assert audit_records[0]['event_type'] == "intent_validated"

    def test_validate_intent_trade_risk_fails(self, use_case, pending_intent, dependencies):
        """Test validation fails when trade risk check fails."""
        # Configure risk policy to fail trade check
        dependencies['risk_policy'].trade_check_passes = False

        updated_intent, result = use_case.execute("intent-001")

        assert updated_intent.status == IntentStatus.FAILED
        assert result.passed is False
        assert "risk exceeds" in result.reason.lower()

        # Check audit trail
        audit_records = dependencies['audit_trail'].get_history("intent-001")
        assert len(audit_records) == 1
        assert audit_records[0]['event_type'] == "intent_validation_failed"

    def test_validate_intent_drawdown_fails(self, use_case, pending_intent, dependencies):
        """Test validation fails when drawdown limit exceeded."""
        # Configure risk policy to fail drawdown check
        dependencies['risk_policy'].drawdown_check_passes = False

        updated_intent, result = use_case.execute("intent-001")

        assert updated_intent.status == IntentStatus.FAILED
        assert result.passed is False
        assert "drawdown" in result.reason.lower()

        # Check trading was paused
        assert 1 in dependencies['risk_policy'].paused_clients

        # Check pause event was published
        pause_events = [e for e in dependencies['message_bus'].published_events if e.event_type == "PolicyPaused"]
        assert len(pause_events) == 1

    def test_validate_intent_not_found_raises(self, use_case):
        """Test that validating non-existent intent raises ValueError."""
        with pytest.raises(ValueError, match="Intent .* not found"):
            use_case.execute("nonexistent")

    def test_validate_intent_not_pending_raises(self, use_case, pending_intent, dependencies):
        """Test that validating non-pending intent raises ValueError."""
        # Mark intent as executed
        executed_intent = pending_intent.mark_as_executed(
            timestamp=datetime.now(),
            order_id="order-001",
            exchange_order_id="binance-001",
            fill_price=Decimal("90000"),
            fill_quantity=Decimal("0.005"),
        )
        dependencies['intent_repo'].save(executed_intent)

        with pytest.raises(ValueError, match="is not pending"):
            use_case.execute("intent-001")


# ============================================================================
# Test ExecuteIntentUseCase
# ============================================================================

class TestExecuteIntentUseCase:
    """Tests for ExecuteIntentUseCase."""

    @pytest.fixture
    def dependencies(self):
        return {
            'intent_repo': InMemoryIntentRepository(),
            'order_execution': FakeOrderExecution(),
            'message_bus': FakeMessageBus(),
            'clock': FakeClock(),
            'audit_trail': FakeAuditTrail(),
        }

    @pytest.fixture
    def use_case(self, dependencies):
        return ExecuteIntentUseCase(**dependencies)

    @pytest.fixture
    def validated_intent(self, dependencies):
        intent = TradingIntent(
            intent_id="intent-001",
            client_id=1,
            symbol="BTCUSDC",
            side="BUY",
            status=IntentStatus.PENDING,
            quantity=Decimal("0.00555556"),
            entry_price=Decimal("90000.00"),
            stop_price=Decimal("88200.00"),
            target_price=Decimal("93600.00"),
            strategy_name="Mean Reversion",
            regime="sideways",
            confidence=0.85,
            reason="Test intent",
            created_at=datetime(2025, 12, 23, 12, 0, 0),
            risk_amount=Decimal("10.00"),
            risk_percent=Decimal("1.0"),
        )
        # Mark as validated
        validated = intent.mark_as_validated(datetime(2025, 12, 23, 12, 0, 0))
        dependencies['intent_repo'].save(validated)
        return validated

    def test_execute_intent_dry_run_success(self, use_case, validated_intent, dependencies):
        """Test dry run execution."""
        updated_intent, order_id = use_case.execute("intent-001", dry_run=True)

        assert updated_intent.status == IntentStatus.EXECUTED
        assert updated_intent.executed_at is not None
        assert order_id is not None
        assert "DRY" in order_id

        # Check no real order was placed
        assert len(dependencies['order_execution'].placed_orders) == 0

        # Check audit trail
        audit_records = dependencies['audit_trail'].get_history("intent-001")
        assert any("dry_run" in r['event_type'] for r in audit_records)

    def test_execute_intent_live_success(self, use_case, validated_intent, dependencies):
        """Test live execution."""
        updated_intent, order_id = use_case.execute("intent-001", dry_run=False)

        assert updated_intent.status == IntentStatus.EXECUTED
        assert order_id is not None
        assert "DRY" not in order_id

        # Check real order was placed
        assert len(dependencies['order_execution'].placed_orders) == 1
        placed_order = dependencies['order_execution'].placed_orders[0]
        assert placed_order['symbol'] == "BTCUSDC"
        assert placed_order['side'] == "BUY"

        # Check event was published
        order_events = [e for e in dependencies['message_bus'].published_events if e.event_type == "OrderPlaced"]
        assert len(order_events) == 1

    def test_execute_intent_live_fails(self, use_case, validated_intent, dependencies):
        """Test live execution failure."""
        # Configure order execution to fail
        dependencies['order_execution'].should_succeed = False

        updated_intent, order_id = use_case.execute("intent-001", dry_run=False)

        assert updated_intent.status == IntentStatus.FAILED
        assert order_id is None

        # Check audit trail has failure record
        audit_records = dependencies['audit_trail'].get_history("intent-001")
        assert any("failed" in r['event_type'] for r in audit_records)

    def test_execute_intent_not_validated_raises(self, use_case, dependencies):
        """Test executing non-validated intent raises ValueError."""
        # Create pending (not validated) intent
        pending = TradingIntent(
            intent_id="intent-002",
            client_id=1,
            symbol="BTCUSDC",
            side="BUY",
            status=IntentStatus.PENDING,
            quantity=Decimal("0.005"),
            entry_price=Decimal("90000"),
            stop_price=Decimal("88200"),
            strategy_name="Test",
            regime="sideways",
            confidence=0.8,
            reason="Test",
            created_at=datetime.now(),
            risk_amount=Decimal("10"),
            risk_percent=Decimal("1"),
        )
        dependencies['intent_repo'].save(pending)

        with pytest.raises(ValueError, match="is not validated"):
            use_case.execute("intent-002")
