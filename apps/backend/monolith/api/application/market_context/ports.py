"""
Market Research & Context Engine - Ports (Interfaces)

Port definitions for the Market Context core.
These are Protocol interfaces that adapters must implement.

Hexagonal Architecture Pattern:
- Ports define WHAT operations are needed (interfaces)
- Adapters implement HOW those operations work (concrete implementations)
- Use cases depend on ports, NOT on adapters (dependency inversion)

Architecture Reference: ADR-0017 (Market Research & Context Engine)
"""

from __future__ import annotations
from typing import Protocol, List, Optional
from datetime import datetime
from .domain import MetricPoint


class DerivativesMetricCollector(Protocol):
    """
    Port for collecting derivatives metrics from external data sources.

    Implementations:
    - BinanceDerivativesAdapter: Fetches from Binance Futures API
    - MockDerivativesAdapter: Test double for unit tests
    """

    def collect_metrics(self, symbol: str) -> List[MetricPoint]:
        """
        Collect all derivatives metrics for a trading pair.

        Args:
            symbol: Trading pair (e.g., "BTCUSDT")

        Returns:
            List of MetricPoint domain entities (funding rate, OI, mark price)

        Raises:
            Exception: If data collection fails (network error, API error, etc.)

        Example:
            >>> collector = BinanceDerivativesAdapter()
            >>> metrics = collector.collect_metrics("BTCUSDT")
            >>> len(metrics)
            3  # funding_rate, open_interest, mark_price
        """
        ...


class MetricRepository(Protocol):
    """
    Port for persisting and querying metric data.

    Implementations:
    - DjangoMetricRepository: Stores in PostgreSQL via Django ORM
    - InMemoryMetricRepository: Test double for unit tests
    """

    def save_metric(self, metric: MetricPoint) -> MetricPoint:
        """
        Save a single metric point (idempotent upsert).

        If a metric with the same (client, source, symbol, metric_name, timestamp)
        already exists, this operation should be a no-op (idempotent).

        Args:
            metric: MetricPoint domain entity to save

        Returns:
            The saved MetricPoint (may be the existing one if duplicate)

        Raises:
            Exception: If persistence fails

        Example:
            >>> repo = DjangoMetricRepository(client_id=1)
            >>> metric = MetricPoint(...)
            >>> saved = repo.save_metric(metric)
            >>> saved.metric_name
            'funding_rate'
        """
        ...

    def save_metrics_batch(self, metrics: List[MetricPoint]) -> int:
        """
        Save multiple metrics in a single transaction (idempotent).

        This method should be optimized for bulk inserts (e.g., using
        Django's bulk_create with ignore_conflicts=True).

        Args:
            metrics: List of MetricPoint domain entities to save

        Returns:
            Number of new metrics actually inserted (not counting duplicates)

        Raises:
            Exception: If persistence fails

        Example:
            >>> repo = DjangoMetricRepository(client_id=1)
            >>> metrics = [metric1, metric2, metric3]
            >>> count = repo.save_metrics_batch(metrics)
            >>> count
            3  # All new
        """
        ...

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

        Example:
            >>> repo = DjangoMetricRepository(client_id=1)
            >>> latest = repo.get_latest_metric("BTCUSDT", "funding_rate", "binance_futures")
            >>> latest.value
            Decimal('0.0001')
        """
        ...

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

        Example:
            >>> repo = DjangoMetricRepository(client_id=1)
            >>> metrics = repo.get_time_series(
            ...     "BTCUSDT",
            ...     "funding_rate",
            ...     "binance_futures",
            ...     datetime(2025, 12, 28, 0, 0),
            ...     datetime(2025, 12, 28, 23, 59),
            ... )
            >>> len(metrics)
            288  # 8-hour funding (3 times per day, multiple collections)
        """
        ...
