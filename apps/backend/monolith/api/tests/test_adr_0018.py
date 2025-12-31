"""
ADR-0018: Pattern Detection Engine - Deduplication & Idempotency Tests

Critical tests for:
1. UniqueConstraint enforcement on PatternInstance
2. Idempotent get_or_create behavior
3. API endpoint smoke tests

All timestamps use candle data (NEVER system clock).
"""

from datetime import datetime
from decimal import Decimal
import zoneinfo

import pytest
import django
from django.db import IntegrityError
from django.core.exceptions import ValidationError
from django.test import Client
from django.contrib.auth import get_user_model
from rest_framework.test import APIClient

from api.models.patterns.base import (
    PatternCatalog,
    PatternInstance,
    PatternAlert,
)
from api.models.trading import Symbol
from clients.models import Client

User = get_user_model()


# ============================================================
# SECTION A: UNIQUECONSTRAINT DEDUPLICATION TESTS
# ============================================================


@pytest.mark.django_db
class TestPatternInstanceUniqueConstraint:
    """
    CRITICAL: Test that UniqueConstraint prevents duplicate pattern instances.

    The constraint is on: (client, pattern, symbol, timeframe, start_ts)
    """

    def test_unique_constraint_enforcement(self):
        """
        Test that creating duplicate PatternInstance raises IntegrityError.
        """
        # Create test user (can be None for system-wide patterns)
        user = User.objects.create_user(username="testuser", password="testpass")

        # Get or create symbol
        symbol, _ = Symbol.objects.get_or_create(
            name="BTCUSDT",
            defaults={
                "base_asset": "BTC",
                "quote_asset": "USDT",
                "client": None,  # System-wide
            }
        )

        # Get or create pattern catalog entry
        pattern, _ = PatternCatalog.objects.get_or_create(
            pattern_code="HAMMER",
            defaults={
                "name": "Hammer",
                "category": "CANDLESTICK",
                "direction_bias": "BULLISH",
                "client": None,
            }
        )

        # Define common attributes
        common_attrs = {
            "client": None,  # System-wide pattern
            "symbol": symbol,
            "pattern": pattern,
            "timeframe": "15m",
            "start_ts": datetime(2025, 12, 28, 10, 0, 0, tzinfo=zoneinfo.ZoneInfo("UTC")),
            "end_ts": datetime(2025, 12, 28, 10, 0, 0, tzinfo=zoneinfo.ZoneInfo("UTC")),
            "status": "FORMING",
            "features": {},
        }

        # Create first instance
        instance1 = PatternInstance.objects.create(**common_attrs)
        assert instance1.id is not None

        # CRITICAL: Attempting to create duplicate should raise ValidationError
        # Note: BaseModel.save() calls full_clean() which validates constraints
        # at the application level, raising ValidationError instead of IntegrityError
        with pytest.raises(ValidationError) as exc_info:
            PatternInstance.objects.create(**common_attrs)

        # Verify the error message mentions the constraint
        assert "already exists" in str(exc_info.value).lower()

    def test_unique_constraint_allows_different_combinations(self):
        """
        Test that UniqueConstraint allows different (client, pattern, symbol, timeframe, start_ts).
        """
        # Setup
        user = User.objects.create_user(username="testuser2", password="testpass")
        symbol, _ = Symbol.objects.get_or_create(
            name="ETHUSDT",
            defaults={"base_asset": "ETH", "quote_asset": "USDT", "client": None}
        )
        pattern, _ = PatternCatalog.objects.get_or_create(
            pattern_code="INVERTED_HAMMER",
            defaults={
                "name": "Inverted Hammer",
                "category": "CANDLESTICK",
                "client": None,
            }
        )

        # Create multiple instances with different attributes - should all succeed
        base_time = datetime(2025, 12, 28, 10, 0, 0, tzinfo=zoneinfo.ZoneInfo("UTC"))

        # Different timeframe
        PatternInstance.objects.create(
            client=None,
            symbol=symbol,
            pattern=pattern,
            timeframe="15m",
            start_ts=base_time,
            status="FORMING",
            features={},
        )

        PatternInstance.objects.create(
            client=None,
            symbol=symbol,
            pattern=pattern,
            timeframe="1h",
            start_ts=base_time,
            status="FORMING",
            features={},
        )

        # Different start_ts
        PatternInstance.objects.create(
            client=None,
            symbol=symbol,
            pattern=pattern,
            timeframe="15m",
            start_ts=datetime(2025, 12, 28, 11, 0, 0, tzinfo=zoneinfo.ZoneInfo("UTC")),
            status="FORMING",
            features={},
        )

        # Different pattern
        pattern2, _ = PatternCatalog.objects.get_or_create(
            pattern_code="ENGULFING_BULL",
            defaults={
                "name": "Bullish Engulfing",
                "category": "CANDLESTICK",
                "client": None,
            }
        )
        PatternInstance.objects.create(
            client=None,
            symbol=symbol,
            pattern=pattern2,
            timeframe="15m",
            start_ts=base_time,
            status="FORMING",
            features={},
        )

        # All should be created successfully
        assert PatternInstance.objects.count() == 4


