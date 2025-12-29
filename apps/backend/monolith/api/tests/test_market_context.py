"""
Unit tests for Market Research & Context Engine (Core 2).

Tests domain entities, adapters, use cases, and integration with Django models.
Follows established testing patterns from the codebase.

Architecture Reference: ADR-0017 (Market Research & Context Engine)
"""

import pytest
from decimal import Decimal
from datetime import datetime, timedelta
from django.utils import timezone

# Test imports (domain entities - NO Django dependencies)
from api.application.market_context.domain import MetricPoint, validate_metric_name, validate_source

# Test imports (adapters and use cases)
from api.application.market_context import (
    MockDerivativesAdapter,
    InMemoryMetricRepository,
    CollectDerivativesMetrics,
    GetLatestMetrics,
)


# ==========================================
# DOMAIN ENTITY TESTS
# ==========================================


class TestMetricPointDomain:
    """Test MetricPoint domain entity (framework-agnostic)."""

    def test_metric_point_creation(self):
        """Test creating a valid MetricPoint."""
        metric = MetricPoint(
            timestamp=timezone.now(),
            symbol="BTCUSDT",
            metric_name="funding_rate",
            value=Decimal("0.0001"),
            source="binance_futures",
            tags={"contract": "perpetual"},
        )

        assert metric.symbol == "BTCUSDT"
        assert metric.metric_name == "funding_rate"
        assert metric.value == Decimal("0.0001")
        assert metric.source == "binance_futures"
        assert metric.is_funding_rate is True
        assert metric.is_open_interest is False

    def test_metric_point_immutability(self):
        """Test that MetricPoint is immutable (frozen dataclass)."""
        metric = MetricPoint(
            timestamp=timezone.now(),
            symbol="BTCUSDT",
            metric_name="funding_rate",
            value=Decimal("0.0001"),
            source="binance_futures",
        )

        with pytest.raises(Exception):  # FrozenInstanceError
            metric.value = Decimal("0.0002")

    def test_metric_point_validation_empty_symbol(self):
        """Test validation rejects empty symbol."""
        with pytest.raises(ValueError, match="symbol must be non-empty string"):
            MetricPoint(
                timestamp=timezone.now(),
                symbol="",
                metric_name="funding_rate",
                value=Decimal("0.0001"),
                source="binance_futures",
            )

    def test_metric_point_validation_invalid_value_type(self):
        """Test validation rejects non-Decimal value."""
        with pytest.raises(TypeError, match="value must be Decimal"):
            MetricPoint(
                timestamp=timezone.now(),
                symbol="BTCUSDT",
                metric_name="funding_rate",
                value=0.0001,  # float instead of Decimal
                source="binance_futures",
            )

    def test_metric_point_string_representation(self):
        """Test __str__ method."""
        metric = MetricPoint(
            timestamp=datetime(2025, 12, 28, 10, 0, 0),
            symbol="BTCUSDT",
            metric_name="funding_rate",
            value=Decimal("0.0001"),
            source="binance_futures",
        )

        str_repr = str(metric)
        assert "BTCUSDT" in str_repr
        assert "funding_rate" in str_repr
        assert "0.0001" in str_repr
        assert "binance_futures" in str_repr

    def test_metric_point_to_dict(self):
        """Test serialization to dictionary."""
        timestamp = timezone.now()
        metric = MetricPoint(
            timestamp=timestamp,
            symbol="BTCUSDT",
            metric_name="funding_rate",
            value=Decimal("0.0001"),
            source="binance_futures",
            tags={"contract": "perpetual"},
        )

        data = metric.to_dict()

        assert data["symbol"] == "BTCUSDT"
        assert data["metric_name"] == "funding_rate"
        assert data["value"] == "0.0001"  # Decimal → string
        assert data["source"] == "binance_futures"
        assert data["tags"] == {"contract": "perpetual"}

    def test_metric_point_from_dict(self):
        """Test deserialization from dictionary."""
        data = {
            "timestamp": "2025-12-28T10:00:00Z",
            "symbol": "BTCUSDT",
            "metric_name": "funding_rate",
            "value": "0.0001",
            "source": "binance_futures",
            "tags": {"contract": "perpetual"},
        }

        metric = MetricPoint.from_dict(data)

        assert metric.symbol == "BTCUSDT"
        assert metric.metric_name == "funding_rate"
        assert metric.value == Decimal("0.0001")
        assert metric.source == "binance_futures"


