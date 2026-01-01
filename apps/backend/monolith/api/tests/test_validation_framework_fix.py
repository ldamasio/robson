"""
Minimal test for ValidationFramework invocation fix.

Verifies that pattern_trigger auto-validation executes without raising
TypeError about unexpected client_id keyword argument.
"""
import pytest
from django.test import RequestFactory
from rest_framework.test import force_authenticate
from clients.models import Client
from api.models import Symbol, Strategy
from api.views.trading_intent_views import pattern_trigger
import json


@pytest.mark.django_db
class TestValidationFrameworkInvocationFix:
    """Test that ValidationFramework is invoked correctly."""

    def test_pattern_trigger_auto_validate_does_not_raise(self):
        """
        Test that pattern_trigger with auto_validate=true returns validation_result
        without raising TypeError about client_id.
        """
        # Setup: Create test data
        from django.contrib.auth import get_user_model
        from decimal import Decimal

        User = get_user_model()

        # Create client
        client = Client.objects.create(
            name="Test Client",
            email="test@example.com",
        )

        # Create user
        user = User.objects.create_user(
            username="testuser",
            email="test@example.com",
            password="testpass123",
        )
        client.users.add(user)

        # Create symbol
        symbol = Symbol.objects.create(
            name="BTCUSDT",
            base_asset="BTC",
            quote_asset="USDT",
            client=client,
        )

        # Create strategy (optional)
        strategy = Strategy.objects.create(
            name="Test Strategy",
            client=client,
        )

        # Prepare request
        factory = RequestFactory()
        payload = {
            "pattern_code": "TEST_PATTERN",
            "pattern_event_id": "test_validation_fix_evt_001",
            "symbol": symbol.id,
            "side": "BUY",
            "entry_price": "95000",
            "stop_price": "93500",
            "capital": "100",
            "auto_validate": True,  # This triggers the fixed code path
        }

        if strategy:
            payload["strategy"] = strategy.id

        request = factory.post(
            "/api/pattern-triggers/",
            data=json.dumps(payload),
            content_type="application/json",
        )
        force_authenticate(request, user=user)

        # Execute: Call pattern_trigger view
        response = pattern_trigger(request)

        # Assert: Should return 201 with validation_result
        assert response.status_code == 201, f"Expected 201, got {response.status_code}: {response.data}"
        assert response.data.get("status") == "PROCESSED"
        assert "intent_id" in response.data
        assert "validation_result" in response.data, "auto_validate=true should return validation_result"

        # Verify validation_result structure
        validation_result = response.data["validation_result"]
        assert "status" in validation_result
        assert "issues" in validation_result
