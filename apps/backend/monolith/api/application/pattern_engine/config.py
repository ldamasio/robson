"""
Pattern Engine Configuration.

Default thresholds and parameters for each detector.
All values based on PATTERN_ENGINE_V1.md specification.
"""

from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal


@dataclass(frozen=True)
class HammerConfig:
    """
    Configuration for Hammer candlestick pattern.

    Pattern: Small body at top, long lower wick (2x body minimum).
    Bias: BULLISH (reversal from downtrend).
    """

    min_body_pct: Decimal = Decimal("0.10")  # 10% minimum body
    max_body_pct: Decimal = Decimal("0.30")  # 30% maximum body
    min_lower_wick_pct: Decimal = Decimal("0.60")  # 60% minimum lower wick
    max_upper_wick_pct: Decimal = Decimal("0.15")  # 15% maximum upper wick
    min_lower_to_body_ratio: Decimal = Decimal("2.0")  # Lower wick >= 2x body
    base_confidence: Decimal = Decimal("0.75")  # Starting confidence


@dataclass(frozen=True)
class InvertedHammerConfig:
    """
    Configuration for Inverted Hammer candlestick pattern.

    Pattern: Small body at bottom, long upper wick (2x body minimum).
    Bias: BULLISH (reversal from downtrend, needs confirmation).
    """

    min_body_pct: Decimal = Decimal("0.10")  # 10% minimum body
    max_body_pct: Decimal = Decimal("0.30")  # 30% maximum body
    min_upper_wick_pct: Decimal = Decimal("0.60")  # 60% minimum upper wick
    max_lower_wick_pct: Decimal = Decimal("0.15")  # 15% maximum lower wick
    min_upper_to_body_ratio: Decimal = Decimal("2.0")  # Upper wick >= 2x body
    base_confidence: Decimal = Decimal("0.70")  # Slightly lower (needs confirmation)


@dataclass(frozen=True)
class EngulfingConfig:
    """
    Configuration for Bullish/Bearish Engulfing pattern.

    Pattern: Second candle completely engulfs first candle's body.
    Bias: BULLISH (bullish engulfing) or BEARISH (bearish engulfing).
    """

    min_engulf_ratio: Decimal = Decimal("1.05")  # Body2 >= 1.05x Body1
    min_body_pct_first: Decimal = Decimal("0.20")  # First candle >= 20% body
    min_body_pct_second: Decimal = Decimal("0.30")  # Second candle >= 30% body
    max_wick_pct: Decimal = Decimal("0.40")  # Neither candle > 40% wicks
    base_confidence_bullish: Decimal = Decimal("0.80")
    base_confidence_bearish: Decimal = Decimal("0.80")


@dataclass(frozen=True)
class MorningStarConfig:
    """
    Configuration for Morning Star pattern.

    Pattern: 3-candle reversal - large bearish, small indecision, large bullish.
    Bias: BULLISH (reversal from downtrend).
    """

    min_first_body_pct: Decimal = Decimal("0.60")  # First candle >= 60% body (bearish)
    max_middle_body_pct: Decimal = Decimal("0.30")  # Middle candle <= 30% body
    min_third_body_pct: Decimal = Decimal("0.60")  # Third candle >= 60% body (bullish)
    min_middle_gap_down: Decimal = Decimal("0.005")  # Middle gaps down >= 0.5%
    min_third_penetration: Decimal = Decimal("0.50")  # Third closes >= 50% into first
    base_confidence: Decimal = Decimal("0.85")


@dataclass(frozen=True)
class HeadAndShouldersConfig:
    """
    Configuration for Head and Shoulders chart pattern.

    Pattern: Left Shoulder - Head - Right Shoulder with neckline support.
    Bias: BEARISH (reversal from uptrend).
    """

    min_head_prominence: Decimal = Decimal("0.03")  # Head >= 3% above shoulders
    max_shoulder_asymmetry: Decimal = Decimal("0.05")  # LS/RS heights within 5%
    max_neckline_slope_pct: Decimal = Decimal("0.02")  # Neckline <= 2% slope
    min_pivot_window_bars: int = 3  # k=3 fractal window
    min_bars_between_pivots: int = 5  # Minimum spacing between pivots
    max_pattern_bars: int = 100  # Maximum pattern duration
    base_confidence: Decimal = Decimal("0.80")


@dataclass(frozen=True)
class InvertedHeadAndShouldersConfig:
    """
    Configuration for Inverted Head and Shoulders chart pattern.

    Pattern: Left Shoulder - Head - Right Shoulder with neckline resistance.
    Bias: BULLISH (reversal from downtrend).
    """

    min_head_prominence: Decimal = Decimal("0.03")  # Head >= 3% below shoulders
    max_shoulder_asymmetry: Decimal = Decimal("0.05")  # LS/RS depths within 5%
    max_neckline_slope_pct: Decimal = Decimal("0.02")  # Neckline <= 2% slope
    min_pivot_window_bars: int = 3  # k=3 fractal window
    min_bars_between_pivots: int = 5  # Minimum spacing between pivots
    max_pattern_bars: int = 100  # Maximum pattern duration
    base_confidence: Decimal = Decimal("0.80")


@dataclass(frozen=True)
class PatternEngineConfig:
    """
    Master configuration for Pattern Detection Engine.

    Aggregates all detector configurations.
    """

    hammer: HammerConfig = HammerConfig()
    inverted_hammer: InvertedHammerConfig = InvertedHammerConfig()
    engulfing: EngulfingConfig = EngulfingConfig()
    morning_star: MorningStarConfig = MorningStarConfig()
    head_and_shoulders: HeadAndShouldersConfig = HeadAndShouldersConfig()
    inverted_head_and_shoulders: InvertedHeadAndShouldersConfig = InvertedHeadAndShouldersConfig()

    # Global settings
    default_candle_limit: int = 100  # Default window size for detection
    min_confidence_threshold: Decimal = Decimal("0.70")  # Minimum confidence to emit
    enable_confirmation_checks: bool = True  # Check confirmation for FORMING patterns
    enable_invalidation_checks: bool = True  # Check invalidation for active patterns


# Singleton instance
DEFAULT_CONFIG = PatternEngineConfig()