class TestValidationHelpers:
    """Test validation helper functions."""

    def test_validate_metric_name_known_metrics(self):
        """Test validation accepts known metric names."""
        assert validate_metric_name("funding_rate") is True
        assert validate_metric_name("open_interest") is True
        assert validate_metric_name("mark_price") is True

    def test_validate_metric_name_unknown_metric(self):
        """Test validation rejects unknown metric names."""
        assert validate_metric_name("unknown_metric") is False

    def test_validate_source_known_sources(self):
        """Test validation accepts known sources."""
        assert validate_source("binance_futures") is True
        assert validate_source("binance_spot") is True
        assert validate_source("defillama") is True

    def test_validate_source_unknown_source(self):
        """Test validation rejects unknown sources."""
        assert validate_source("unknown_source") is False


# ==========================================
# ADAPTER TESTS
# ==========================================


class TestMockDerivativesAdapter:
    """Test mock adapter for derivatives data collection."""

    def test_collect_metrics_returns_expected_count(self):
        """Test mock adapter returns expected number of metrics."""
        adapter = MockDerivativesAdapter()
        metrics = adapter.collect_metrics("BTCUSDT")

        assert len(metrics) == 3  # funding_rate, open_interest, mark_price

    def test_collect_metrics_returns_valid_metric_points(self):
        """Test mock adapter returns valid MetricPoint instances."""
        adapter = MockDerivativesAdapter()
        metrics = adapter.collect_metrics("BTCUSDT")

        for metric in metrics:
            assert isinstance(metric, MetricPoint)
            assert metric.symbol == "BTCUSDT"
            assert metric.source == "mock_futures"

    def test_collect_metrics_includes_expected_metric_types(self):
        """Test mock adapter returns expected metric types."""
        adapter = MockDerivativesAdapter()
        metrics = adapter.collect_metrics("BTCUSDT")

        metric_names = {m.metric_name for m in metrics}
        assert "funding_rate" in metric_names
        assert "open_interest" in metric_names
        assert "mark_price" in metric_names


class TestInMemoryMetricRepository:
    """Test in-memory repository for testing."""

    def test_save_metric_stores_metric(self):
        """Test saving a metric stores it in memory."""
        repo = InMemoryMetricRepository(client_id=1)
        metric = MetricPoint(
            timestamp=timezone.now(),
            symbol="BTCUSDT",
            metric_name="funding_rate",
            value=Decimal("0.0001"),
            source="binance_futures",
        )

        saved = repo.save_metric(metric)

        assert saved == metric
        assert len(repo.metrics) == 1

    def test_save_metric_idempotency(self):
        """Test saving duplicate metric is idempotent."""
        repo = InMemoryMetricRepository(client_id=1)
        timestamp = timezone.now()

        metric1 = MetricPoint(
            timestamp=timestamp,
            symbol="BTCUSDT",
            metric_name="funding_rate",
            value=Decimal("0.0001"),
            source="binance_futures",
        )

        # Save same metric twice
        repo.save_metric(metric1)
        repo.save_metric(metric1)

        # Should only be stored once (idempotent)
        assert len(repo.metrics) == 1

    def test_get_latest_metric_returns_most_recent(self):
        """Test get_latest_metric returns newest metric."""
        repo = InMemoryMetricRepository(client_id=1)

        # Save metrics with different timestamps
        old_metric = MetricPoint(
            timestamp=timezone.now() - timedelta(minutes=5),
            symbol="BTCUSDT",
            metric_name="funding_rate",
            value=Decimal("0.0001"),
            source="binance_futures",
        )

        new_metric = MetricPoint(
            timestamp=timezone.now(),
            symbol="BTCUSDT",
            metric_name="funding_rate",
            value=Decimal("0.0002"),
            source="binance_futures",
        )

        repo.save_metric(old_metric)
        repo.save_metric(new_metric)

        latest = repo.get_latest_metric("BTCUSDT", "funding_rate", "binance_futures")

        assert latest == new_metric
        assert latest.value == Decimal("0.0002")

    def test_get_time_series_filters_by_time_range(self):
        """Test get_time_series filters by time range."""
        repo = InMemoryMetricRepository(client_id=1)

        now = timezone.now()
        start_time = now - timedelta(hours=2)
        end_time = now - timedelta(hours=1)

        # Metrics: before range, in range, after range
        before = MetricPoint(
            timestamp=start_time - timedelta(minutes=30),
            symbol="BTCUSDT",
            metric_name="funding_rate",
            value=Decimal("0.0001"),
            source="binance_futures",
        )

        in_range = MetricPoint(
            timestamp=start_time + timedelta(minutes=30),
            symbol="BTCUSDT",
            metric_name="funding_rate",
            value=Decimal("0.0002"),
            source="binance_futures",
        )

        after = MetricPoint(
            timestamp=end_time + timedelta(minutes=30),
            symbol="BTCUSDT",
            metric_name="funding_rate",
            value=Decimal("0.0003"),
            source="binance_futures",
        )

        repo.save_metric(before)
        repo.save_metric(in_range)
        repo.save_metric(after)

        series = repo.get_time_series(
            "BTCUSDT",
            "funding_rate",
            "binance_futures",
            start_time,
            end_time,
        )

        # Should only return metric in range
        assert len(series) == 1
        assert series[0] == in_range


