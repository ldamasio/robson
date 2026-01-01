"""
Phase 6 Tests: Pattern Triggers (POST /api/pattern-triggers/).

Test coverage:
A) Happy path - successful pattern trigger creation
B) Idempotency - duplicate pattern_event_id returns ALREADY_PROCESSED
C) LIVE auto-execution block - auto_execute=true with execution_mode=live returns 400
D) Decimal quantization regression - ensure decimal precision is clamped correctly

All tests are deterministic and avoid external services.
"""
import json
from decimal import Decimal

import pytest
from django.contrib.auth import get_user_model
from django.test import RequestFactory
from rest_framework.test import force_authenticate

from api.models import Symbol, Strategy, TradingIntent
from api.models.trading import PatternTrigger
from api.views.trading_intent_views import pattern_trigger
from clients.models import Client

User = get_user_model()


@pytest.mark.django_db
class TestPatternTriggerHappyPath:
    """Test successful pattern trigger creation (happy path)."""

    def test_pattern_trigger_happy_path(self):
        """
        Test POST /api/pattern-triggers/ happy path:
        - Creates TradingIntent
        - Returns 201 with intent_id
        - Records PatternTrigger for idempotency
        """
        # Setup: Create test data
        client = Client.objects.create(
            name="Test Client",
            email="test@example.com",
        )

        user = User.objects.create_user(
            username="testuser",
            email="test@example.com",
            password="testpass123",
        )
        client.users.add(user)

        symbol = Symbol.objects.create(
            name="BTCUSDT",
            base_asset="BTC",
            quote_asset="USDT",
            client=client,
        )

        strategy = Strategy.objects.create(
            name="Test Strategy",
            client=client,
        )

        # Prepare request
        factory = RequestFactory()
        payload = {
            "pattern_code": "HAMMER",
            "pattern_event_id": "test_happy_path_evt_001",
            "symbol": symbol.id,
            "strategy": strategy.id,
            "side": "BUY",
            "entry_price": "95000.00",
            "stop_price": "93500.00",
            "capital": "100.00",
            "auto_validate": False,  # Happy path without validation
        }

        request = factory.post(
            "/api/pattern-triggers/",
            data=json.dumps(payload),
            content_type="application/json",
        )
        force_authenticate(request, user=user)

        # Execute
        response = pattern_trigger(request)

        # Assert: Response structure
        assert response.status_code == 201, f"Expected 201, got {response.status_code}: {response.data}"
        assert response.data.get("status") == "PROCESSED"
        assert "intent_id" in response.data
        assert response.data.get("pattern_code") == "HAMMER"

        # Assert: TradingIntent created
        intent_id = response.data["intent_id"]
        intent = TradingIntent.objects.get(intent_id=intent_id)
        assert intent.client == client
        assert intent.symbol == symbol
        assert intent.strategy == strategy
        assert intent.side == "BUY"
        assert intent.entry_price == Decimal("95000.00")
        assert intent.stop_price == Decimal("93500.00")
        assert intent.capital == Decimal("100.00")
        assert intent.status == "PENDING"  # Not validated yet
        assert intent.pattern_code == "HAMMER"
        assert intent.pattern_source == "pattern"
        assert intent.pattern_event_id == "test_happy_path_evt_001"

        # Assert: PatternTrigger recorded for idempotency
        trigger = PatternTrigger.objects.get(
            client=client,
            pattern_event_id="test_happy_path_evt_001"
        )
        assert trigger.pattern_code == "HAMMER"
        assert trigger.intent == intent
        assert trigger.status == "processed"

    def test_pattern_trigger_with_auto_validate(self):
        """
        Test POST /api/pattern-triggers/ with auto_validate=true:
        - Creates TradingIntent
        - Auto-validates intent
        - Returns validation_result in response
        """
        # Setup
        client = Client.objects.create(
            name="Test Client",
            email="test@example.com",
        )

        user = User.objects.create_user(
            username="testuser2",
            email="test2@example.com",
            password="testpass123",
        )
        client.users.add(user)

        symbol = Symbol.objects.create(
            name="BTCUSDT",
            base_asset="BTC",
            quote_asset="USDT",
            client=client,
        )

        strategy = Strategy.objects.create(
            name="Test Strategy",
            client=client,
        )

        # Prepare request
        factory = RequestFactory()
        payload = {
            "pattern_code": "HAMMER",
            "pattern_event_id": "test_auto_validate_evt_001",
            "symbol": symbol.id,
            "strategy": strategy.id,
            "side": "BUY",
            "entry_price": "95000.00",
            "stop_price": "93500.00",
            "capital": "100.00",
            "auto_validate": True,  # Enable auto-validation
        }

        request = factory.post(
            "/api/pattern-triggers/",
            data=json.dumps(payload),
            content_type="application/json",
        )
        force_authenticate(request, user=user)

        # Execute
        response = pattern_trigger(request)

        # Assert: Response includes validation_result
        assert response.status_code == 201
        assert response.data.get("status") == "PROCESSED"
        assert "validation_result" in response.data, "auto_validate=true should return validation_result"

        # Assert: TradingIntent is VALIDATED
        intent_id = response.data["intent_id"]
        intent = TradingIntent.objects.get(intent_id=intent_id)
        assert intent.status == "VALIDATED"
        assert intent.validated_at is not None
        assert intent.validation_result is not None

        # Verify validation_result structure
        validation_result = response.data["validation_result"]
        assert "status" in validation_result
        assert "issues" in validation_result


