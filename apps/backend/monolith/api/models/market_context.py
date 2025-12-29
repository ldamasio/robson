"""
Market Research & Context Engine Models

Django models for storing derivatives data and market context snapshots.
Part of Core 2: Market Research & Context Engine (ADR-0017).

Key Models:
- MetricPoint: Raw time-series data (funding rate, open interest, mark price)
- FeatureVector: Computed features for ML/heuristic classification (future)
- MarketContextSnapshot: Classified market regime + risk bias (future)

Architecture:
- These models are the PERSISTENCE layer only
- Domain entities live in api/application/market_context/domain.py
- Adapters map between domain entities and Django models
"""

from django.db import models
from django.utils import timezone
from decimal import Decimal
from .base import TimestampMixin, TenantMixin


class MetricPoint(TimestampMixin, TenantMixin, models.Model):
    """
    Raw time-series metric from external data sources.

    Stores derivatives data (funding rate, open interest, mark price)
    and on-chain metrics with full auditability.

    Key Design Decisions:
    - Immutable after creation (no updates, only inserts)
    - Idempotent upsert via unique constraint
    - Source-tagged for multi-provider support
    - Timestamp precision: seconds (not milliseconds)

    Unique Constraint:
    (client_id, source, symbol, metric_name, timestamp)

    This ensures:
    - No duplicate data points
    - Idempotent collection (can re-run without duplicates)
    - Multi-tenant isolation (client_id scoped)
    - Multi-source support (binance_futures vs defillama vs glassnode)
    """

    # ========================================================================
    # Data Identifiers
    # ========================================================================

    timestamp = models.DateTimeField(
        db_index=True,
        help_text="Metric timestamp (UTC, truncated to seconds)",
    )

    symbol = models.CharField(
        max_length=20,
        db_index=True,
        help_text="Trading pair (e.g., BTCUSDT)",
    )

    metric_name = models.CharField(
        max_length=50,
        db_index=True,
        help_text=(
            "Metric type: "
            "funding_rate, open_interest, mark_price, "
            "index_price, next_funding_time, "
            "tvl, active_addresses, gas_price, etc."
        ),
    )

    source = models.CharField(
        max_length=50,
        db_index=True,
        help_text=(
            "Data source: "
            "binance_futures, binance_spot, "
            "defillama, glassnode, coinmetrics, etc."
        ),
    )

    # ========================================================================
    # Metric Value
    # ========================================================================

    value = models.DecimalField(
        max_digits=30,
        decimal_places=18,
        help_text=(
            "Metric value with high precision. "
            "Examples: "
            "0.000100000000000000 (funding rate 0.01%), "
            "1234567.890000000000000000 (open interest in contracts), "
            "95432.100000000000000000 (mark price in USDT)"
        ),
    )

    # ========================================================================
    # Optional Metadata (JSON)
    # ========================================================================

    tags = models.JSONField(
        default=dict,
        blank=True,
        help_text=(
            "Optional metadata for context. "
            "Examples: "
            '{"timeframe": "8h", "contract": "perpetual"}, '
            '{"chain": "ethereum", "protocol": "uniswap"}'
        ),
    )

    # ========================================================================
    # Audit Trail
    # ========================================================================

    collection_metadata = models.JSONField(
        default=dict,
        blank=True,
        help_text=(
            "Collection context for debugging. "
            "Examples: "
            '{"collector_version": "1.0.0", "api_latency_ms": 120, "retry_count": 0}'
        ),
    )

    # ========================================================================
    # Model Meta
    # ========================================================================

    class Meta:
        db_table = 'market_metric_points'
        verbose_name = 'Market Metric Point'
        verbose_name_plural = 'Market Metric Points'

        # Idempotency constraint: prevent duplicate data points
        unique_together = [
            ('client', 'source', 'symbol', 'metric_name', 'timestamp'),
        ]

        # Query optimization indexes
        indexes = [
            # Fetch all metrics for a symbol in time range
            models.Index(
                fields=['client', 'symbol', 'timestamp'],
                name='idx_metric_symbol_time',
            ),
            # Fetch specific metric across symbols
            models.Index(
                fields=['client', 'metric_name', 'timestamp'],
                name='idx_metric_name_time',
            ),
            # Fetch latest metrics by source
            models.Index(
                fields=['client', 'source', 'symbol', 'metric_name', '-timestamp'],
                name='idx_metric_latest',
            ),
        ]

        # Default ordering: newest first
        ordering = ['-timestamp', 'symbol', 'metric_name']

    def __str__(self):
        return (
            f"{self.symbol} | {self.metric_name}={self.value} | "
            f"{self.source} @ {self.timestamp.isoformat()}"
        )

    def __repr__(self):
        return (
            f"<MetricPoint(symbol='{self.symbol}', "
            f"metric_name='{self.metric_name}', "
            f"value={self.value}, "
            f"source='{self.source}', "
            f"timestamp='{self.timestamp.isoformat()}')>"
        )

    # ========================================================================
    # Utility Methods
    # ========================================================================

    @property
    def age_seconds(self) -> float:
        """Returns metric age in seconds (for freshness checks)."""
        delta = timezone.now() - self.timestamp
        return delta.total_seconds()

    @property
    def is_stale(self, max_age_seconds: int = 300) -> bool:
        """
        Check if metric is stale (older than threshold).

        Args:
            max_age_seconds: Maximum allowed age (default: 5 minutes)

        Returns:
            True if metric is stale, False otherwise
        """
        return self.age_seconds > max_age_seconds

    @classmethod
    def latest_for_metric(cls, client_id: int, symbol: str, metric_name: str, source: str):
        """
        Get the most recent metric point for a specific metric.

        Args:
            client_id: Client (tenant) ID
            symbol: Trading pair
            metric_name: Metric type
            source: Data source

        Returns:
            MetricPoint instance or None
        """
        return cls.objects.filter(
            client_id=client_id,
            symbol=symbol,
            metric_name=metric_name,
            source=source,
        ).order_by('-timestamp').first()

    @classmethod
    def get_time_series(
        cls,
        client_id: int,
        symbol: str,
        metric_name: str,
        source: str,
        start_time: timezone.datetime,
        end_time: timezone.datetime = None,
    ):
        """
        Fetch time series data for a metric within a time range.

        Args:
            client_id: Client (tenant) ID
            symbol: Trading pair
            metric_name: Metric type
            source: Data source
            start_time: Start of time range (inclusive)
            end_time: End of time range (inclusive), defaults to now

        Returns:
            QuerySet of MetricPoint instances ordered by timestamp
        """
        if end_time is None:
            end_time = timezone.now()

        return cls.objects.filter(
            client_id=client_id,
            symbol=symbol,
            metric_name=metric_name,
            source=source,
            timestamp__gte=start_time,
            timestamp__lte=end_time,
        ).order_by('timestamp')


