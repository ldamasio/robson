"""
Market Research & Context Engine - Adapters (Implementations)

Concrete implementations of ports for the Market Context core.
These adapters CAN import Django (ORM, settings, etc.).

Adapters:
- BinanceDerivativesAdapter: Fetches derivatives data from Binance Futures API
- DjangoMetricRepository: Persists MetricPoint to PostgreSQL via Django ORM

Architecture Reference: ADR-0017 (Market Research & Context Engine)
"""

from __future__ import annotations
from typing import List, Optional
from datetime import datetime
from decimal import Decimal
import logging

from django.utils import timezone

from .ports import DerivativesMetricCollector, MetricRepository
from .domain import MetricPoint

logger = logging.getLogger(__name__)


# ==========================================
# DATA COLLECTION ADAPTER (Binance Futures)
# ==========================================


class BinanceDerivativesAdapter(DerivativesMetricCollector):
    """
    Adapter for fetching derivatives data from Binance Futures API.

    This adapter:
    1. Wraps DerivativesDataService (service layer)
    2. Calls Binance Futures API endpoints
    3. Normalizes raw API responses to MetricPoint domain entities
    4. Returns list of MetricPoint (funding rate, OI, mark price)

    Design:
    - Service layer returns raw dicts (API responses)
    - Adapter normalizes to domain entities
    - Use case orchestrates collection → repository persistence

    Multi-tenant:
    - Uses system credentials (K8s secrets)
    - Client association happens at repository layer
    """

    def __init__(self, use_testnet: bool = None):
        """
        Initialize Binance Derivatives Adapter.

        Args:
            use_testnet: Override testnet setting. If None, uses settings.BINANCE_USE_TESTNET
        """
        # Lazy import to avoid circular dependencies
        from api.services import DerivativesDataService

        self.service = DerivativesDataService(use_testnet=use_testnet)
        mode = "TESTNET" if self.service.is_production is False else "PRODUCTION"
        logger.info(f"BinanceDerivativesAdapter initialized in {mode} mode")

    def collect_metrics(self, symbol: str) -> List[MetricPoint]:
        """
        Collect all derivatives metrics for a trading pair.

        This method:
        1. Calls service to fetch raw API data
        2. Normalizes responses to MetricPoint domain entities
        3. Returns list of metrics (funding rate, OI, mark price)

        Optimization:
        - Uses get_mark_price() for both mark price AND funding rate (single call)
        - Only 2 API calls total (mark_price + open_interest)

        Args:
            symbol: Trading pair (e.g., "BTCUSDT")

        Returns:
            List of MetricPoint domain entities [funding_rate, open_interest, mark_price]

        Raises:
            Exception: If API calls fail

        Example:
            >>> adapter = BinanceDerivativesAdapter()
            >>> metrics = adapter.collect_metrics("BTCUSDT")
            >>> [m.metric_name for m in metrics]
            ['funding_rate', 'open_interest', 'mark_price', 'index_price']
        """
        try:
            logger.info(f"Collecting derivatives metrics for {symbol}")

            # Collect all metrics via service
            data = self.service.collect_all_metrics(symbol)

            # Normalize to domain entities
            metrics = []
            current_time = timezone.now()

            # Extract mark price data (includes funding rate)
            mark_data = data["mark_price_data"]

            # Metric 1: Funding Rate (from mark price response)
            funding_rate = self._normalize_funding_rate(
                symbol=symbol,
                mark_data=mark_data,
                timestamp=current_time,
            )
            metrics.append(funding_rate)

            # Metric 2: Open Interest
            oi_data = data["open_interest_data"]
            open_interest = self._normalize_open_interest(
                symbol=symbol,
                oi_data=oi_data,
                timestamp=current_time,
            )
            metrics.append(open_interest)

            # Metric 3: Mark Price
            mark_price = self._normalize_mark_price(
                symbol=symbol,
                mark_data=mark_data,
                timestamp=current_time,
            )
            metrics.append(mark_price)

            # Metric 4: Index Price (bonus, already in mark_data)
            if "indexPrice" in mark_data:
                index_price = self._normalize_index_price(
                    symbol=symbol,
                    mark_data=mark_data,
                    timestamp=current_time,
                )
                metrics.append(index_price)

            logger.info(f"Collected {len(metrics)} metrics for {symbol}")
            return metrics

        except Exception as e:
            logger.error(f"Failed to collect metrics for {symbol}: {e}")
            raise

    # ========================================================================
    # Private Normalization Methods
    # ========================================================================

    def _normalize_funding_rate(
        self,
        symbol: str,
        mark_data: dict,
        timestamp: datetime,
    ) -> MetricPoint:
        """
        Normalize funding rate from mark price API response.

        Args:
            symbol: Trading pair
            mark_data: Response from futures_mark_price()
            timestamp: Collection timestamp

        Returns:
            MetricPoint for funding rate
        """
        return MetricPoint(
            timestamp=timestamp,
            symbol=symbol,
            metric_name="funding_rate",
            value=Decimal(mark_data["lastFundingRate"]),
            source="binance_futures",
            tags={
                "contract": "perpetual",
                "next_funding_time": str(mark_data.get("nextFundingTime", "")),
            },
            collection_metadata={
                "api_response_time": str(mark_data.get("time", "")),
                "collector_version": "1.0.0",
            },
        )

    def _normalize_open_interest(
        self,
        symbol: str,
        oi_data: dict,
        timestamp: datetime,
    ) -> MetricPoint:
        """
        Normalize open interest from API response.

        Args:
            symbol: Trading pair
            oi_data: Response from futures_open_interest()
            timestamp: Collection timestamp

        Returns:
            MetricPoint for open interest
        """
        return MetricPoint(
            timestamp=timestamp,
            symbol=symbol,
            metric_name="open_interest",
            value=Decimal(oi_data["openInterest"]),
            source="binance_futures",
            tags={
                "contract": "perpetual",
                "unit": "contracts",
            },
            collection_metadata={
                "api_response_time": str(oi_data.get("time", "")),
                "collector_version": "1.0.0",
            },
        )

    def _normalize_mark_price(
        self,
        symbol: str,
        mark_data: dict,
        timestamp: datetime,
    ) -> MetricPoint:
        """
        Normalize mark price from API response.

        Args:
            symbol: Trading pair
            mark_data: Response from futures_mark_price()
            timestamp: Collection timestamp

        Returns:
            MetricPoint for mark price
        """
        return MetricPoint(
            timestamp=timestamp,
            symbol=symbol,
            metric_name="mark_price",
            value=Decimal(mark_data["markPrice"]),
            source="binance_futures",
            tags={
                "contract": "perpetual",
                "quote_asset": "USDT",
            },
            collection_metadata={
                "api_response_time": str(mark_data.get("time", "")),
                "collector_version": "1.0.0",
            },
        )

    def _normalize_index_price(
        self,
        symbol: str,
        mark_data: dict,
        timestamp: datetime,
    ) -> MetricPoint:
        """
        Normalize index price from API response.

        Args:
            symbol: Trading pair
            mark_data: Response from futures_mark_price()
            timestamp: Collection timestamp

        Returns:
            MetricPoint for index price
        """
        return MetricPoint(
            timestamp=timestamp,
            symbol=symbol,
            metric_name="index_price",
            value=Decimal(mark_data["indexPrice"]),
            source="binance_futures",
            tags={
                "contract": "perpetual",
                "quote_asset": "USDT",
            },
            collection_metadata={
                "api_response_time": str(mark_data.get("time", "")),
                "collector_version": "1.0.0",
            },
        )


