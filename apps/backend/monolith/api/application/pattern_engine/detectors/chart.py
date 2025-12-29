"""
Chart Pattern Detectors.

Implements detection logic for multi-bar chart patterns.
All detectors are deterministic and use candle timestamps only.
"""

from __future__ import annotations

from decimal import Decimal

from ..config import HeadAndShouldersConfig, InvertedHeadAndShouldersConfig
from ..domain import CandleWindow, PatternSignature, PivotPoint
from ..helpers import (
    calculate_head_prominence,
    calculate_neckline_slope,
    calculate_shoulder_symmetry,
    find_pivots,
    get_latest_price,
    price_breaks_level,
    validate_pivot_spacing,
)
from .base import BaseChartDetector


class HeadAndShouldersDetector(BaseChartDetector):
    """
    Detect Head and Shoulders chart pattern.

    Pattern: Left Shoulder - Head - Right Shoulder with neckline support.
    Bias: BEARISH (reversal from uptrend).
    """

    def __init__(self, config: HeadAndShouldersConfig | None = None):
        """
        Initialize detector.

        Args:
            config: Detector configuration (uses defaults if None)
        """
        super().__init__(pattern_code="HEAD_AND_SHOULDERS")
        self.config = config or HeadAndShouldersConfig()

    def detect(self, window: CandleWindow) -> list[PatternSignature]:
        """
        Scan window for Head and Shoulders patterns.

        Steps:
        1. Find swing highs using fractal algorithm (k=3)
        2. Look for 3 consecutive highs: LS - HEAD - RS
        3. Validate head prominence (>= 3%)
        4. Validate shoulder symmetry (<= 5%)
        5. Calculate neckline from lows between pivots
        6. Validate neckline slope (<= 2%)

        Args:
            window: Candle window to analyze

        Returns:
            List of detected signatures
        """
        # Need enough data for pattern
        if len(window) < self.config.max_pattern_bars:
            return []

        # Find swing highs
        swing_highs = find_pivots(window, k=self.config.min_pivot_window_bars, pivot_type="HIGH")

        if len(swing_highs) < 3:
            return []

        signatures = []

        # Check consecutive triplets of swing highs
        for i in range(len(swing_highs) - 2):
            left_shoulder = swing_highs[i]
            head = swing_highs[i + 1]
            right_shoulder = swing_highs[i + 2]

            signature = self._check_hns_pattern(window, left_shoulder, head, right_shoulder)
            if signature:
                signatures.append(signature)

        return signatures

    def _check_hns_pattern(
        self,
        window: CandleWindow,
        left_shoulder: PivotPoint,
        head: PivotPoint,
        right_shoulder: PivotPoint,
    ) -> PatternSignature | None:
        """
        Check if three swing highs form valid H&S pattern.

        Args:
            window: Candle window
            left_shoulder: Left shoulder pivot
            head: Head pivot
            right_shoulder: Right shoulder pivot

        Returns:
            PatternSignature if valid H&S, None otherwise
        """
        # Rule 1: Validate pivot spacing
        pivots = [left_shoulder, head, right_shoulder]
        if not validate_pivot_spacing(pivots, self.config.min_bars_between_pivots):
            return None

        # Rule 2: Pattern duration within limits
        pattern_bars = right_shoulder.bar_index - left_shoulder.bar_index
        if pattern_bars > self.config.max_pattern_bars:
            return None

        # Rule 3: Head must be HIGHER than both shoulders
        if head.price <= left_shoulder.price or head.price <= right_shoulder.price:
            return None

        # Rule 4: Head prominence >= 3%
        prominence = calculate_head_prominence(head, left_shoulder, right_shoulder)
        if prominence < self.config.min_head_prominence:
            return None

        # Rule 5: Shoulder symmetry <= 5%
        symmetry = calculate_shoulder_symmetry(left_shoulder, right_shoulder)
        if symmetry > self.config.max_shoulder_asymmetry:
            return None

        # Rule 6: Find neckline (support between pivots)
        # Neckline = lows between LS-HEAD and HEAD-RS
        neckline_left = self._find_low_between(window, left_shoulder.bar_index, head.bar_index)
        neckline_right = self._find_low_between(window, head.bar_index, right_shoulder.bar_index)

        if neckline_left is None or neckline_right is None:
            return None

        # Rule 7: Neckline slope <= 2%
        neckline_slope = calculate_neckline_slope(neckline_left, neckline_right)
        if abs(neckline_slope) > self.config.max_neckline_slope_pct:
            return None

        # Calculate target price (head height projected down from neckline)
        avg_neckline = (neckline_left.price + neckline_right.price) / Decimal("2")
        head_height = head.price - avg_neckline
        target_price = avg_neckline - head_height

        # Create signature
        evidence = {
            "left_shoulder_price": float(left_shoulder.price),
            "head_price": float(head.price),
            "right_shoulder_price": float(right_shoulder.price),
            "neckline_left_price": float(neckline_left.price),
            "neckline_right_price": float(neckline_right.price),
            "neckline_slope_pct": float(neckline_slope),
            "head_prominence_pct": float(prominence),
            "shoulder_symmetry": float(symmetry),
            "neckline_support": float(avg_neckline),
            "target_price": float(target_price),
            "pattern_bars": pattern_bars,
        }

        return PatternSignature(
            pattern_code=self.pattern_code,
            symbol=window.symbol,
            timeframe=window.timeframe,
            start_ts=left_shoulder.ts,  # From candle timestamp
            end_ts=right_shoulder.ts,  # From candle timestamp
            confidence=self.config.base_confidence,
            evidence=evidence,
            key_points=(left_shoulder, head, right_shoulder, neckline_left, neckline_right),
        )

    def _find_low_between(
        self, window: CandleWindow, start_index: int, end_index: int
    ) -> PivotPoint | None:
        """
        Find lowest low between two bar indices.

        Args:
            window: Candle window
            start_index: Start index (exclusive)
            end_index: End index (exclusive)

        Returns:
            PivotPoint at lowest low, or None if range invalid
        """
        if start_index >= end_index or end_index >= len(window):
            return None

        lowest_candle = None
        lowest_price = None
        lowest_index = None

        for i in range(start_index + 1, end_index):
            candle = window[i]
            if lowest_price is None or candle.low < lowest_price:
                lowest_price = candle.low
                lowest_candle = candle
                lowest_index = i

        if lowest_candle is None:
            return None

        return PivotPoint(
            ts=lowest_candle.ts,
            price=lowest_price,
            pivot_type="LOW",
            bar_index=lowest_index,
        )

    def check_confirmation(self, instance: object, window: CandleWindow) -> dict | None:
        """
        Check if H&S is confirmed.

        Confirmation: Price closes BELOW neckline with 0.2% threshold.

        Args:
            instance: PatternInstance
            window: Recent candles

        Returns:
            Evidence dict if confirmed, None otherwise
        """
        if len(window) < 1:
            return None

        # Get neckline support level
        neckline_support = Decimal(str(instance.evidence["neckline_support"]))
        current_price = get_latest_price(window)

        # Check if price broke below neckline with threshold
        if price_breaks_level(current_price, neckline_support, "BELOW", Decimal("0.002")):
            return {
                "confirmation_type": "NECKLINE_BREAK",
                "neckline_level": float(neckline_support),
                "breakout_price": float(current_price),
                "target_price": instance.evidence["target_price"],
                "confirmation_ts": window[-1].ts.isoformat(),
            }

        return None

    def check_invalidation(self, instance: object, window: CandleWindow) -> dict | None:
        """
        Check if H&S is invalidated.

        Invalidation: Price closes ABOVE head with 0.5% threshold.

        Args:
            instance: PatternInstance
            window: Recent candles

        Returns:
            Evidence dict if invalidated, None otherwise
        """
        if len(window) < 1:
            return None

        # Get head price
        head_price = Decimal(str(instance.evidence["head_price"]))
        current_price = get_latest_price(window)

        # Check if price broke above head with threshold
        if price_breaks_level(current_price, head_price, "ABOVE", Decimal("0.005")):
            return {
                "invalidation_reason": "PRICE_ABOVE_HEAD",
                "head_price": float(head_price),
                "invalidation_price": float(current_price),
                "invalidation_ts": window[-1].ts.isoformat(),
            }

        return None


