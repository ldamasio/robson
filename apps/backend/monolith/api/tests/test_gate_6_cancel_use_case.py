"""
Unit tests for CancelOperationUseCase (Gate 6).

Tests the business logic for operation cancellation at the domain level.
No Django views or REST API - pure use case testing.
"""

import pytest
from unittest.mock import Mock, MagicMock

from api.application.use_cases import (
    CancelOperationUseCase,
    CancelOperationCommand,
    CancelOperationResult,
)
from api.models import Operation, Symbol, Strategy, Order
from clients.models import Client
from api.models.trading import InvalidOperationStatusError


@pytest.mark.django_db
class TestCancelOperationUseCase:
    """Unit tests for CancelOperationUseCase (Gate 6)."""

    @pytest.fixture
    def client(self):
        """Create test client."""
        return Client.objects.create(name="Test Client", email="test@example.com")

    @pytest.fixture
    def symbol(self, client):
        """Create test symbol."""
        return Symbol.objects.create(
            client=client,
            name="BTCUSDT",
            base_asset="BTC",
            quote_asset="USDT",
        )

    @pytest.fixture
    def strategy(self, client):
        """Create test strategy."""
        return Strategy.objects.create(
            client=client,
            name="Test Strategy",
            description="Strategy for testing",
        )

    def test_cancel_active_operation_success(self, client, symbol, strategy):
        """Use case cancels ACTIVE operation."""
        # Arrange: Operation in ACTIVE state
        operation = Operation.objects.create(
            client=client,
            strategy=strategy,
            symbol=symbol,
            side="BUY",
            status="ACTIVE",
        )

        use_case = CancelOperationUseCase()
        command = CancelOperationCommand(
            operation_id=operation.id,
            client_id=client.id
        )

        # Act
        result = use_case.execute(command)

        # Assert
        assert result.success is True
        assert result.operation_id == operation.id
        assert result.previous_status == "ACTIVE"
        assert result.new_status == "CANCELLED"
        assert result.error_message is None

        # Verify database state
        operation.refresh_from_db()
        assert operation.status == "CANCELLED"

    def test_cancel_planned_operation_success(self, client, symbol, strategy):
        """Use case cancels PLANNED operation."""
        # Arrange: Operation in PLANNED state
        operation = Operation.objects.create(
            client=client,
            strategy=strategy,
            symbol=symbol,
            side="BUY",
            status="PLANNED",
        )

        use_case = CancelOperationUseCase()
        command = CancelOperationCommand(
            operation_id=operation.id,
            client_id=client.id
        )

        # Act
        result = use_case.execute(command)

        # Assert
        assert result.success is True
        assert result.previous_status == "PLANNED"
        assert result.new_status == "CANCELLED"

        # Verify database state
        operation.refresh_from_db()
        assert operation.status == "CANCELLED"

    def test_cancel_closed_operation_fails(self, client, symbol, strategy):
        """Cannot cancel CLOSED operations (terminal state)."""
        # Arrange: Operation in CLOSED state
        operation = Operation.objects.create(
            client=client,
            strategy=strategy,
            symbol=symbol,
            side="BUY",
            status="CLOSED",
        )

        use_case = CancelOperationUseCase()
        command = CancelOperationCommand(
            operation_id=operation.id,
            client_id=client.id
        )

        # Act
        result = use_case.execute(command)

        # Assert
        assert result.success is False
        assert result.previous_status == "CLOSED"
        assert result.new_status == "CLOSED"  # Status unchanged
        assert "Cannot cancel operation in CLOSED state" in result.error_message

        # Verify database state unchanged
        operation.refresh_from_db()
        assert operation.status == "CLOSED"

    def test_cancel_already_cancelled_succeeds_idempotent(self, client, symbol, strategy):
        """Cancelling CANCELLED operation is safe no-op."""
        # Arrange: Operation already CANCELLED
        operation = Operation.objects.create(
            client=client,
            strategy=strategy,
            symbol=symbol,
            side="BUY",
            status="CANCELLED",
        )

        use_case = CancelOperationUseCase()
        command = CancelOperationCommand(
            operation_id=operation.id,
            client_id=client.id
        )

        # Act
        result = use_case.execute(command)

        # Assert: Success (idempotent)
        assert result.success is True
        assert result.previous_status == "CANCELLED"
        assert result.new_status == "CANCELLED"
        assert result.error_message is None

    def test_cross_tenant_isolation_enforced(self, client, symbol, strategy):
        """User cannot cancel another user's operation."""
        # Arrange: Create two clients
        client_a = client
        client_b = Client.objects.create(name="Other Client", email="other@example.com")

        # Operation owned by client A
        operation = Operation.objects.create(
            client=client_a,
            strategy=strategy,
            symbol=symbol,
            side="BUY",
            status="ACTIVE",
        )

        use_case = CancelOperationUseCase()
        # Try to cancel with client B
        command = CancelOperationCommand(
            operation_id=operation.id,
            client_id=client_b.id
        )

        # Act
        result = use_case.execute(command)

        # Assert: Access denied
        assert result.success is False
        assert result.error_message == "Operation not found or access denied"

        # Verify database state unchanged
        operation.refresh_from_db()
        assert operation.status == "ACTIVE"  # Not cancelled

    def test_nonexistent_operation_fails(self, client):
        """Cancelling non-existent operation fails gracefully."""
        # Arrange: No operation exists
        use_case = CancelOperationUseCase()
        command = CancelOperationCommand(
            operation_id=99999,  # Non-existent
            client_id=client.id
        )

        # Act
        result = use_case.execute(command)

        # Assert
        assert result.success is False
        assert result.error_message == "Operation not found or access denied"

    def test_returns_success_result(self, client, symbol, strategy):
        """Use case returns CancelOperationResult with correct fields."""
        # Arrange
        operation = Operation.objects.create(
            client=client,
            strategy=strategy,
            symbol=symbol,
            side="BUY",
            status="ACTIVE",
        )

        use_case = CancelOperationUseCase()
        command = CancelOperationCommand(
            operation_id=operation.id,
            client_id=client.id
        )

        # Act
        result = use_case.execute(command)

        # Assert: All fields populated correctly
        assert isinstance(result, CancelOperationResult)
        assert hasattr(result, 'success')
        assert hasattr(result, 'operation_id')
        assert hasattr(result, 'previous_status')
        assert hasattr(result, 'new_status')
        assert hasattr(result, 'error_message')
        assert result.success is True
        assert result.operation_id == operation.id
        assert result.previous_status == "ACTIVE"
        assert result.new_status == "CANCELLED"

    def test_returns_error_result_for_invalid_state(self, client, symbol, strategy):
        """Use case returns CancelOperationResult with error_message."""
        # Arrange: CLOSED operation cannot be cancelled
        operation = Operation.objects.create(
            client=client,
            strategy=strategy,
            symbol=symbol,
            side="BUY",
            status="CLOSED",
        )

        use_case = CancelOperationUseCase()
        command = CancelOperationCommand(
            operation_id=operation.id,
            client_id=client.id
        )

        # Act
        result = use_case.execute(command)

        # Assert: Error result
        assert result.success is False
        assert result.error_message is not None
        assert "Cannot cancel" in result.error_message
        assert "CLOSED" in result.error_message


