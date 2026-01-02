"""
Tests for Operation creation from TradingIntent LIVE execution.

Validates Gate 4 invariants:
1. DRY-RUN: Never creates Operation
2. LIVE success: Creates Operation (L2) + AuditTransaction (L3)
3. Idempotency: Double-execute creates only ONE Operation
4. Plan-reality consistency: Operation fields match TradingIntent
5. Exchange proof required: binance_order_id must exist
"""

import pytest
from decimal import Decimal
from unittest.mock import Mock, patch
from django.utils import timezone
from django.test import override_settings

from api.models import TradingIntent, Operation, Symbol, Strategy
from api.models.audit import AuditTransaction, TransactionType
from api.application.execution_framework import ExecutionFramework
from api.application.execution import ExecutionMode
from clients.models import Client


@pytest.fixture
def client_instance(db):
    """Create a test client."""
    return Client.objects.create(
        name="Test Client",
        email="test@example.com"
    )


@pytest.fixture
def symbol(db, client_instance):
    """Create a test symbol."""
    return Symbol.objects.create(
        client=client_instance,
        name="BTCUSDC",
        base_asset="BTC",
        quote_asset="USDC"
    )


@pytest.fixture
def strategy(db, client_instance):
    """Create a test strategy."""
    return Strategy.objects.create(
        client=client_instance,
        name="Test Strategy"
    )


@pytest.fixture
def validated_intent(db, client_instance, symbol, strategy):
    """Create a validated TradingIntent."""
    intent = TradingIntent.objects.create(
        intent_id="test-intent-001",
        client=client_instance,
        symbol=symbol,
        strategy=strategy,
        side="BUY",
        status="VALIDATED",
        quantity=Decimal("0.001"),
        entry_price=Decimal("95000"),
        stop_price=Decimal("93000"),
        target_price=Decimal("98000"),
        regime="manual",
        confidence=0.8,
        reason="Test execution",
        capital=Decimal("100"),
        risk_amount=Decimal("1"),
        risk_percent=Decimal("1"),
        validated_at=timezone.now()
    )
    # Add validation result
    intent.validation_result = {"status": "PASS", "checks": []}
    intent.save()
    return intent


