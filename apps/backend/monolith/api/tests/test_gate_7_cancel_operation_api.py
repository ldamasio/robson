"""
Tests for Operation Cancellation API endpoint (Gate 7).

Tests cover POST /api/operations/<operation_id>/cancel/ endpoint.
This is a DRF integration test that calls the CancelOperationUseCase through the REST API.
"""

import pytest
from rest_framework.test import APIClient
from django.contrib.auth import get_user_model

from clients.models import Client
from api.models import Operation, Symbol, Strategy

User = get_user_model()


@pytest.fixture
def api_client():
    """Create API client."""
    return APIClient()


@pytest.fixture
def client_instance(db):
    """Create a test client."""
    return Client.objects.create(
        name="Test Client"
    )


@pytest.fixture
def user(db, client_instance):
    """Create a test user associated with a client."""
    user = User.objects.create_user(
        username="testuser",
        email="test@example.com",
        password="testpass123"
    )
    user.client = client_instance
    user.save()
    return user


@pytest.fixture
def other_client(db):
    """Create another test client for cross-tenant tests."""
    return Client.objects.create(
        name="Other Client",
        email="other@example.com"
    )


@pytest.fixture
def other_user(db, other_client):
    """Create a test user associated with the other client."""
    user = User.objects.create_user(
        username="otheruser",
        email="other@example.com",
        password="testpass123"
    )
    user.client = other_client
    user.save()
    return user


@pytest.fixture
def symbol(db, client_instance):
    """Create a test symbol."""
    return Symbol.objects.create(
        client=client_instance,
        name="BTCUSDT",
        base_asset="BTC",
        quote_asset="USDT",
        description="Bitcoin to USDT"
    )


@pytest.fixture
def strategy(db, client_instance):
    """Create a test strategy."""
    return Strategy.objects.create(
        client=client_instance,
        name="Test Strategy",
        description="Test trading strategy",
        config={"type": "mean_reversion"},
        risk_config={"max_risk_percent": 1.0}
    )