# ==========================================
# PERSISTENCE ADAPTER (Django ORM)
# ==========================================


class DjangoMetricRepository(MetricRepository):
    """
    Repository for persisting MetricPoint domain entities to PostgreSQL.

    This adapter:
    1. Maps MetricPoint domain entity → Django MetricPoint model
    2. Implements idempotent upsert (unique constraint prevents duplicates)
    3. Handles multi-tenant isolation (client_id scoped)

    Design:
    - Receives domain entities (NO Django dependencies)
    - Maps to Django models (WITH Django dependencies)
    - Returns domain entities (adapters do the translation)

    Multi-tenant:
    - All operations scoped to client_id (passed in constructor)
    - Unique constraint: (client_id, source, symbol, metric_name, timestamp)
    """

    def __init__(self, client_id: int):
        """
        Initialize Django Metric Repository.

        Args:
            client_id: Client (tenant) ID for multi-tenant isolation
        """
        # Lazy import to avoid circular dependencies
        from api.models import MetricPoint as DjangoMetricPoint

        self._MetricPoint = DjangoMetricPoint
        self.client_id = client_id

        logger.debug(f"DjangoMetricRepository initialized for client_id={client_id}")

    def save_metric(self, metric: MetricPoint) -> MetricPoint:
        """
        Save a single metric point (idempotent upsert).

        Uses get_or_create for idempotency. If a metric with the same
        (client_id, source, symbol, metric_name, timestamp) exists, returns
        the existing one without modification.

        Args:
            metric: MetricPoint domain entity to save

        Returns:
            The saved MetricPoint domain entity

        Raises:
            Exception: If persistence fails
        """
        try:
            # Map domain entity → Django model
            django_metric, created = self._MetricPoint.objects.get_or_create(
                client_id=self.client_id,
                source=metric.source,
                symbol=metric.symbol,
                metric_name=metric.metric_name,
                timestamp=metric.timestamp,
                defaults={
                    "value": metric.value,
                    "tags": metric.tags,
                    "collection_metadata": metric.collection_metadata,
                },
            )

            if created:
                logger.debug(
                    f"Saved new metric: {metric.symbol} | {metric.metric_name}={metric.value}"
                )
            else:
                logger.debug(
                    f"Metric already exists (idempotent): {metric.symbol} | {metric.metric_name}"
                )

            # Map Django model → domain entity
            return self._to_domain(django_metric)

        except Exception as e:
            logger.error(f"Failed to save metric: {e}")
            raise

    def save_metrics_batch(self, metrics: List[MetricPoint]) -> int:
        """
        Save multiple metrics in a single transaction (idempotent).

        Uses bulk_create with ignore_conflicts=True for performance.
        Duplicate metrics (based on unique constraint) are silently ignored.

        Args:
            metrics: List of MetricPoint domain entities to save

        Returns:
            Number of new metrics actually inserted (approximate, see note)

        Note:
            When using ignore_conflicts=True, Django does NOT return the number
            of actually created records. The return value is the length of the
            input list, NOT the number of new records. This is a known Django
            limitation. For exact counts, query the database after insertion.

        Raises:
            Exception: If persistence fails
        """
        try:
            if not metrics:
                logger.debug("No metrics to save (empty list)")
                return 0

            # Map domain entities → Django models
            django_metrics = [
                self._MetricPoint(
                    client_id=self.client_id,
                    source=metric.source,
                    symbol=metric.symbol,
                    metric_name=metric.metric_name,
                    timestamp=metric.timestamp,
                    value=metric.value,
                    tags=metric.tags,
                    collection_metadata=metric.collection_metadata,
                )
                for metric in metrics
            ]

            # Bulk insert (ignore duplicates)
            self._MetricPoint.objects.bulk_create(
                django_metrics,
                ignore_conflicts=True,  # Idempotency via unique constraint
            )

            # Note: ignore_conflicts=True means we don't know exact count of new records
            # Return input length as approximation
            count = len(metrics)
            logger.info(f"Bulk saved {count} metrics (duplicates ignored)")

            return count

        except Exception as e:
            logger.error(f"Failed to bulk save metrics: {e}")
            raise

    def get_latest_metric(
        self,
        symbol: str,
        metric_name: str,
        source: str,
    ) -> Optional[MetricPoint]:
        """
        Get the most recent metric for a specific type.

        Args:
            symbol: Trading pair (e.g., "BTCUSDT")
            metric_name: Metric type (e.g., "funding_rate")
            source: Data source (e.g., "binance_futures")

        Returns:
            MetricPoint if found, None otherwise
        """
        try:
            django_metric = (
                self._MetricPoint.objects.filter(
                    client_id=self.client_id,
                    symbol=symbol,
                    metric_name=metric_name,
                    source=source,
                )
                .order_by("-timestamp")
                .first()
            )

            if django_metric:
                return self._to_domain(django_metric)
            return None

        except Exception as e:
            logger.error(f"Failed to get latest metric: {e}")
            raise

    def get_time_series(
        self,
        symbol: str,
        metric_name: str,
        source: str,
        start_time: datetime,
        end_time: datetime,
    ) -> List[MetricPoint]:
        """
        Get time series data for a metric within a time range.

        Args:
            symbol: Trading pair (e.g., "BTCUSDT")
            metric_name: Metric type (e.g., "funding_rate")
            source: Data source (e.g., "binance_futures")
            start_time: Start of time range (inclusive)
            end_time: End of time range (inclusive)

        Returns:
            List of MetricPoint entities ordered by timestamp (ascending)
        """
        try:
            django_metrics = self._MetricPoint.objects.filter(
                client_id=self.client_id,
                symbol=symbol,
                metric_name=metric_name,
                source=source,
                timestamp__gte=start_time,
                timestamp__lte=end_time,
            ).order_by("timestamp")

            return [self._to_domain(dm) for dm in django_metrics]

        except Exception as e:
            logger.error(f"Failed to get time series: {e}")
            raise

    # ========================================================================
    # Private Mapping Methods
    # ========================================================================

    def _to_domain(self, django_metric) -> MetricPoint:
        """
        Map Django model → domain entity.

        Args:
            django_metric: Django MetricPoint model instance

        Returns:
            MetricPoint domain entity
        """
        return MetricPoint(
            timestamp=django_metric.timestamp,
            symbol=django_metric.symbol,
            metric_name=django_metric.metric_name,
            value=django_metric.value,
            source=django_metric.source,
            tags=django_metric.tags,
            collection_metadata=django_metric.collection_metadata,
        )


