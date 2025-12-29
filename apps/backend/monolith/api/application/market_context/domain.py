"""
Market Research & Context Engine - Domain Entities

Framework-agnostic domain entities for the Market Context core.
These are immutable value objects with zero Django dependencies.

For persistence, see api/models/market_context.py (Django models).
For use cases, see api/application/market_context/use_cases.py.

Architecture Reference: ADR-0017 (Market Research & Context Engine)
"""

from __future__ import annotations
from dataclasses import dataclass, field
from datetime import datetime
from decimal import Decimal
from typing import Dict, Optional


@dataclass(frozen=True)
class MetricPoint:
    """
    Immutable raw time-series metric from external data sources.

    This is the domain representation (NO Django dependencies).
    For database persistence, adapters will map this to Django's MetricPoint model.

    Examples:
        >>> funding_rate = MetricPoint(
        ...     timestamp=datetime.now(),
        ...     symbol="BTCUSDT",
        ...     metric_name="funding_rate",
        ...     value=Decimal("0.0001"),
        ...     source="binance_futures",
        ...     tags={"timeframe": "8h", "contract": "perpetual"},
        ... )
        >>> funding_rate.metric_name
        'funding_rate'
        >>> funding_rate.value
        Decimal('0.0001')

    Immutability Guarantees:
        - Cannot modify timestamp, symbol, or value after creation
        - Safe to use in concurrent contexts
        - Hash-safe for use in sets/dicts

    Validation:
        - Timestamp must not be None
        - Symbol must be non-empty string
        - Metric name must be non-empty string
        - Value precision is preserved (Decimal, not float)
        - Source must be non-empty string
    """

    timestamp: datetime
    symbol: str
    metric_name: str
    value: Decimal
    source: str
    tags: Dict[str, str] = field(default_factory=dict)
    collection_metadata: Dict[str, any] = field(default_factory=dict)

    def __post_init__(self):
        """Validate domain invariants after initialization."""
        # Type validation (defensive programming)
        if not isinstance(self.timestamp, datetime):
            raise TypeError(f"timestamp must be datetime, got {type(self.timestamp)}")

        if not isinstance(self.symbol, str) or not self.symbol:
            raise ValueError(f"symbol must be non-empty string, got {self.symbol!r}")

        if not isinstance(self.metric_name, str) or not self.metric_name:
            raise ValueError(f"metric_name must be non-empty string, got {self.metric_name!r}")

        if not isinstance(self.value, Decimal):
            raise TypeError(f"value must be Decimal, got {type(self.value)}")

        if not isinstance(self.source, str) or not self.source:
            raise ValueError(f"source must be non-empty string, got {self.source!r}")

        # Symbol format validation (optional, can be relaxed)
        if len(self.symbol) > 20:
            raise ValueError(f"symbol too long (max 20 chars): {self.symbol!r}")

        if len(self.metric_name) > 50:
            raise ValueError(f"metric_name too long (max 50 chars): {self.metric_name!r}")

        if len(self.source) > 50:
            raise ValueError(f"source too long (max 50 chars): {self.source!r}")

    def __str__(self) -> str:
        """Human-readable string representation."""
        return (
            f"{self.symbol} | {self.metric_name}={self.value} | "
            f"{self.source} @ {self.timestamp.isoformat()}"
        )

    def __repr__(self) -> str:
        """Developer-friendly representation for debugging."""
        return (
            f"MetricPoint(timestamp={self.timestamp.isoformat()!r}, "
            f"symbol={self.symbol!r}, "
            f"metric_name={self.metric_name!r}, "
            f"value={self.value!r}, "
            f"source={self.source!r})"
        )

    @property
    def is_funding_rate(self) -> bool:
        """Check if this is a funding rate metric."""
        return self.metric_name == "funding_rate"

    @property
    def is_open_interest(self) -> bool:
        """Check if this is an open interest metric."""
        return self.metric_name == "open_interest"

    @property
    def is_mark_price(self) -> bool:
        """Check if this is a mark price metric."""
        return self.metric_name == "mark_price"

    @property
    def is_derivatives_metric(self) -> bool:
        """Check if this is a derivatives-related metric."""
        return self.metric_name in {"funding_rate", "open_interest", "mark_price", "index_price"}

    def to_dict(self) -> Dict[str, any]:
        """
        Convert to dictionary for serialization.

        Returns:
            Dictionary with all fields (timestamp as ISO string, value as string)
        """
        return {
            "timestamp": self.timestamp.isoformat(),
            "symbol": self.symbol,
            "metric_name": self.metric_name,
            "value": str(self.value),
            "source": self.source,
            "tags": self.tags,
            "collection_metadata": self.collection_metadata,
        }

    @classmethod
    def from_dict(cls, data: Dict[str, any]) -> MetricPoint:
        """
        Create MetricPoint from dictionary.

        Args:
            data: Dictionary with required fields

        Returns:
            MetricPoint instance

        Raises:
            KeyError: If required field is missing
            ValueError: If timestamp or value cannot be parsed
        """
        return cls(
            timestamp=datetime.fromisoformat(data["timestamp"]),
            symbol=data["symbol"],
            metric_name=data["metric_name"],
            value=Decimal(data["value"]),
            source=data["source"],
            tags=data.get("tags", {}),
            collection_metadata=data.get("collection_metadata", {}),
        )