@pytest.mark.django_db
class TestCancelOperationUseCaseWithOrders:
    """Test cancellation with orders attached to operation."""

    @pytest.fixture
    def setup_data(self):
        """Setup test data with orders."""
        client = Client.objects.create(name="Test Client", email="test@example.com")
        symbol = Symbol.objects.create(
            client=client,
            name="BTCUSDT",
            base_asset="BTC",
            quote_asset="USDT",
        )
        strategy = Strategy.objects.create(
            client=client,
            name="Test Strategy",
        )
        return client, symbol, strategy

    def test_cancel_active_operation_with_entry_orders(self, setup_data):
        """Cancelling operation with entry orders preserves orders."""
        client, symbol, strategy = setup_data

        # Create operation with entry order
        operation = Operation.objects.create(
            client=client,
            strategy=strategy,
            symbol=symbol,
            side="BUY",
            status="ACTIVE",
        )

        entry_order = Order.objects.create(
            client=client,
            symbol=symbol,
            strategy=strategy,
            side="BUY",
            order_type="MARKET",
            quantity="0.1",
            price="50000",
            status="FILLED",
        )
        operation.entry_orders.add(entry_order)

        # Cancel
        use_case = CancelOperationUseCase()
        command = CancelOperationCommand(
            operation_id=operation.id,
            client_id=client.id
        )
        result = use_case.execute(command)

        # Assert: Operation cancelled, orders preserved
        assert result.success is True
        operation.refresh_from_db()
        assert operation.status == "CANCELLED"
        assert operation.entry_orders.count() == 1  # Entry order still there
        assert operation.exit_orders.count() == 0  # No exit orders


class TestCancelOperationUseCaseMocked:
    """Tests with mocked repository for pure unit testing (no DB)."""

    def test_cancel_with_mock_repository(self):
        """Test use case with mocked repository (no database)."""
        # Arrange: Mock operation
        mock_operation = Mock()
        mock_operation.id = 123
        mock_operation.status = "ACTIVE"
        mock_operation.set_status = Mock()
        mock_operation.save = Mock()

        # Arrange: Mock repository
        mock_repo = Mock()
        mock_repo.get = Mock(return_value=mock_operation)

        # Act
        use_case = CancelOperationUseCase(operation_repository=mock_repo)
        command = CancelOperationCommand(operation_id=123, client_id=1)
        result = use_case.execute(command)

        # Assert
        assert result.success is True
        assert result.operation_id == 123
        assert result.previous_status == "ACTIVE"
        assert result.new_status == "CANCELLED"
        mock_operation.set_status.assert_called_once_with("CANCELLED")
        mock_operation.save.assert_called_once()

    def test_cancel_not_found_with_mock_repository(self):
        """Test use case with mocked repository (operation not found)."""
        # Arrange: Mock repository raises exception
        mock_repo = Mock()
        mock_repo.get = Mock(side_effect=Operation.DoesNotExist)

        # Act
        use_case = CancelOperationUseCase(operation_repository=mock_repo)
        command = CancelOperationCommand(operation_id=999, client_id=1)
        result = use_case.execute(command)

        # Assert
        assert result.success is False
        assert result.error_message == "Operation not found or access denied"