class InvertedHeadAndShouldersDetector(BaseChartDetector):
    """
    Detect Inverted Head and Shoulders chart pattern.

    Pattern: Left Shoulder - Head - Right Shoulder with neckline resistance.
    Bias: BULLISH (reversal from downtrend).
    """

    def __init__(self, config: InvertedHeadAndShouldersConfig | None = None):
        """
        Initialize detector.

        Args:
            config: Detector configuration (uses defaults if None)
        """
        super().__init__(pattern_code="INVERTED_HEAD_AND_SHOULDERS")
        self.config = config or InvertedHeadAndShouldersConfig()

    def detect(self, window: CandleWindow) -> list[PatternSignature]:
        """
        Scan window for Inverted Head and Shoulders patterns.

        Steps:
        1. Find swing lows using fractal algorithm (k=3)
        2. Look for 3 consecutive lows: LS - HEAD - RS
        3. Validate head prominence (>= 3%)
        4. Validate shoulder symmetry (<= 5%)
        5. Calculate neckline from highs between pivots
        6. Validate neckline slope (<= 2%)

        Args:
            window: Candle window to analyze

        Returns:
            List of detected signatures
        """
        # Need enough data for pattern
        if len(window) < self.config.max_pattern_bars:
            return []

        # Find swing lows
        swing_lows = find_pivots(window, k=self.config.min_pivot_window_bars, pivot_type="LOW")

        if len(swing_lows) < 3:
            return []

        signatures = []

        # Check consecutive triplets of swing lows
        for i in range(len(swing_lows) - 2):
            left_shoulder = swing_lows[i]
            head = swing_lows[i + 1]
            right_shoulder = swing_lows[i + 2]

            signature = self._check_ihns_pattern(window, left_shoulder, head, right_shoulder)
            if signature:
                signatures.append(signature)

        return signatures

    def _check_ihns_pattern(
        self,
        window: CandleWindow,
        left_shoulder: PivotPoint,
        head: PivotPoint,
        right_shoulder: PivotPoint,
    ) -> PatternSignature | None:
        """
        Check if three swing lows form valid IHNS pattern.

        Args:
            window: Candle window
            left_shoulder: Left shoulder pivot
            head: Head pivot
            right_shoulder: Right shoulder pivot

        Returns:
            PatternSignature if valid IHNS, None otherwise
        """
        # Rule 1: Validate pivot spacing
        pivots = [left_shoulder, head, right_shoulder]
        if not validate_pivot_spacing(pivots, self.config.min_bars_between_pivots):
            return None

        # Rule 2: Pattern duration within limits
        pattern_bars = right_shoulder.bar_index - left_shoulder.bar_index
        if pattern_bars > self.config.max_pattern_bars:
            return None

        # Rule 3: Head must be LOWER than both shoulders
        if head.price >= left_shoulder.price or head.price >= right_shoulder.price:
            return None

        # Rule 4: Head prominence >= 3%
        prominence = calculate_head_prominence(head, left_shoulder, right_shoulder)
        if prominence < self.config.min_head_prominence:
            return None

        # Rule 5: Shoulder symmetry <= 5%
        symmetry = calculate_shoulder_symmetry(left_shoulder, right_shoulder)
        if symmetry > self.config.max_shoulder_asymmetry:
            return None

        # Rule 6: Find neckline (resistance between pivots)
        # Neckline = highs between LS-HEAD and HEAD-RS
        neckline_left = self._find_high_between(window, left_shoulder.bar_index, head.bar_index)
        neckline_right = self._find_high_between(window, head.bar_index, right_shoulder.bar_index)

        if neckline_left is None or neckline_right is None:
            return None

        # Rule 7: Neckline slope <= 2%
        neckline_slope = calculate_neckline_slope(neckline_left, neckline_right)
        if abs(neckline_slope) > self.config.max_neckline_slope_pct:
            return None

        # Calculate target price (head depth projected up from neckline)
        avg_neckline = (neckline_left.price + neckline_right.price) / Decimal("2")
        head_depth = avg_neckline - head.price
        target_price = avg_neckline + head_depth

        # Create signature
        evidence = {
            "left_shoulder_price": float(left_shoulder.price),
            "head_price": float(head.price),
            "right_shoulder_price": float(right_shoulder.price),
            "neckline_left_price": float(neckline_left.price),
            "neckline_right_price": float(neckline_right.price),
            "neckline_slope_pct": float(neckline_slope),
            "head_prominence_pct": float(prominence),
            "shoulder_symmetry": float(symmetry),
            "neckline_resistance": float(avg_neckline),
            "target_price": float(target_price),
            "pattern_bars": pattern_bars,
        }

        return PatternSignature(
            pattern_code=self.pattern_code,
            symbol=window.symbol,
            timeframe=window.timeframe,
            start_ts=left_shoulder.ts,  # From candle timestamp
            end_ts=right_shoulder.ts,  # From candle timestamp
            confidence=self.config.base_confidence,
            evidence=evidence,
            key_points=(left_shoulder, head, right_shoulder, neckline_left, neckline_right),
        )

    def _find_high_between(
        self, window: CandleWindow, start_index: int, end_index: int
    ) -> PivotPoint | None:
        """
        Find highest high between two bar indices.

        Args:
            window: Candle window
            start_index: Start index (exclusive)
            end_index: End index (exclusive)

        Returns:
            PivotPoint at highest high, or None if range invalid
        """
        if start_index >= end_index or end_index >= len(window):
            return None

        highest_candle = None
        highest_price = None
        highest_index = None

        for i in range(start_index + 1, end_index):
            candle = window[i]
            if highest_price is None or candle.high > highest_price:
                highest_price = candle.high
                highest_candle = candle
                highest_index = i

        if highest_candle is None:
            return None

        return PivotPoint(
            ts=highest_candle.ts,
            price=highest_price,
            pivot_type="HIGH",
            bar_index=highest_index,
        )

    def check_confirmation(self, instance: object, window: CandleWindow) -> dict | None:
        """
        Check if IHNS is confirmed.

        Confirmation: Price closes ABOVE neckline with 0.2% threshold.

        Args:
            instance: PatternInstance
            window: Recent candles

        Returns:
            Evidence dict if confirmed, None otherwise
        """
        if len(window) < 1:
            return None

        # Get neckline resistance level
        neckline_resistance = Decimal(str(instance.evidence["neckline_resistance"]))
        current_price = get_latest_price(window)

        # Check if price broke above neckline with threshold
        if price_breaks_level(current_price, neckline_resistance, "ABOVE", Decimal("0.002")):
            return {
                "confirmation_type": "NECKLINE_BREAK",
                "neckline_level": float(neckline_resistance),
                "breakout_price": float(current_price),
                "target_price": instance.evidence["target_price"],
                "confirmation_ts": window[-1].ts.isoformat(),
            }

        return None

    def check_invalidation(self, instance: object, window: CandleWindow) -> dict | None:
        """
        Check if IHNS is invalidated.

        Invalidation: Price closes BELOW head with 0.5% threshold.

        Args:
            instance: PatternInstance
            window: Recent candles

        Returns:
            Evidence dict if invalidated, None otherwise
        """
        if len(window) < 1:
            return None

        # Get head price
        head_price = Decimal(str(instance.evidence["head_price"]))
        current_price = get_latest_price(window)

        # Check if price broke below head with threshold
        if price_breaks_level(current_price, head_price, "BELOW", Decimal("0.005")):
            return {
                "invalidation_reason": "PRICE_BELOW_HEAD",
                "head_price": float(head_price),
                "invalidation_price": float(current_price),
                "invalidation_ts": window[-1].ts.isoformat(),
            }

        return None