@pytest.mark.django_db
class TestPatternTriggerIdempotency:
    """Test idempotency protection (duplicate pattern_event_id)."""

    def test_duplicate_pattern_event_id_returns_already_processed(self):
        """
        Test that duplicate pattern_event_id returns ALREADY_PROCESSED:
        - First request creates intent and returns 201
        - Second request with same pattern_event_id returns 200 with ALREADY_PROCESSED
        - No duplicate intents created
        """
        # Setup
        client = Client.objects.create(
            name="Test Client",
            email="test@example.com",
        )

        user = User.objects.create_user(
            username="testuser3",
            email="test3@example.com",
            password="testpass123",
        )
        client.users.add(user)

        symbol = Symbol.objects.create(
            name="BTCUSDT",
            base_asset="BTC",
            quote_asset="USDT",
            client=client,
        )

        strategy = Strategy.objects.create(
            name="Test Strategy",
            client=client,
        )

        factory = RequestFactory()
        payload = {
            "pattern_code": "HAMMER",
            "pattern_event_id": "test_idempotency_evt_001",  # Same event ID
            "symbol": symbol.id,
            "strategy": strategy.id,
            "side": "BUY",
            "entry_price": "95000.00",
            "stop_price": "93500.00",
            "capital": "100.00",
        }

        # FIRST REQUEST
        request1 = factory.post(
            "/api/pattern-triggers/",
            data=json.dumps(payload),
            content_type="application/json",
        )
        force_authenticate(request1, user=user)

        response1 = pattern_trigger(request1)

        # Assert first request success
        assert response1.status_code == 201
        assert response1.data.get("status") == "PROCESSED"
        first_intent_id = response1.data["intent_id"]

        # SECOND REQUEST (same payload, same pattern_event_id)
        request2 = factory.post(
            "/api/pattern-triggers/",
            data=json.dumps(payload),
            content_type="application/json",
        )
        force_authenticate(request2, user=user)

        response2 = pattern_trigger(request2)

        # Assert: Second request returns ALREADY_PROCESSED
        assert response2.status_code == 200, f"Expected 200, got {response2.status_code}: {response2.data}"
        assert response2.data.get("status") == "ALREADY_PROCESSED"
        assert response2.data.get("intent_id") == first_intent_id
        assert "already processed" in response2.data.get("message", "").lower()

        # Assert: Only ONE intent was created
        intent_count = TradingIntent.objects.filter(
            client=client,
            pattern_event_id="test_idempotency_evt_001"
        ).count()
        assert intent_count == 1, "Only one intent should be created (idempotency)"

        # Assert: Only ONE pattern trigger record exists
        trigger_count = PatternTrigger.objects.filter(
            client=client,
            pattern_event_id="test_idempotency_evt_001"
        ).count()
        assert trigger_count == 1, "Only one pattern trigger should exist (idempotency)"


