"""
Market Research & Context Engine - Use Cases

Business logic for the Market Context core.
Use cases orchestrate domain entities, ports, and adapters.

Use cases are framework-agnostic (NO Django dependencies).
All external dependencies are injected via ports (dependency inversion).

Architecture Reference: ADR-0017 (Market Research & Context Engine)
"""

from __future__ import annotations
import logging
from typing import List

from .ports import DerivativesMetricCollector, MetricRepository
from .domain import MetricPoint

logger = logging.getLogger(__name__)


class CollectDerivativesMetrics:
    """
    Use case for collecting derivatives data and persisting to repository.

    This use case orchestrates:
    1. Fetching derivatives data from external source (via collector port)
    2. Normalizing raw data to MetricPoint domain entities (done by adapter)
    3. Persisting metrics to repository (via repository port)
    4. Returning count of new metrics saved

    Design:
    - Framework-agnostic (no Django imports)
    - Dependencies injected via constructor (ports)
    - Returns primitive types or domain entities
    - Logging for observability

    Example Usage:
        >>> collector = BinanceDerivativesAdapter()
        >>> repository = DjangoMetricRepository(client_id=1)
        >>> use_case = CollectDerivativesMetrics(collector, repository)
        >>> count = use_case.execute("BTCUSDT")
        >>> count
        4  # funding_rate, open_interest, mark_price, index_price
    """

    def __init__(
        self,
        collector: DerivativesMetricCollector,
        repository: MetricRepository,
    ):
        """
        Initialize use case with dependencies.

        Args:
            collector: Port for collecting derivatives metrics
            repository: Port for persisting metrics
        """
        self.collector = collector
        self.repository = repository

    def execute(self, symbol: str) -> int:
        """
        Collect derivatives metrics and persist to repository.

        This method:
        1. Calls collector to fetch all derivatives metrics
        2. Persists metrics in batch (idempotent)
        3. Returns count of metrics processed

        Args:
            symbol: Trading pair (e.g., "BTCUSDT")

        Returns:
            Number of metrics collected and persisted

        Raises:
            ValueError: If symbol is empty or invalid
            Exception: If collection or persistence fails

        Example:
            >>> use_case.execute("BTCUSDT")
            4  # 4 metrics saved
        """
        # Validation
        if not symbol or not isinstance(symbol, str):
            raise ValueError(f"symbol must be non-empty string, got {symbol!r}")

        logger.info(f"Starting derivatives metrics collection for {symbol}")

        try:
            # Step 1: Collect metrics from external source
            logger.debug(f"Collecting metrics for {symbol}")
            metrics = self.collector.collect_metrics(symbol)

            if not metrics:
                logger.warning(f"No metrics collected for {symbol}")
                return 0

            logger.info(f"Collected {len(metrics)} metrics for {symbol}")

            # Step 2: Persist metrics to repository (batch operation)
            logger.debug(f"Persisting {len(metrics)} metrics for {symbol}")
            count = self.repository.save_metrics_batch(metrics)

            logger.info(
                f"Derivatives metrics collection complete for {symbol}: "
                f"{count} metrics processed"
            )

            return count

        except Exception as e:
            logger.error(
                f"Failed to collect derivatives metrics for {symbol}: {e}"
            )
            raise


class GetLatestMetrics:
    """
    Use case for retrieving the latest derivatives metrics for a symbol.

    This use case:
    1. Queries repository for latest metrics
    2. Returns metrics as domain entities

    Example Usage:
        >>> repository = DjangoMetricRepository(client_id=1)
        >>> use_case = GetLatestMetrics(repository)
        >>> metrics = use_case.execute("BTCUSDT", "binance_futures")
        >>> [m.metric_name for m in metrics]
        ['funding_rate', 'open_interest', 'mark_price']
    """

    def __init__(self, repository: MetricRepository):
        """
        Initialize use case with repository.

        Args:
            repository: Port for querying metrics
        """
        self.repository = repository

    def execute(
        self,
        symbol: str,
        source: str,
        metric_names: List[str] = None,
    ) -> List[MetricPoint]:
        """
        Get latest metrics for a symbol.

        Args:
            symbol: Trading pair (e.g., "BTCUSDT")
            source: Data source (e.g., "binance_futures")
            metric_names: Optional list of specific metrics to fetch
                         (default: ["funding_rate", "open_interest", "mark_price"])

        Returns:
            List of latest MetricPoint entities

        Raises:
            ValueError: If symbol or source is empty
            Exception: If repository query fails

        Example:
            >>> use_case.execute("BTCUSDT", "binance_futures")
            [MetricPoint(...), MetricPoint(...), MetricPoint(...)]
        """
        # Validation
        if not symbol or not isinstance(symbol, str):
            raise ValueError(f"symbol must be non-empty string, got {symbol!r}")

        if not source or not isinstance(source, str):
            raise ValueError(f"source must be non-empty string, got {source!r}")

        # Default metric names
        if metric_names is None:
            metric_names = ["funding_rate", "open_interest", "mark_price"]

        logger.debug(
            f"Fetching latest metrics for {symbol} from {source}: {metric_names}"
        )

        try:
            # Fetch latest for each metric
            metrics = []
            for metric_name in metric_names:
                latest = self.repository.get_latest_metric(
                    symbol=symbol,
                    metric_name=metric_name,
                    source=source,
                )
                if latest:
                    metrics.append(latest)
                else:
                    logger.warning(
                        f"No data found for {symbol} | {metric_name} | {source}"
                    )

            logger.info(
                f"Retrieved {len(metrics)} latest metrics for {symbol} from {source}"
            )

            return metrics

        except Exception as e:
            logger.error(f"Failed to get latest metrics for {symbol}: {e}")
            raise


class CheckMetricFreshness:
    """
    Use case for checking if metrics are stale (freshness monitoring).

    ⚠️ PLACEHOLDER for Milestone 4: Freshness Monitor

    This use case will:
    1. Query repository for latest metrics
    2. Calculate age of metrics
    3. Return freshness status (FRESH | STALE | MISSING)

    Design TBD after Milestone 1 is validated.
    """

    def __init__(self, repository: MetricRepository):
        """Initialize use case with repository."""
        self.repository = repository

    def execute(
        self,
        symbol: str,
        source: str,
        max_age_seconds: int = 300,
    ) -> dict:
        """
        Check freshness of metrics for a symbol.

        Args:
            symbol: Trading pair (e.g., "BTCUSDT")
            source: Data source (e.g., "binance_futures")
            max_age_seconds: Maximum allowed age in seconds (default: 5 minutes)

        Returns:
            Dictionary with freshness status:
            {
                "funding_rate": "FRESH",
                "open_interest": "STALE",
                "mark_price": "MISSING",
                "overall_status": "STALE",
            }

        Example:
            >>> use_case.execute("BTCUSDT", "binance_futures", max_age_seconds=300)
            {"funding_rate": "FRESH", "open_interest": "FRESH", ...}
        """
        # Placeholder implementation
        logger.warning(
            "CheckMetricFreshness use case is a placeholder (Milestone 4)"
        )
        return {
            "funding_rate": "UNKNOWN",
            "open_interest": "UNKNOWN",
            "mark_price": "UNKNOWN",
            "overall_status": "UNKNOWN",
        }