# ==========================================
# MOCK ADAPTERS (Testing)
# ==========================================


class MockDerivativesAdapter(DerivativesMetricCollector):
    """Mock adapter for testing (returns fake data)."""

    def collect_metrics(self, symbol: str) -> List[MetricPoint]:
        """Return mock metrics for testing."""
        current_time = timezone.now()
        return [
            MetricPoint(
                timestamp=current_time,
                symbol=symbol,
                metric_name="funding_rate",
                value=Decimal("0.0001"),
                source="mock_futures",
                tags={"test": "true"},
            ),
            MetricPoint(
                timestamp=current_time,
                symbol=symbol,
                metric_name="open_interest",
                value=Decimal("12345.67"),
                source="mock_futures",
                tags={"test": "true"},
            ),
            MetricPoint(
                timestamp=current_time,
                symbol=symbol,
                metric_name="mark_price",
                value=Decimal("95000.00"),
                source="mock_futures",
                tags={"test": "true"},
            ),
        ]


class InMemoryMetricRepository(MetricRepository):
    """In-memory repository for testing (no database required)."""

    def __init__(self, client_id: int):
        """Initialize in-memory repository."""
        self.client_id = client_id
        self.metrics: List[MetricPoint] = []

    def save_metric(self, metric: MetricPoint) -> MetricPoint:
        """Save to in-memory list."""
        # Check for duplicate (naive implementation)
        for existing in self.metrics:
            if (
                existing.source == metric.source
                and existing.symbol == metric.symbol
                and existing.metric_name == metric.metric_name
                and existing.timestamp == metric.timestamp
            ):
                return existing  # Idempotent

        self.metrics.append(metric)
        return metric

    def save_metrics_batch(self, metrics: List[MetricPoint]) -> int:
        """Save batch to in-memory list."""
        count = 0
        for metric in metrics:
            saved = self.save_metric(metric)
            if saved == metric:  # New metric
                count += 1
        return count

    def get_latest_metric(
        self,
        symbol: str,
        metric_name: str,
        source: str,
    ) -> Optional[MetricPoint]:
        """Get latest metric from in-memory list."""
        matching = [
            m
            for m in self.metrics
            if m.symbol == symbol
            and m.metric_name == metric_name
            and m.source == source
        ]
        if matching:
            return max(matching, key=lambda m: m.timestamp)
        return None

    def get_time_series(
        self,
        symbol: str,
        metric_name: str,
        source: str,
        start_time: datetime,
        end_time: datetime,
    ) -> List[MetricPoint]:
        """Get time series from in-memory list."""
        return sorted(
            [
                m
                for m in self.metrics
                if m.symbol == symbol
                and m.metric_name == metric_name
                and m.source == source
                and start_time <= m.timestamp <= end_time
            ],
            key=lambda m: m.timestamp,
        )