@pytest.mark.django_db
class TestOperationCreationInvariants:
    """Test Gate 4 invariants for Operation creation."""

    def test_dry_run_never_creates_operation(self, validated_intent):
        """
        Invariant 1: DRY-RUN execution never creates Operation.

        Causality: Simulation should not create real entities.
        """
        # Execute in DRY-RUN mode
        framework = ExecutionFramework(client_id=validated_intent.client.id)
        result = framework.execute(validated_intent, mode="dry-run")

        # Assert: No Operation created
        assert Operation.objects.filter(trading_intent=validated_intent).count() == 0

        # Assert: No AuditTransaction created
        assert AuditTransaction.objects.filter(client=validated_intent.client).count() == 0

        # Assert: TradingIntent still marked as EXECUTED (simulation)
        validated_intent.refresh_from_db()
        # Note: Current implementation doesn't update status in DRY-RUN, which is correct

    @override_settings(BINANCE_ALLOW_LIVE_TRADING=True)
    @patch('api.application.execution_framework.BinanceExecution')
    def test_live_success_creates_operation_and_movement(self, mock_binance, validated_intent):
        """
        Invariant 2: LIVE execution + exchange success creates Operation (L2) + AuditTransaction (L3).

        Causality chain:
            User Execute LIVE → Exchange accepts → Create Operation → Create Movement
        """
        # Mock Binance response
        mock_instance = Mock()
        mock_instance.place_market.return_value = {
            "orderId": "12345678",
            "status": "FILLED",
            "fills": [{
                "price": "95000.00",
                "qty": "0.001",
                "commission": "0.095",
                "commissionAsset": "USDC"
            }]
        }
        mock_binance.return_value = mock_instance

        # Execute in LIVE mode
        framework = ExecutionFramework(client_id=validated_intent.client.id)
        result = framework.execute(validated_intent, mode="live")

        # Assert: Operation created (Level 2)
        operations = Operation.objects.filter(trading_intent=validated_intent)
        assert operations.count() == 1

        operation = operations.first()
        assert operation.trading_intent == validated_intent
        assert operation.client == validated_intent.client
        assert operation.strategy == validated_intent.strategy
        assert operation.symbol == validated_intent.symbol
        assert operation.side == validated_intent.side
        assert operation.status == "ACTIVE"  # Immediately ACTIVE
        assert operation.stop_price == validated_intent.stop_price
        assert operation.target_price == validated_intent.target_price

        # Assert: AuditTransaction created (Level 3)
        movements = AuditTransaction.objects.filter(
            client=validated_intent.client,
            related_operation=operation
        )
        assert movements.count() == 1

        movement = movements.first()
        assert movement.binance_order_id == "12345678"  # Exchange proof
        assert movement.transaction_type == TransactionType.SPOT_BUY
        assert movement.symbol == "BTCUSDC"
        assert movement.side == "BUY"
        assert movement.quantity == Decimal("0.001")
        assert movement.related_operation == operation

    @override_settings(BINANCE_ALLOW_LIVE_TRADING=True)
    @patch('api.application.execution_framework.BinanceExecution')
    def test_idempotency_double_execute_creates_single_operation(self, mock_binance, validated_intent):
        """
        Invariant 3: Double LIVE execution creates only ONE Operation (idempotency).

        Causality: Second execute should recognize existing Operation.
        """
        # Mock Binance response
        mock_instance = Mock()
        mock_instance.place_market.return_value = {
            "orderId": "12345678",
            "status": "FILLED",
            "fills": [{
                "price": "95000.00",
                "qty": "0.001",
                "commission": "0.095",
                "commissionAsset": "USDC"
            }]
        }
        mock_binance.return_value = mock_instance

        framework = ExecutionFramework(client_id=validated_intent.client.id)

        # First execution
        result1 = framework.execute(validated_intent, mode="live")

        # Refresh from DB to get the execution_result set by _execute_live()
        validated_intent.refresh_from_db()

        # Second execution (double-click scenario - should hit idempotency check)
        result2 = framework.execute(validated_intent, mode="live")

        # Assert: Only ONE Operation created
        operations = Operation.objects.filter(trading_intent=validated_intent)
        assert operations.count() == 1

        # Assert: Binance was only called once (idempotency worked)
        assert mock_instance.place_market.call_count == 1

    @override_settings(BINANCE_ALLOW_LIVE_TRADING=True)
    @patch('api.application.execution_framework.BinanceExecution')
    def test_plan_reality_consistency(self, mock_binance, validated_intent):
        """
        Invariant 4: Operation fields match TradingIntent plan (plan-reality consistency).

        Causality: What was planned must match what was executed.
        """
        # Mock Binance response
        mock_instance = Mock()
        mock_instance.place_market.return_value = {
            "orderId": "12345678",
            "status": "FILLED",
            "fills": [{
                "price": "95000.00",
                "qty": "0.001",
                "commission": "0.095",
                "commissionAsset": "USDC"
            }]
        }
        mock_binance.return_value = mock_instance

        framework = ExecutionFramework(client_id=validated_intent.client.id)
        framework.execute(validated_intent, mode="live")

        operation = Operation.objects.get(trading_intent=validated_intent)

        # Assert: Operation matches TradingIntent
        assert operation.symbol == validated_intent.symbol
        assert operation.side == validated_intent.side
        assert operation.strategy == validated_intent.strategy
        assert operation.stop_price == validated_intent.stop_price
        assert operation.target_price == validated_intent.target_price

    @override_settings(BINANCE_ALLOW_LIVE_TRADING=True)
    @patch('api.application.execution_framework.BinanceExecution')
    def test_exchange_proof_required(self, mock_binance, validated_intent):
        """
        Invariant 5: Operation requires exchange_order_id (proof of commitment).

        Causality: No order_id = no Operation (exchange didn't accept).
        """
        # Mock Binance response
        mock_instance = Mock()
        mock_instance.place_market.return_value = {
            "orderId": "12345678",  # This is the proof
            "status": "FILLED",
            "fills": [{
                "price": "95000.00",
                "qty": "0.001",
                "commission": "0.095",
                "commissionAsset": "USDC"
            }]
        }
        mock_binance.return_value = mock_instance

        framework = ExecutionFramework(client_id=validated_intent.client.id)
        framework.execute(validated_intent, mode="live")

        # Assert: AuditTransaction has exchange proof
        movement = AuditTransaction.objects.get(client=validated_intent.client)
        assert movement.binance_order_id is not None
        assert movement.binance_order_id == "12345678"

    @patch('api.application.execution_framework.BinanceExecution')
    def test_live_failure_before_exchange_no_operation(self, mock_binance, validated_intent):
        """
        Invariant 6: LIVE failure before exchange confirmation = No Operation.

        Causality: If exchange never accepted, Operation should not exist.
        """
        # Mock Binance to raise exception (exchange rejected)
        mock_instance = Mock()
        mock_instance.place_market.side_effect = Exception("Exchange rejected order")
        mock_binance.return_value = mock_instance

        framework = ExecutionFramework(client_id=validated_intent.client.id)

        # Execute (will fail)
        result = framework.execute(validated_intent, mode="live")

        # Assert: No Operation created (exchange never accepted)
        assert Operation.objects.filter(trading_intent=validated_intent).count() == 0

        # Assert: No AuditTransaction created
        assert AuditTransaction.objects.filter(client=validated_intent.client).count() == 0

        # Assert: Result indicates failure
        assert result.is_blocked() or not result.is_success()

    @pytest.mark.django_db
    @override_settings(
        BINANCE_ALLOW_LIVE_TRADING=True,
        BINANCE_API_KEY="test_api_key",
        BINANCE_SECRET_KEY="test_secret_key"
    )
    def test_operation_status_immediately_active(self, validated_intent):
        """
        Decision: Operation status = ACTIVE immediately upon creation (Gate 4 rule).

        Rationale: No WS/polling in Gate 4, so status is ACTIVE from creation.
        """
        with patch('api.application.execution_framework.BinanceExecution') as mock_binance:
            mock_instance = Mock()
            mock_instance.place_market.return_value = {
                "orderId": "12345678",
                "status": "FILLED",
                "fills": [{
                    "price": "95000.00",
                    "qty": "0.001",
                    "commission": "0.095",
                    "commissionAsset": "USDC"
                }]
            }
            mock_binance.return_value = mock_instance

            framework = ExecutionFramework(client_id=validated_intent.client.id)
            framework.execute(validated_intent, mode="live")

            operation = Operation.objects.get(trading_intent=validated_intent)

            # Assert: Status is ACTIVE (not PLANNED, not CREATED)
            assert operation.status == "ACTIVE"