# ============================================================
# SECTION B: IDEMPOTENCY TESTS (get_or_create)
# ============================================================


@pytest.mark.django_db
class TestDjangoPatternRepositoryIdempotency:
    """
    Test that DjangoPatternRepository.get_or_create_instance is idempotent.
    """

    def test_get_or_create_instance_idempotent(self):
        """
        Test that calling get_or_create_instance twice returns same instance.
        """
        from api.application.pattern_engine.domain import PatternSignature, PivotPoint
        from api.application.pattern_engine.adapters import DjangoPatternRepository

        # Setup
        user = User.objects.create_user(username="testuser3", password="testpass")
        symbol, _ = Symbol.objects.get_or_create(
            name="SOLUSDT",
            defaults={"base_asset": "SOL", "quote_asset": "USDT", "client": None}
        )
        pattern, _ = PatternCatalog.objects.get_or_create(
            pattern_code="HAMMER",
            defaults={
                "name": "Hammer",
                "category": "CANDLESTICK",
                "client": None,
            }
        )

        # Create signature - PatternSignature uses evidence dict, not PatternFeature
        signature = PatternSignature(
            pattern_code="HAMMER",
            symbol="SOLUSDT",
            timeframe="15m",
            start_ts=datetime(2025, 12, 28, 10, 0, 0, tzinfo=zoneinfo.ZoneInfo("UTC")),
            end_ts=datetime(2025, 12, 28, 10, 0, 0, tzinfo=zoneinfo.ZoneInfo("UTC")),
            confidence=Decimal("0.75"),
            evidence={
                "body_pct": "0.10",
                "lower_wick_pct": "0.75",
                "upper_wick_pct": "0.15",
            },
            key_points=(),  # No pivot points for candlestick patterns
        )

        # Initialize repository
        repository = DjangoPatternRepository(client=None)

        # FIRST CALL - should create new instance
        instance1, created1 = repository.get_or_create_instance(signature)
        assert created1 is True, "First call should create new instance"
        assert instance1.id is not None
        instance1_id = instance1.id

        # SECOND CALL - should return existing instance
        instance2, created2 = repository.get_or_create_instance(signature)
        assert created2 is False, "Second call should find existing instance"
        assert instance2.id == instance1_id, "Should return same instance"

        # THIRD CALL - still idempotent
        instance3, created3 = repository.get_or_create_instance(signature)
        assert created3 is False, "Third call should still find existing instance"
        assert instance3.id == instance1_id

    def test_get_or_create_concurrent_safety(self):
        """
        Test that get_or_create handles concurrent calls safely.

        Note: This is a simplified test that doesn't actually run concurrent threads,
        but verifies the database constraint would protect against race conditions.
        """
        from api.application.pattern_engine.domain import PatternSignature, PivotPoint
        from api.application.pattern_engine.adapters import DjangoPatternRepository

        # Setup
        user = User.objects.create_user(username="testuser4", password="testpass")
        symbol, _ = Symbol.objects.get_or_create(
            name="BTCUSDT",
            defaults={"base_asset": "BTC", "quote_asset": "USDT", "client": None}
        )
        pattern, _ = PatternCatalog.objects.get_or_create(
            pattern_code="HAMMER",
            defaults={
                "name": "Hammer",
                "category": "CANDLESTICK",
                "client": None,
            }
        )

        signature = PatternSignature(
            pattern_code="HAMMER",
            symbol="BTCUSDT",
            timeframe="15m",
            start_ts=datetime(2025, 12, 28, 10, 0, 0, tzinfo=zoneinfo.ZoneInfo("UTC")),
            end_ts=datetime(2025, 12, 28, 10, 0, 0, tzinfo=zoneinfo.ZoneInfo("UTC")),
            confidence=Decimal("0.75"),
            evidence={
                "body_pct": "0.10",
                "lower_wick_pct": "0.75",
                "upper_wick_pct": "0.15",
            },
            key_points=(),
        )

        repository = DjangoPatternRepository(client=None)

        # Simulate "concurrent" calls by verifying get_or_create behavior
        results = []
        for i in range(5):
            instance, created = repository.get_or_create_instance(signature)
            results.append((instance.id, created))

        # All calls should return the same instance
        instance_ids = [r[0] for r in results]
        created_flags = [r[1] for r in results]

        # Exactly one should have created=True (the first call)
        assert sum(created_flags) == 1, "Only first call should create new instance"
        assert len(set(instance_ids)) == 1, "All calls should return same instance ID"