# ==========================================
# USE CASE TESTS
# ==========================================


class TestCollectDerivativesMetrics:
    """Test CollectDerivativesMetrics use case."""

    def test_execute_collects_and_persists_metrics(self):
        """Test use case collects and persists metrics."""
        collector = MockDerivativesAdapter()
        repository = InMemoryMetricRepository(client_id=1)
        use_case = CollectDerivativesMetrics(collector, repository)

        count = use_case.execute("BTCUSDT")

        assert count == 3  # 3 metrics collected
        assert len(repository.metrics) == 3

    def test_execute_validates_symbol(self):
        """Test use case validates symbol parameter."""
        collector = MockDerivativesAdapter()
        repository = InMemoryMetricRepository(client_id=1)
        use_case = CollectDerivativesMetrics(collector, repository)

        with pytest.raises(ValueError, match="symbol must be non-empty string"):
            use_case.execute("")

    def test_execute_handles_empty_collection(self):
        """Test use case handles empty metric collection."""
        # Create collector that returns no metrics
        class EmptyAdapter:
            def collect_metrics(self, symbol: str):
                return []

        collector = EmptyAdapter()
        repository = InMemoryMetricRepository(client_id=1)
        use_case = CollectDerivativesMetrics(collector, repository)

        count = use_case.execute("BTCUSDT")

        assert count == 0
        assert len(repository.metrics) == 0


class TestGetLatestMetrics:
    """Test GetLatestMetrics use case."""

    def test_execute_retrieves_latest_metrics(self):
        """Test use case retrieves latest metrics."""
        # Setup: Save metrics to repository
        repository = InMemoryMetricRepository(client_id=1)

        metrics = [
            MetricPoint(
                timestamp=timezone.now(),
                symbol="BTCUSDT",
                metric_name="funding_rate",
                value=Decimal("0.0001"),
                source="binance_futures",
            ),
            MetricPoint(
                timestamp=timezone.now(),
                symbol="BTCUSDT",
                metric_name="open_interest",
                value=Decimal("12345.67"),
                source="binance_futures",
            ),
        ]

        for metric in metrics:
            repository.save_metric(metric)

        # Execute use case
        use_case = GetLatestMetrics(repository)
        latest = use_case.execute(
            "BTCUSDT",
            "binance_futures",
            metric_names=["funding_rate", "open_interest"],
        )

        assert len(latest) == 2
        assert latest[0].metric_name == "funding_rate"
        assert latest[1].metric_name == "open_interest"

    def test_execute_handles_missing_metrics(self):
        """Test use case handles missing metrics gracefully."""
        repository = InMemoryMetricRepository(client_id=1)
        use_case = GetLatestMetrics(repository)

        # No metrics in repository
        latest = use_case.execute("BTCUSDT", "binance_futures")

        assert len(latest) == 0  # No metrics found, returns empty list


# ==========================================
# INTEGRATION TESTS (Django Models)
# ==========================================


@pytest.mark.django_db
class TestDjangoMetricRepository:
    """Test Django ORM repository (requires database)."""

    def test_save_metric_creates_django_model(self):
        """Test saving metric creates Django model instance."""
        from api.application.market_context import DjangoMetricRepository

        repo = DjangoMetricRepository(client_id=1)
        metric = MetricPoint(
            timestamp=timezone.now(),
            symbol="BTCUSDT",
            metric_name="funding_rate",
            value=Decimal("0.0001"),
            source="binance_futures",
        )

        saved = repo.save_metric(metric)

        assert saved.symbol == "BTCUSDT"
        assert saved.metric_name == "funding_rate"

        # Verify Django model was created
        from api.models import MetricPoint as DjangoMetricPoint

        count = DjangoMetricPoint.objects.filter(
            symbol="BTCUSDT",
            metric_name="funding_rate",
        ).count()

        assert count == 1