@pytest.mark.django_db
class TestCancelOperationAPI:
    """Tests for POST /api/operations/<operation_id>/cancel/"""

    def test_cancel_active_operation_success(self, api_client, user, symbol, strategy):
        """Test cancelling own ACTIVE operation returns 200 OK."""
        # Create operation in ACTIVE state
        operation = Operation.objects.create(
            client=user.client,
            strategy=strategy,
            symbol=symbol,
            side="BUY",
            status="ACTIVE",
        )

        # Authenticate
        api_client.force_authenticate(user=user)

        # Cancel operation
        response = api_client.post(f"/api/operations/{operation.id}/cancel/")

        # Assert
        assert response.status_code == 200
        assert response.data["success"] is True
        assert response.data["operation_id"] == operation.id
        assert response.data["previous_status"] == "ACTIVE"
        assert response.data["new_status"] == "CANCELLED"

        # Verify database state
        operation.refresh_from_db()
        assert operation.status == "CANCELLED"

    def test_cancel_planned_operation_success(self, api_client, user, symbol, strategy):
        """Test cancelling own PLANNED operation returns 200 OK."""
        operation = Operation.objects.create(
            client=user.client,
            strategy=strategy,
            symbol=symbol,
            side="BUY",
            status="PLANNED",
        )

        api_client.force_authenticate(user=user)
        response = api_client.post(f"/api/operations/{operation.id}/cancel/")

        assert response.status_code == 200
        assert response.data["previous_status"] == "PLANNED"
        assert response.data["new_status"] == "CANCELLED"

        operation.refresh_from_db()
        assert operation.status == "CANCELLED"

    def test_cancel_already_cancelled_idempotent(self, api_client, user, symbol, strategy):
        """Test cancelling already CANCELLED operation returns 200 OK (idempotent)."""
        operation = Operation.objects.create(
            client=user.client,
            strategy=strategy,
            symbol=symbol,
            side="BUY",
            status="CANCELLED",
        )

        api_client.force_authenticate(user=user)
        response = api_client.post(f"/api/operations/{operation.id}/cancel/")

        # Idempotent: returns 200 OK
        assert response.status_code == 200
        assert response.data["success"] is True
        assert response.data["previous_status"] == "CANCELLED"
        assert response.data["new_status"] == "CANCELLED"

    def test_cancel_closed_operation_returns_409(self, api_client, user, symbol, strategy):
        """Test cancelling CLOSED operation returns 409 Conflict."""
        operation = Operation.objects.create(
            client=user.client,
            strategy=strategy,
            symbol=symbol,
            side="BUY",
            status="CLOSED",
        )

        api_client.force_authenticate(user=user)
        response = api_client.post(f"/api/operations/{operation.id}/cancel/")

        assert response.status_code == 409
        assert response.data["success"] is False
        assert response.data["operation_id"] == operation.id
        assert "error" in response.data
        assert "Cannot cancel" in response.data["error"]
        assert "CLOSED" in response.data["error"]

        # Verify no state change
        operation.refresh_from_db()
        assert operation.status == "CLOSED"

    def test_cancel_cross_tenant_returns_404(self, api_client, user, other_user, symbol, strategy):
        """Test cancelling another tenant's operation returns 404 Not Found."""
        # Create operation owned by other_user
        operation = Operation.objects.create(
            client=other_user.client,
            strategy=strategy,
            symbol=symbol,
            side="BUY",
            status="ACTIVE",
        )

        # Try to cancel as different user
        api_client.force_authenticate(user=user)
        response = api_client.post(f"/api/operations/{operation.id}/cancel/")

        assert response.status_code == 404
        assert response.data["success"] is False
        assert "error" in response.data
        assert "not found" in response.data["error"].lower()

        # Verify no state change
        operation.refresh_from_db()
        assert operation.status == "ACTIVE"

    def test_cancel_nonexistent_returns_404(self, api_client, user):
        """Test cancelling non-existent operation returns 404 Not Found."""
        api_client.force_authenticate(user=user)
        response = api_client.post("/api/operations/99999/cancel/")

        assert response.status_code == 404
        assert response.data["success"] is False
        assert "error" in response.data

    def test_cancel_unauthenticated_returns_401(self, api_client, user, symbol, strategy):
        """Test cancelling without authentication returns 401 Unauthorized."""
        operation = Operation.objects.create(
            client=user.client,
            strategy=strategy,
            symbol=symbol,
            side="BUY",
            status="ACTIVE",
        )

        # Do not authenticate
        response = api_client.post(f"/api/operations/{operation.id}/cancel/")

        assert response.status_code == 401

        # Verify no state change
        operation.refresh_from_db()
        assert operation.status == "ACTIVE"

    def test_cancel_response_json_structure(self, api_client, user, symbol, strategy):
        """Test response JSON structure matches specification."""
        operation = Operation.objects.create(
            client=user.client,
            strategy=strategy,
            symbol=symbol,
            side="BUY",
            status="ACTIVE",
        )

        api_client.force_authenticate(user=user)
        response = api_client.post(f"/api/operations/{operation.id}/cancel/")

        assert response.status_code == 200
        # Check all required fields
        assert "success" in response.data
        assert "operation_id" in response.data
        assert "previous_status" in response.data
        assert "new_status" in response.data
        assert isinstance(response.data["success"], bool)
        assert isinstance(response.data["operation_id"], int)
        assert isinstance(response.data["previous_status"], str)
        assert isinstance(response.data["new_status"], str)


@pytest.mark.django_db
class TestCancelOperationAPIEdgeCases:
    """Edge case tests for operation cancellation API."""

    def test_cancel_user_without_client_returns_400(self, api_client):
        """Test cancelling when user has no associated client returns 400."""
        # Create user without client
        user = User.objects.create_user(
            username="noclient",
            email="noclient@example.com",
            password="testpass123"
        )
        # No client associated

        api_client.force_authenticate(user=user)
        response = api_client.post("/api/operations/123/cancel/")

        assert response.status_code == 400
        assert "error" in response.data

    def test_cancel_with_invalid_operation_id_format(self, api_client, user):
        """Test cancelling with non-numeric operation_id."""
        api_client.force_authenticate(user=user)

        # Django URL routing will reject non-integer IDs before view is called
        response = api_client.post("/api/operations/invalid/cancel/")

        # Should return 404 (Django URL pattern match failure)
        assert response.status_code == 404


@pytest.mark.django_db
class TestCancelOperationAPIMultipleOperations:
    """Tests with multiple operations to verify correctness."""

    def test_cancel_only_targeted_operation(self, api_client, user, symbol, strategy):
        """Test that only the targeted operation is cancelled, not others."""
        # Create multiple operations
        op1 = Operation.objects.create(
            client=user.client,
            strategy=strategy,
            symbol=symbol,
            side="BUY",
            status="ACTIVE",
        )
        op2 = Operation.objects.create(
            client=user.client,
            strategy=strategy,
            symbol=symbol,
            side="SELL",
            status="ACTIVE",
        )

        api_client.force_authenticate(user=user)

        # Cancel only op1
        response = api_client.post(f"/api/operations/{op1.id}/cancel/")

        assert response.status_code == 200

        # Verify op1 is cancelled, op2 is still active
        op1.refresh_from_db()
        op2.refresh_from_db()
        assert op1.status == "CANCELLED"
        assert op2.status == "ACTIVE"