@dataclass(frozen=True)
class FeatureVector:
    """
    Computed features for market regime classification.

    ⚠️ PLACEHOLDER for Milestone 2: Feature Engineering

    This will contain:
    - funding_rate_momentum: Rate of change of funding rate
    - oi_delta_15m: Open interest change over 15 minutes
    - mark_spot_spread: Difference between mark and spot price
    - volatility_estimate: Rolling volatility (e.g., 1-hour window)
    - etc.

    Design TBD after Milestone 1 is validated.
    """

    timestamp: datetime
    symbol: str
    features: Dict[str, Decimal]
    logic_version: str

    def __str__(self) -> str:
        return f"{self.symbol} features @ {self.timestamp.isoformat()}"


@dataclass(frozen=True)
class MarketContextSnapshot:
    """
    Classified market regime and risk bias snapshot.

    ⚠️ PLACEHOLDER for Milestone 3: Regime Classification

    This will contain:
    - market_regime: NORMAL | CHOP_RISK | SQUEEZE_RISK | HIGH_VOL
    - risk_bias: CONSERVATIVE | BALANCED | AGGRESSIVE
    - stop_vulnerability: LOW | MEDIUM | HIGH
    - recommended_posture: FAVOR_ENTRY | FAVOR_EXIT | WAIT_CONFIRM

    Design TBD after Milestone 2 is validated.
    """

    timestamp: datetime
    symbol: str
    market_regime: str
    risk_bias: str
    context_data: Dict[str, any]
    logic_version: str

    def __str__(self) -> str:
        return f"{self.symbol} | {self.market_regime} @ {self.timestamp.isoformat()}"


# ============================================================================
# Validation Helpers
# ============================================================================


def validate_metric_name(metric_name: str) -> bool:
    """
    Validate that a metric name is recognized.

    Args:
        metric_name: Metric name to validate

    Returns:
        True if valid, False otherwise
    """
    KNOWN_METRICS = {
        # Derivatives metrics (Binance Futures)
        "funding_rate",
        "open_interest",
        "mark_price",
        "index_price",
        "next_funding_time",
        # On-chain metrics (future)
        "tvl",
        "active_addresses",
        "gas_price",
        "transaction_count",
        # Spot metrics (future)
        "spot_price",
        "volume_24h",
        "bid_ask_spread",
    }
    return metric_name in KNOWN_METRICS


def validate_source(source: str) -> bool:
    """
    Validate that a data source is recognized.

    Args:
        source: Data source to validate

    Returns:
        True if valid, False otherwise
    """
    KNOWN_SOURCES = {
        # Exchange APIs
        "binance_futures",
        "binance_spot",
        "bybit_futures",
        # On-chain data providers
        "defillama",
        "glassnode",
        "coinmetrics",
        "dune_analytics",
    }
    return source in KNOWN_SOURCES