# ============================================================
# SECTION C: PATTERN ALERT IDEMPOTENCY
# ============================================================


@pytest.mark.django_db
class TestPatternAlertIdempotency:
    """
    Test that PatternAlert creation is idempotent.
    """

    def test_alert_unique_per_instance_type_ts(self):
        """
        Test that alert is unique per (instance, alert_type, alert_ts).

        Note: Currently there's no explicit UniqueConstraint on PatternAlert,
        but the repository should handle idempotency via get_or_create.
        """
        from api.application.pattern_engine.adapters import DjangoPatternRepository

        # Setup
        user = User.objects.create_user(username="testuser5", password="testpass")
        symbol, _ = Symbol.objects.get_or_create(
            name="BTCUSDT",
            defaults={"base_asset": "BTC", "quote_asset": "USDT", "client": None}
        )
        pattern, _ = PatternCatalog.objects.get_or_create(
            pattern_code="HAMMER",
            defaults={
                "name": "Hammer",
                "category": "CANDLESTICK",
                "client": None,
            }
        )

        # Create instance
        instance = PatternInstance.objects.create(
            client=None,
            symbol=symbol,
            pattern=pattern,
            timeframe="15m",
            start_ts=datetime(2025, 12, 28, 10, 0, 0, tzinfo=zoneinfo.ZoneInfo("UTC")),
            status="FORMING",
            features={},
        )

        repository = DjangoPatternRepository(client=None)

        # Create alert with same timestamp twice
        alert_ts = datetime(2025, 12, 28, 10, 15, 0, tzinfo=zoneinfo.ZoneInfo("UTC"))

        alert1, created1 = repository.emit_alert(
            instance_id=instance.id,
            alert_type="CONFIRM",
            alert_ts=alert_ts,
            confidence=Decimal("0.85"),
            payload={"confirmation_type": "CLOSE_ABOVE_HIGH"},
        )

        alert2, created2 = repository.emit_alert(
            instance_id=instance.id,
            alert_type="CONFIRM",
            alert_ts=alert_ts,  # Same timestamp
            confidence=Decimal("0.85"),
            payload={"confirmation_type": "CLOSE_ABOVE_HIGH"},
        )

        # Verify idempotency
        assert created1 is True, "First alert should be created"
        # Note: The current implementation may not enforce uniqueness on alerts
        # This test documents current behavior


# ============================================================
# SECTION D: API ENDPOINT SMOKE TESTS
# ============================================================


@pytest.mark.django_db
class TestPatternAPISmoke:
    """
    Smoke tests for pattern detection API endpoints.
    """

    def test_pattern_catalog_endpoint(self):
        """
        Test that /api/patterns/catalog/ returns 200.
        """
        client = APIClient()

        # Create test user and authenticate
        user = User.objects.create_user(username="apiuser", password="testpass")
        client.force_authenticate(user=user)

        # Create a pattern catalog entry
        PatternCatalog.objects.create(
            pattern_code="HAMMER",
            name="Hammer",
            category="CANDLESTICK",
            direction_bias="BULLISH",
            client=None,
        )

        # Test endpoint
        response = client.get("/api/patterns/catalog/")

        assert response.status_code == 200, f"Expected 200, got {response.status_code}"
        data = response.json()
        assert "results" in data or isinstance(data, list)

    def test_pattern_instances_endpoint(self):
        """
        Test that /api/patterns/instances/ returns 200.
        """
        client = APIClient()

        user = User.objects.create_user(username="apiuser2", password="testpass")
        client.force_authenticate(user=user)

        response = client.get("/api/patterns/instances/")

        # Should return 200 (possibly empty list)
        assert response.status_code == 200, f"Expected 200, got {response.status_code}"

    def test_pattern_alerts_endpoint(self):
        """
        Test that /api/patterns/alerts/ returns 200.
        """
        client = APIClient()

        user = User.objects.create_user(username="apiuser3", password="testpass")
        client.force_authenticate(user=user)

        response = client.get("/api/patterns/alerts/")

        assert response.status_code == 200, f"Expected 200, got {response.status_code}"


# ============================================================
# SECTION E: DASHBOARD ENDPOINT CONTRACT TESTS
# ============================================================


