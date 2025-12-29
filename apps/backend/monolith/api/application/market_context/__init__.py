"""
Market Research & Context Engine - Application Layer

This package implements Core 2: Market Research & Context Engine (ADR-0017).

Package structure follows hexagonal architecture pattern:

    domain.py        - Framework-agnostic domain entities (MetricPoint, etc.)
    ports.py         - Interface definitions (Protocols)
    adapters.py      - Concrete implementations (Binance, Django ORM)
    use_cases.py     - Business logic (CollectDerivativesMetrics, etc.)

Architecture Reference:
- ADR-0017: Market Research & Context Engine
- docs/market-context/README.md: User-facing documentation
- docs/market-context/IMPLEMENTATION-PLAN.md: Implementation milestones
"""

from .domain import (
    MetricPoint,
    FeatureVector,
    MarketContextSnapshot,
    validate_metric_name,
    validate_source,
)

from .ports import (
    DerivativesMetricCollector,
    MetricRepository,
)

from .adapters import (
    BinanceDerivativesAdapter,
    DjangoMetricRepository,
    MockDerivativesAdapter,
    InMemoryMetricRepository,
)

from .use_cases import (
    CollectDerivativesMetrics,
    GetLatestMetrics,
    CheckMetricFreshness,
)

__all__ = [
    # Domain entities
    "MetricPoint",
    "FeatureVector",
    "MarketContextSnapshot",
    # Validation helpers
    "validate_metric_name",
    "validate_source",
    # Ports
    "DerivativesMetricCollector",
    "MetricRepository",
    # Adapters
    "BinanceDerivativesAdapter",
    "DjangoMetricRepository",
    "MockDerivativesAdapter",
    "InMemoryMetricRepository",
    # Use cases
    "CollectDerivativesMetrics",
    "GetLatestMetrics",
    "CheckMetricFreshness",
]
