"""
Pattern Detection Engine - Public API.

Exports domain entities, ports, configuration, helpers, and use cases.
"""

from .config import (
    DEFAULT_CONFIG,
    EngulfingConfig,
    HammerConfig,
    HeadAndShouldersConfig,
    InvertedHammerConfig,
    InvertedHeadAndShouldersConfig,
    MorningStarConfig,
    PatternEngineConfig,
)
from .domain import (
    OHLCV,
    CandleMetrics,
    CandleWindow,
    PatternLifecycleEvent,
    PatternSignature,
    PivotPoint,
)
from .helpers import (
    calculate_head_prominence,
    calculate_neckline_slope,
    calculate_shoulder_symmetry,
    compute_candle_metrics,
    find_pivots,
    get_latest_price,
    price_breaks_level,
    validate_pivot_spacing,
)
from .ports import CandleProvider, CandleProviderError, PatternDetector, PatternRepository
from .use_cases import PatternScanCommand, PatternScanResult, PatternScanUseCase

__all__ = [
    # Domain entities
    "OHLCV",
    "CandleWindow",
    "PivotPoint",
    "PatternSignature",
    "PatternLifecycleEvent",
    "CandleMetrics",
    # Ports
    "CandleProvider",
    "PatternRepository",
    "PatternDetector",
    "CandleProviderError",
    # Configuration
    "PatternEngineConfig",
    "HammerConfig",
    "InvertedHammerConfig",
    "EngulfingConfig",
    "MorningStarConfig",
    "HeadAndShouldersConfig",
    "InvertedHeadAndShouldersConfig",
    "DEFAULT_CONFIG",
    # Helpers
    "compute_candle_metrics",
    "find_pivots",
    "calculate_neckline_slope",
    "calculate_head_prominence",
    "calculate_shoulder_symmetry",
    "validate_pivot_spacing",
    "get_latest_price",
    "price_breaks_level",
    # Use cases
    "PatternScanUseCase",
    "PatternScanCommand",
    "PatternScanResult",
]