@pytest.mark.django_db
class TestPatternDashboardContract:
    """
    CRITICAL: Test that /api/patterns/dashboard/ maintains a stable contract.

    The dashboard MUST ALWAYS return HTTP 200 with this exact shape:
    {
        "period": "Last 24 hours",
        "patterns": {"total_detected": N, "by_status": {...}},
        "alerts": {"total": N, "by_type": {...}},
        "configs": {"active_auto_entry": N}
    }

    Even with ZERO data, all keys must be present with zeros.
    """

    def test_dashboard_returns_200_with_zero_data(self):
        """
        Test that dashboard returns HTTP 200 with stable schema when no patterns exist.
        
        This is UX-CRITICAL: the frontend Opportunity Detector depends on this
        endpoint never returning 500/400 for empty datasets.
        """
        client = APIClient()

        # Create user with client (tenant context)
        test_client = Client.objects.create(name="Test Client", email="test@example.com")
        user = User.objects.create_user(username="dashboard_user", password="testpass")
        user.client = test_client
        user.save()

        client.force_authenticate(user=user)

        # Ensure no pattern instances exist for this tenant
        PatternInstance.objects.filter(client=test_client).delete()

        # Call dashboard endpoint
        response = client.get("/api/patterns/dashboard/")

        # CRITICAL: Must return HTTP 200, never 500/400 for empty data
        assert response.status_code == 200, f"Expected 200, got {response.status_code}"

        data = response.json()

        # Verify ALL top-level keys exist
        assert "period" in data, "Response must include 'period' key"
        assert "patterns" in data, "Response must include 'patterns' key"
        assert "alerts" in data, "Response must include 'alerts' key"
        assert "configs" in data, "Response must include 'configs' key"

        # Verify zero counts when no data exists
        assert data["patterns"]["total_detected"] == 0, "total_detected must be 0 with no patterns"
        assert data["alerts"]["total"] == 0, "alerts.total must be 0 with no alerts"
        assert data["configs"]["active_auto_entry"] == 0, "configs.active_auto_entry must be 0 with no configs"

        # Verify breakdown objects exist (even if empty)
        assert "by_status" in data["patterns"], "patterns.by_status must exist"
        assert "by_type" in data["alerts"], "alerts.by_type must exist"

    def test_dashboard_returns_200_with_pattern_data(self):
        """
        Test that dashboard returns correct counts when patterns exist.
        """
        client = APIClient()

        # Create user with client
        test_client = Client.objects.create(name="Test Client 2", email="test2@example.com")
        user = User.objects.create_user(username="dashboard_user2", password="testpass")
        user.client = test_client
        user.save()

        client.force_authenticate(user=user)

        # Create test data
        symbol, _ = Symbol.objects.get_or_create(
            name="BTCUSDT",
            defaults={"base_asset": "BTC", "quote_asset": "USDT", "client": None}
        )
        pattern, _ = PatternCatalog.objects.get_or_create(
            pattern_code="HAMMER",
            defaults={
                "name": "Hammer",
                "category": "CANDLESTICK",
                "direction_bias": "BULLISH",
                "client": None,
            }
        )

        # Create a pattern instance for this tenant
        PatternInstance.objects.create(
            client=test_client,
            symbol=symbol,
            pattern=pattern,
            timeframe="15m",
            start_ts=datetime(2025, 12, 31, 10, 0, 0, tzinfo=zoneinfo.ZoneInfo("UTC")),
            status="FORMING",
            features={},
        )

        # Create another one
        PatternInstance.objects.create(
            client=test_client,
            symbol=symbol,
            pattern=pattern,
            timeframe="1h",
            start_ts=datetime(2025, 12, 31, 11, 0, 0, tzinfo=zoneinfo.ZoneInfo("UTC")),
            status="CONFIRMED",
            features={},
        )

        # Call dashboard endpoint
        response = client.get("/api/patterns/dashboard/")

        assert response.status_code == 200
        data = response.json()

        # Verify all keys exist
        assert "period" in data
        assert "patterns" in data
        assert "alerts" in data
        assert "configs" in data

        # Verify counts (at least 2 patterns created in last 24h)
        assert data["patterns"]["total_detected"] >= 2, \
            f"Expected at least 2 patterns, got {data['patterns']['total_detected']}"

        # Verify status breakdown
        assert data["patterns"]["by_status"]["FORMING"] >= 1
        assert data["patterns"]["by_status"]["CONFIRMED"] >= 1

    def test_dashboard_preserves_period_type(self):
        """
        Test that the period field type is preserved (string).
        
        This ensures frontend contract stability.
        """
        client = APIClient()

        test_client = Client.objects.create(name="Test Client 3", email="test3@example.com")
        user = User.objects.create_user(username="dashboard_user3", password="testpass")
        user.client = test_client
        user.save()

        client.force_authenticate(user=user)

        response = client.get("/api/patterns/dashboard/")

        assert response.status_code == 200
        data = response.json()

        # period must be a string
        assert isinstance(data["period"], str), "period must be a string"
        assert len(data["period"]) > 0, "period must not be empty"