class FeatureVector(TimestampMixin, TenantMixin, models.Model):
    """
    Computed features for market regime classification.

    ⚠️ PLACEHOLDER for Milestone 2: Feature Engineering

    This model will store:
    - Derived features (funding_rate_momentum, oi_delta_15m, etc.)
    - Aggregated metrics (rolling averages, volatility estimates)
    - Normalized values for ML models

    Design TBD after Milestone 1 is validated.
    """

    timestamp = models.DateTimeField(
        db_index=True,
        help_text="Feature vector timestamp (UTC)",
    )

    symbol = models.CharField(
        max_length=20,
        db_index=True,
        help_text="Trading pair (e.g., BTCUSDT)",
    )

    features = models.JSONField(
        default=dict,
        help_text="Computed features as key-value pairs",
    )

    logic_version = models.CharField(
        max_length=20,
        help_text="Feature computation logic version (e.g., 'v1.0.0')",
    )

    class Meta:
        db_table = 'market_feature_vectors'
        verbose_name = 'Market Feature Vector'
        verbose_name_plural = 'Market Feature Vectors'

        unique_together = [
            ('client', 'symbol', 'timestamp', 'logic_version'),
        ]

        indexes = [
            models.Index(
                fields=['client', 'symbol', '-timestamp'],
                name='idx_features_latest',
            ),
        ]

        ordering = ['-timestamp', 'symbol']

    def __str__(self):
        return f"{self.symbol} features @ {self.timestamp.isoformat()}"


class MarketContextSnapshot(TimestampMixin, TenantMixin, models.Model):
    """
    Classified market regime and risk bias snapshot.

    ⚠️ PLACEHOLDER for Milestone 3: Regime Classification

    This model will store:
    - market_regime: NORMAL | CHOP_RISK | SQUEEZE_RISK | HIGH_VOL
    - risk_bias: CONSERVATIVE | BALANCED | AGGRESSIVE
    - stop_vulnerability: LOW | MEDIUM | HIGH
    - recommended_posture: FAVOR_ENTRY | FAVOR_EXIT | WAIT_CONFIRM

    Design TBD after Milestone 2 is validated.
    """

    timestamp = models.DateTimeField(
        db_index=True,
        help_text="Snapshot timestamp (UTC)",
    )

    symbol = models.CharField(
        max_length=20,
        db_index=True,
        help_text="Trading pair (e.g., BTCUSDT)",
    )

    market_regime = models.CharField(
        max_length=20,
        help_text="Classified market regime",
    )

    risk_bias = models.CharField(
        max_length=20,
        help_text="Risk bias classification",
    )

    context_data = models.JSONField(
        default=dict,
        help_text="Full context snapshot with all signals",
    )

    logic_version = models.CharField(
        max_length=20,
        help_text="Classification logic version (e.g., 'v1.0.0')",
    )

    class Meta:
        db_table = 'market_context_snapshots'
        verbose_name = 'Market Context Snapshot'
        verbose_name_plural = 'Market Context Snapshots'

        unique_together = [
            ('client', 'symbol', 'timestamp', 'logic_version'),
        ]

        indexes = [
            models.Index(
                fields=['client', 'symbol', '-timestamp'],
                name='idx_context_latest',
            ),
        ]

        ordering = ['-timestamp', 'symbol']

    def __str__(self):
        return f"{self.symbol} | {self.market_regime} @ {self.timestamp.isoformat()}"
