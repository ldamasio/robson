"""
Pattern Detectors - Public API.

Exports all detector implementations.
"""

from .base import BaseCandlestickDetector, BaseChartDetector, BasePatternDetector
from .candlestick import (
    EngulfingDetector,
    HammerDetector,
    InvertedHammerDetector,
    MorningStarDetector,
)
from .chart import HeadAndShouldersDetector, InvertedHeadAndShouldersDetector

__all__ = [
    # Base classes
    "BasePatternDetector",
    "BaseCandlestickDetector",
    "BaseChartDetector",
    # Candlestick detectors
    "HammerDetector",
    "InvertedHammerDetector",
    "EngulfingDetector",
    "MorningStarDetector",
    # Chart detectors
    "HeadAndShouldersDetector",
    "InvertedHeadAndShouldersDetector",
]