@pytest.mark.django_db
class TestPatternTriggerLiveExecutionBlock:
    """Test that LIVE auto-execution is blocked (Phase 5 MVP constraint)."""

    def test_auto_execute_with_live_mode_returns_400(self):
        """
        Test that auto_execute=true with execution_mode=live is blocked:
        - MVP only allows dry-run auto-execution
        - LIVE execution requires manual user confirmation
        - Endpoint returns 400 Bad Request
        """
        # Setup
        client = Client.objects.create(
            name="Test Client",
            email="test@example.com",
        )

        user = User.objects.create_user(
            username="testuser4",
            email="test4@example.com",
            password="testpass123",
        )
        client.users.add(user)

        symbol = Symbol.objects.create(
            name="BTCUSDT",
            base_asset="BTC",
            quote_asset="USDT",
            client=client,
        )

        strategy = Strategy.objects.create(
            name="Test Strategy",
            client=client,
        )

        factory = RequestFactory()
        payload = {
            "pattern_code": "HAMMER",
            "pattern_event_id": "test_live_block_evt_001",
            "symbol": symbol.id,
            "strategy": strategy.id,
            "side": "BUY",
            "entry_price": "95000.00",
            "stop_price": "93500.00",
            "capital": "100.00",
            "auto_execute": True,  # Request auto-execution
            "execution_mode": "live",  # LIVE mode (should be blocked)
        }

        request = factory.post(
            "/api/pattern-triggers/",
            data=json.dumps(payload),
            content_type="application/json",
        )
        force_authenticate(request, user=user)

        # Execute
        response = pattern_trigger(request)

        # Assert: Request is rejected with 400
        assert response.status_code == 400, f"Expected 400 (blocked), got {response.status_code}: {response.data}"

        # Error may be in different fields depending on DRF serializer
        error_message = ""
        if "error" in response.data:
            error_message = response.data.get("error", "")
        elif "message" in response.data:
            error_message = response.data.get("message", "")
        elif "non_field_errors" in response.data:
            # DRF serializer validation errors
            errors = response.data.get("non_field_errors", [])
            error_message = str(errors[0]) if errors else ""

        assert error_message, "Error message should be present in response"
        assert "live" in error_message.lower() or "auto" in error_message.lower(), \
            f"Error message should mention LIVE or auto-execution blocking. Got: {error_message}"

        # Assert: No intent was created (blocked at validation)
        intent_count = TradingIntent.objects.filter(
            client=client,
            pattern_event_id="test_live_block_evt_001"
        ).count()
        assert intent_count == 0, "No intent should be created when LIVE auto-execution is blocked"

    def test_auto_execute_with_dry_run_is_allowed(self):
        """
        Test that auto_execute=true with execution_mode=dry-run is allowed:
        - MVP allows dry-run auto-execution
        - Creates intent and marks as VALIDATED
        - Does NOT execute actual order (dry-run only)
        """
        # Setup
        client = Client.objects.create(
            name="Test Client",
            email="test@example.com",
        )

        user = User.objects.create_user(
            username="testuser5",
            email="test5@example.com",
            password="testpass123",
        )
        client.users.add(user)

        symbol = Symbol.objects.create(
            name="BTCUSDT",
            base_asset="BTC",
            quote_asset="USDT",
            client=client,
        )

        strategy = Strategy.objects.create(
            name="Test Strategy",
            client=client,
        )

        factory = RequestFactory()
        payload = {
            "pattern_code": "HAMMER",
            "pattern_event_id": "test_dry_run_allowed_evt_001",
            "symbol": symbol.id,
            "strategy": strategy.id,
            "side": "BUY",
            "entry_price": "95000.00",
            "stop_price": "93500.00",
            "capital": "100.00",
            "auto_execute": True,  # Request auto-execution
            "execution_mode": "dry-run",  # DRY-RUN mode (should be allowed)
            "auto_validate": True,  # Also auto-validate
        }

        request = factory.post(
            "/api/pattern-triggers/",
            data=json.dumps(payload),
            content_type="application/json",
        )
        force_authenticate(request, user=user)

        # Execute
        response = pattern_trigger(request)

        # Assert: Request succeeds
        assert response.status_code == 201, f"Expected 201, got {response.status_code}: {response.data}"
        assert response.data.get("status") == "PROCESSED"

        # Assert: Intent was created and validated
        intent_id = response.data["intent_id"]
        intent = TradingIntent.objects.get(intent_id=intent_id)
        assert intent.status == "VALIDATED"  # Validated but NOT executed

        # Note: In MVP, even with auto_execute=true in dry-run mode,
        # we only validate (not execute). User must manually execute from UI.


@pytest.mark.django_db
class TestDecimalQuantizationRegression:
    """Test decimal precision clamping (regression test for Phase 5 fix)."""

    def test_decimal_precision_is_clamped_to_model_constraints(self):
        """
        Regression test: Ensure decimal precision is clamped to model constraints.

        Background: TradingIntent.risk_percent has max_digits=5, decimal_places=2.
        If calculated risk_percent has more precision (e.g., 1.5789473684210527),
        it must be clamped before saving to avoid DataError.

        This test verifies the fix in trading_intent_views.py:497
        """
        # Setup
        client = Client.objects.create(
            name="Test Client",
            email="test@example.com",
        )

        user = User.objects.create_user(
            username="testuser6",
            email="test6@example.com",
            password="testpass123",
        )
        client.users.add(user)

        symbol = Symbol.objects.create(
            name="BTCUSDT",
            base_asset="BTC",
            quote_asset="USDT",
            client=client,
        )

        strategy = Strategy.objects.create(
            name="Test Strategy",
            client=client,
        )

        factory = RequestFactory()

        # Use values that will produce high-precision risk_percent
        # risk_percent = (risk_amount / capital) * 100
        # Example: (1.5 / 95) * 100 = 1.5789473684210527
        payload = {
            "pattern_code": "HAMMER",
            "pattern_event_id": "test_decimal_precision_evt_001",
            "symbol": symbol.id,
            "strategy": strategy.id,
            "side": "BUY",
            "entry_price": "95000.00",
            "stop_price": "93500.00",  # Stop distance = 1500
            "capital": "95.00",  # Capital chosen to produce high-precision percentage
        }

        request = factory.post(
            "/api/pattern-triggers/",
            data=json.dumps(payload),
            content_type="application/json",
        )
        force_authenticate(request, user=user)

        # Execute - should NOT raise DataError about decimal precision
        response = pattern_trigger(request)

        # Assert: Request succeeds (no DataError)
        assert response.status_code == 201, f"Expected 201, got {response.status_code}: {response.data}"

        # Assert: Intent was created successfully
        intent_id = response.data["intent_id"]
        intent = TradingIntent.objects.get(intent_id=intent_id)

        # Assert: risk_percent is clamped to 2 decimal places
        # Original: 1.5789473684210527
        # Clamped: 1.58 (max_digits=5, decimal_places=2)
        assert intent.risk_percent is not None
        risk_percent_str = str(intent.risk_percent)
        decimal_places = len(risk_percent_str.split('.')[-1]) if '.' in risk_percent_str else 0
        assert decimal_places <= 2, f"risk_percent should have max 2 decimal places, got: {intent.risk_percent}"

        # Assert: risk_percent value is reasonable (should be around 1.58%)
        assert intent.risk_percent < Decimal("10.00"), "risk_percent should be reasonable"
        assert intent.risk_percent > Decimal("0.00"), "risk_percent should be positive"

    def test_high_precision_entry_and_stop_prices(self):
        """
        Test that high-precision entry/stop prices are handled correctly.

        TradingIntent model uses:
        - entry_price: max_digits=18, decimal_places=8
        - stop_price: max_digits=18, decimal_places=8
        - quantity: max_digits=18, decimal_places=8

        This test ensures these fields don't cause precision errors.
        """
        # Setup
        client = Client.objects.create(
            name="Test Client",
            email="test@example.com",
        )

        user = User.objects.create_user(
            username="testuser7",
            email="test7@example.com",
            password="testpass123",
        )
        client.users.add(user)

        symbol = Symbol.objects.create(
            name="BTCUSDT",
            base_asset="BTC",
            quote_asset="USDT",
            client=client,
        )

        strategy = Strategy.objects.create(
            name="Test Strategy",
            client=client,
        )

        factory = RequestFactory()

        # Use high-precision prices (up to 8 decimal places)
        payload = {
            "pattern_code": "HAMMER",
            "pattern_event_id": "test_high_precision_prices_evt_001",
            "symbol": symbol.id,
            "strategy": strategy.id,
            "side": "BUY",
            "entry_price": "95123.45678901",  # 8 decimal places
            "stop_price": "93500.12345678",   # 8 decimal places
            "capital": "100.00",
        }

        request = factory.post(
            "/api/pattern-triggers/",
            data=json.dumps(payload),
            content_type="application/json",
        )
        force_authenticate(request, user=user)

        # Execute - should handle precision correctly
        response = pattern_trigger(request)

        # Assert: Request succeeds
        assert response.status_code == 201, f"Expected 201, got {response.status_code}: {response.data}"

        # Assert: Intent was created with clamped precision
        intent_id = response.data["intent_id"]
        intent = TradingIntent.objects.get(intent_id=intent_id)

        # Assert: Prices are clamped to 8 decimal places
        entry_price_str = str(intent.entry_price)
        stop_price_str = str(intent.stop_price)

        entry_decimals = len(entry_price_str.split('.')[-1]) if '.' in entry_price_str else 0
        stop_decimals = len(stop_price_str.split('.')[-1]) if '.' in stop_price_str else 0

        assert entry_decimals <= 8, f"entry_price should have max 8 decimal places, got: {intent.entry_price}"
        assert stop_decimals <= 8, f"stop_price should have max 8 decimal places, got: {intent.stop_price}"
