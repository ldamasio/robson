"""
Candlestick Pattern Detectors.

Implements detection logic for candlestick reversal patterns.
All detectors are deterministic and use candle timestamps only.
"""

from __future__ import annotations

from decimal import Decimal

from ..config import EngulfingConfig, HammerConfig, InvertedHammerConfig, MorningStarConfig
from ..domain import CandleWindow, PatternSignature
from ..helpers import compute_candle_metrics
from .base import BaseCandlestickDetector


class HammerDetector(BaseCandlestickDetector):
    """
    Detect Hammer candlestick pattern.

    Pattern: Small body at top, long lower wick (2x body minimum).
    Bias: BULLISH (reversal from downtrend).
    """

    def __init__(self, config: HammerConfig | None = None):
        """
        Initialize detector.

        Args:
            config: Detector configuration (uses defaults if None)
        """
        super().__init__(pattern_code="HAMMER")
        self.config = config or HammerConfig()

    def _check_candle(self, window: CandleWindow, index: int) -> PatternSignature | None:
        """
        Check if candle at index is a Hammer.

        Args:
            window: Candle window
            index: Index of candle to check

        Returns:
            PatternSignature if Hammer found, None otherwise
        """
        candle = window[index]
        metrics = compute_candle_metrics(candle)

        # Rule 1: Body between 10-30% of range
        if not (self.config.min_body_pct <= metrics.body_pct <= self.config.max_body_pct):
            return None

        # Rule 2: Lower wick >= 60% of range
        if metrics.lower_wick_pct < self.config.min_lower_wick_pct:
            return None

        # Rule 3: Upper wick <= 15% of range
        if metrics.upper_wick_pct > self.config.max_upper_wick_pct:
            return None

        # Rule 4: Lower wick >= 2x body
        if metrics.lower_wick < (metrics.body * self.config.min_lower_to_body_ratio):
            return None

        # Pattern detected - create signature
        evidence = {
            "body_pct": float(metrics.body_pct),
            "lower_wick_pct": float(metrics.lower_wick_pct),
            "upper_wick_pct": float(metrics.upper_wick_pct),
            "lower_to_body_ratio": (
                float(metrics.lower_wick / metrics.body) if metrics.body > 0 else 0
            ),
            "candle_open": float(candle.open),
            "candle_high": float(candle.high),
            "candle_low": float(candle.low),
            "candle_close": float(candle.close),
        }

        return PatternSignature(
            pattern_code=self.pattern_code,
            symbol=window.symbol,
            timeframe=window.timeframe,
            start_ts=candle.ts,  # From candle timestamp
            end_ts=candle.ts,  # Single-candle pattern
            confidence=self.config.base_confidence,
            evidence=evidence,
            key_points=tuple(),  # No pivot points for candlestick patterns
        )

    def check_confirmation(self, instance: object, window: CandleWindow) -> dict | None:
        """
        Check if Hammer is confirmed.

        Confirmation: Next candle closes ABOVE hammer high.

        Args:
            instance: PatternInstance
            window: Recent candles

        Returns:
            Evidence dict if confirmed, None otherwise
        """
        if len(window) < 2:
            return None

        # Get hammer candle (from evidence)
        hammer_high = Decimal(str(instance.evidence["candle_high"]))

        # Check if latest candle closed above hammer high
        latest_candle = window[-1]
        if latest_candle.close > hammer_high:
            return {
                "confirmation_type": "CLOSE_ABOVE_HIGH",
                "hammer_high": float(hammer_high),
                "confirmation_price": float(latest_candle.close),
                "confirmation_ts": latest_candle.ts.isoformat(),
            }

        return None

    def check_invalidation(self, instance: object, window: CandleWindow) -> dict | None:
        """
        Check if Hammer is invalidated.

        Invalidation: Price closes BELOW hammer low.

        Args:
            instance: PatternInstance
            window: Recent candles

        Returns:
            Evidence dict if invalidated, None otherwise
        """
        if len(window) < 1:
            return None

        # Get hammer candle low (from evidence)
        hammer_low = Decimal(str(instance.evidence["candle_low"]))

        # Check if latest candle closed below hammer low
        latest_candle = window[-1]
        if latest_candle.close < hammer_low:
            return {
                "invalidation_reason": "CLOSE_BELOW_LOW",
                "hammer_low": float(hammer_low),
                "invalidation_price": float(latest_candle.close),
                "invalidation_ts": latest_candle.ts.isoformat(),
            }

        return None


class InvertedHammerDetector(BaseCandlestickDetector):
    """
    Detect Inverted Hammer candlestick pattern.

    Pattern: Small body at bottom, long upper wick (2x body minimum).
    Bias: BULLISH (reversal from downtrend, needs confirmation).
    """

    def __init__(self, config: InvertedHammerConfig | None = None):
        """
        Initialize detector.

        Args:
            config: Detector configuration (uses defaults if None)
        """
        super().__init__(pattern_code="INVERTED_HAMMER")
        self.config = config or InvertedHammerConfig()

    def _check_candle(self, window: CandleWindow, index: int) -> PatternSignature | None:
        """
        Check if candle at index is an Inverted Hammer.

        Args:
            window: Candle window
            index: Index of candle to check

        Returns:
            PatternSignature if Inverted Hammer found, None otherwise
        """
        candle = window[index]
        metrics = compute_candle_metrics(candle)

        # Rule 1: Body between 10-30% of range
        if not (self.config.min_body_pct <= metrics.body_pct <= self.config.max_body_pct):
            return None

        # Rule 2: Upper wick >= 60% of range
        if metrics.upper_wick_pct < self.config.min_upper_wick_pct:
            return None

        # Rule 3: Lower wick <= 15% of range
        if metrics.lower_wick_pct > self.config.max_lower_wick_pct:
            return None

        # Rule 4: Upper wick >= 2x body
        if metrics.upper_wick < (metrics.body * self.config.min_upper_to_body_ratio):
            return None

        # Pattern detected
        evidence = {
            "body_pct": float(metrics.body_pct),
            "upper_wick_pct": float(metrics.upper_wick_pct),
            "lower_wick_pct": float(metrics.lower_wick_pct),
            "upper_to_body_ratio": (
                float(metrics.upper_wick / metrics.body) if metrics.body > 0 else 0
            ),
            "candle_open": float(candle.open),
            "candle_high": float(candle.high),
            "candle_low": float(candle.low),
            "candle_close": float(candle.close),
        }

        return PatternSignature(
            pattern_code=self.pattern_code,
            symbol=window.symbol,
            timeframe=window.timeframe,
            start_ts=candle.ts,
            end_ts=candle.ts,
            confidence=self.config.base_confidence,
            evidence=evidence,
            key_points=tuple(),
        )

    def check_confirmation(self, instance: object, window: CandleWindow) -> dict | None:
        """
        Check if Inverted Hammer is confirmed.

        Confirmation: Next candle closes ABOVE inverted hammer high.

        Args:
            instance: PatternInstance
            window: Recent candles

        Returns:
            Evidence dict if confirmed, None otherwise
        """
        if len(window) < 2:
            return None

        inverted_hammer_high = Decimal(str(instance.evidence["candle_high"]))
        latest_candle = window[-1]

        if latest_candle.close > inverted_hammer_high:
            return {
                "confirmation_type": "CLOSE_ABOVE_HIGH",
                "inverted_hammer_high": float(inverted_hammer_high),
                "confirmation_price": float(latest_candle.close),
                "confirmation_ts": latest_candle.ts.isoformat(),
            }

        return None

    def check_invalidation(self, instance: object, window: CandleWindow) -> dict | None:
        """
        Check if Inverted Hammer is invalidated.

        Invalidation: Price closes BELOW inverted hammer low.

        Args:
            instance: PatternInstance
            window: Recent candles

        Returns:
            Evidence dict if invalidated, None otherwise
        """
        if len(window) < 1:
            return None

        inverted_hammer_low = Decimal(str(instance.evidence["candle_low"]))
        latest_candle = window[-1]

        if latest_candle.close < inverted_hammer_low:
            return {
                "invalidation_reason": "CLOSE_BELOW_LOW",
                "inverted_hammer_low": float(inverted_hammer_low),
                "invalidation_price": float(latest_candle.close),
                "invalidation_ts": latest_candle.ts.isoformat(),
            }

        return None


class EngulfingDetector(BaseCandlestickDetector):
    """
    Detect Bullish/Bearish Engulfing pattern.

    Pattern: Second candle completely engulfs first candle's body.
    Bias: BULLISH (bullish engulfing) or BEARISH (bearish engulfing).
    """

    def __init__(self, config: EngulfingConfig | None = None):
        """
        Initialize detector.

        Args:
            config: Detector configuration (uses defaults if None)
        """
        super().__init__(pattern_code="ENGULFING")
        self.config = config or EngulfingConfig()

    def detect(self, window: CandleWindow) -> list[PatternSignature]:
        """
        Override detect to check 2-candle pattern.

        Args:
            window: Candle window

        Returns:
            List of detected signatures
        """
        if len(window) < 2:
            return []

        signatures = []

        # Check last 2 candles
        signature = self._check_engulfing_pair(window, len(window) - 2)
        if signature:
            signatures.append(signature)

        return signatures

    def _check_engulfing_pair(self, window: CandleWindow, index: int) -> PatternSignature | None:
        """
        Check if candles at index and index+1 form engulfing pattern.

        Args:
            window: Candle window
            index: Index of first candle

        Returns:
            PatternSignature if engulfing found, None otherwise
        """
        if index + 1 >= len(window):
            return None

        first = window[index]
        second = window[index + 1]

        first_metrics = compute_candle_metrics(first)
        second_metrics = compute_candle_metrics(second)

        # Rule 1: First candle has >= 20% body
        if first_metrics.body_pct < self.config.min_body_pct_first:
            return None

        # Rule 2: Second candle has >= 30% body
        if second_metrics.body_pct < self.config.min_body_pct_second:
            return None

        # Rule 3: Neither candle has > 40% wicks
        total_wick_first = first_metrics.upper_wick_pct + first_metrics.lower_wick_pct
        total_wick_second = second_metrics.upper_wick_pct + second_metrics.lower_wick_pct

        if (
            total_wick_first > self.config.max_wick_pct
            or total_wick_second > self.config.max_wick_pct
        ):
            return None

        # Rule 4: Second body engulfs first body
        # Bullish: first bearish, second bullish, second.close > first.open AND second.open < first.close
        # Bearish: first bullish, second bearish, second.close < first.open AND second.open > first.close

        is_bullish_engulfing = (
            first_metrics.is_bearish
            and second_metrics.is_bullish
            and second.close > first.open
            and second.open < first.close
        )

        is_bearish_engulfing = (
            first_metrics.is_bullish
            and second_metrics.is_bearish
            and second.close < first.open
            and second.open > first.close
        )

        if not (is_bullish_engulfing or is_bearish_engulfing):
            return None

        # Rule 5: Engulf ratio >= 1.05
        engulf_ratio = (
            second_metrics.body / first_metrics.body if first_metrics.body > 0 else Decimal("0")
        )
        if engulf_ratio < self.config.min_engulf_ratio:
            return None

        # Determine pattern type and confidence
        if is_bullish_engulfing:
            pattern_code = "BULLISH_ENGULFING"
            confidence = self.config.base_confidence_bullish
        else:
            pattern_code = "BEARISH_ENGULFING"
            confidence = self.config.base_confidence_bearish

        evidence = {
            "pattern_type": pattern_code,
            "first_candle_body_pct": float(first_metrics.body_pct),
            "second_candle_body_pct": float(second_metrics.body_pct),
            "engulf_ratio": float(engulf_ratio),
            "first_candle_open": float(first.open),
            "first_candle_close": float(first.close),
            "second_candle_open": float(second.open),
            "second_candle_high": float(second.high),
            "second_candle_low": float(second.low),
            "second_candle_close": float(second.close),
        }

        return PatternSignature(
            pattern_code=pattern_code,
            symbol=window.symbol,
            timeframe=window.timeframe,
            start_ts=first.ts,
            end_ts=second.ts,
            confidence=confidence,
            evidence=evidence,
            key_points=tuple(),
        )

    def _check_candle(self, window: CandleWindow, index: int) -> PatternSignature | None:
        """
        Not used for engulfing (2-candle pattern).

        Args:
            window: Candle window
            index: Candle index

        Returns:
            None (not applicable)
        """
        return None

    def check_confirmation(self, instance: object, window: CandleWindow) -> dict | None:
        """
        Check if Engulfing is confirmed.

        Bullish: Next candle closes above engulfing high.
        Bearish: Next candle closes below engulfing low.

        Args:
            instance: PatternInstance
            window: Recent candles

        Returns:
            Evidence dict if confirmed, None otherwise
        """
        if len(window) < 1:
            return None

        pattern_type = instance.evidence["pattern_type"]
        latest_candle = window[-1]

        if pattern_type == "BULLISH_ENGULFING":
            engulfing_high = Decimal(str(instance.evidence["second_candle_high"]))
            if latest_candle.close > engulfing_high:
                return {
                    "confirmation_type": "CLOSE_ABOVE_HIGH",
                    "engulfing_high": float(engulfing_high),
                    "confirmation_price": float(latest_candle.close),
                    "confirmation_ts": latest_candle.ts.isoformat(),
                }

        elif pattern_type == "BEARISH_ENGULFING":
            engulfing_low = Decimal(str(instance.evidence["second_candle_low"]))
            if latest_candle.close < engulfing_low:
                return {
                    "confirmation_type": "CLOSE_BELOW_LOW",
                    "engulfing_low": float(engulfing_low),
                    "confirmation_price": float(latest_candle.close),
                    "confirmation_ts": latest_candle.ts.isoformat(),
                }

        return None

    def check_invalidation(self, instance: object, window: CandleWindow) -> dict | None:
        """
        Check if Engulfing is invalidated.

        Bullish: Price closes below engulfing low.
        Bearish: Price closes above engulfing high.

        Args:
            instance: PatternInstance
            window: Recent candles

        Returns:
            Evidence dict if invalidated, None otherwise
        """
        if len(window) < 1:
            return None

        pattern_type = instance.evidence["pattern_type"]
        latest_candle = window[-1]

        if pattern_type == "BULLISH_ENGULFING":
            engulfing_low = Decimal(str(instance.evidence["second_candle_low"]))
            if latest_candle.close < engulfing_low:
                return {
                    "invalidation_reason": "CLOSE_BELOW_LOW",
                    "engulfing_low": float(engulfing_low),
                    "invalidation_price": float(latest_candle.close),
                    "invalidation_ts": latest_candle.ts.isoformat(),
                }

        elif pattern_type == "BEARISH_ENGULFING":
            engulfing_high = Decimal(str(instance.evidence["second_candle_high"]))
            if latest_candle.close > engulfing_high:
                return {
                    "invalidation_reason": "CLOSE_ABOVE_HIGH",
                    "engulfing_high": float(engulfing_high),
                    "invalidation_price": float(latest_candle.close),
                    "invalidation_ts": latest_candle.ts.isoformat(),
                }

        return None


class MorningStarDetector(BaseCandlestickDetector):
    """
    Detect Morning Star pattern.

    Pattern: 3-candle reversal - large bearish, small indecision, large bullish.
    Bias: BULLISH (reversal from downtrend).
    """

    def __init__(self, config: MorningStarConfig | None = None):
        """
        Initialize detector.

        Args:
            config: Detector configuration (uses defaults if None)
        """
        super().__init__(pattern_code="MORNING_STAR")
        self.config = config or MorningStarConfig()

    def detect(self, window: CandleWindow) -> list[PatternSignature]:
        """
        Override detect to check 3-candle pattern.

        Args:
            window: Candle window

        Returns:
            List of detected signatures
        """
        if len(window) < 3:
            return []

        signatures = []

        # Check last 3 candles
        signature = self._check_morning_star_triplet(window, len(window) - 3)
        if signature:
            signatures.append(signature)

        return signatures

    def _check_morning_star_triplet(
        self, window: CandleWindow, index: int
    ) -> PatternSignature | None:
        """
        Check if candles at index, index+1, index+2 form Morning Star.

        Args:
            window: Candle window
            index: Index of first candle

        Returns:
            PatternSignature if Morning Star found, None otherwise
        """
        if index + 2 >= len(window):
            return None

        first = window[index]
        middle = window[index + 1]
        third = window[index + 2]

        first_metrics = compute_candle_metrics(first)
        middle_metrics = compute_candle_metrics(middle)
        third_metrics = compute_candle_metrics(third)

        # Rule 1: First candle is bearish with >= 60% body
        if not first_metrics.is_bearish or first_metrics.body_pct < self.config.min_first_body_pct:
            return None

        # Rule 2: Middle candle is small (indecision) with <= 30% body
        if middle_metrics.body_pct > self.config.max_middle_body_pct:
            return None

        # Rule 3: Third candle is bullish with >= 60% body
        if not third_metrics.is_bullish or third_metrics.body_pct < self.config.min_third_body_pct:
            return None

        # Rule 4: Middle candle gaps down from first
        gap_down = (first.close - middle.open) / first.close if first.close > 0 else Decimal("0")
        if gap_down < self.config.min_middle_gap_down:
            return None

        # Rule 5: Third candle closes >= 50% into first candle's body
        first_body_midpoint = (first.open + first.close) / Decimal("2")
        if third.close < first_body_midpoint:
            return None

        penetration = (
            (third.close - first.close) / (first.open - first.close)
            if (first.open - first.close) > 0
            else Decimal("0")
        )
        if penetration < self.config.min_third_penetration:
            return None

        evidence = {
            "first_candle_body_pct": float(first_metrics.body_pct),
            "middle_candle_body_pct": float(middle_metrics.body_pct),
            "third_candle_body_pct": float(third_metrics.body_pct),
            "gap_down_pct": float(gap_down),
            "penetration_pct": float(penetration),
            "first_candle_open": float(first.open),
            "first_candle_close": float(first.close),
            "middle_candle_open": float(middle.open),
            "middle_candle_close": float(middle.close),
            "third_candle_open": float(third.open),
            "third_candle_high": float(third.high),
            "third_candle_low": float(third.low),
            "third_candle_close": float(third.close),
        }

        return PatternSignature(
            pattern_code=self.pattern_code,
            symbol=window.symbol,
            timeframe=window.timeframe,
            start_ts=first.ts,
            end_ts=third.ts,
            confidence=self.config.base_confidence,
            evidence=evidence,
            key_points=tuple(),
        )

    def _check_candle(self, window: CandleWindow, index: int) -> PatternSignature | None:
        """
        Not used for Morning Star (3-candle pattern).

        Args:
            window: Candle window
            index: Candle index

        Returns:
            None (not applicable)
        """
        return None

    def check_confirmation(self, instance: object, window: CandleWindow) -> dict | None:
        """
        Check if Morning Star is confirmed.

        Confirmation: Next candle closes above third candle high.

        Args:
            instance: PatternInstance
            window: Recent candles

        Returns:
            Evidence dict if confirmed, None otherwise
        """
        if len(window) < 1:
            return None

        third_candle_high = Decimal(str(instance.evidence["third_candle_high"]))
        latest_candle = window[-1]

        if latest_candle.close > third_candle_high:
            return {
                "confirmation_type": "CLOSE_ABOVE_HIGH",
                "third_candle_high": float(third_candle_high),
                "confirmation_price": float(latest_candle.close),
                "confirmation_ts": latest_candle.ts.isoformat(),
            }

        return None

    def check_invalidation(self, instance: object, window: CandleWindow) -> dict | None:
        """
        Check if Morning Star is invalidated.

        Invalidation: Price closes below third candle low.

        Args:
            instance: PatternInstance
            window: Recent candles

        Returns:
            Evidence dict if invalidated, None otherwise
        """
        if len(window) < 1:
            return None

        third_candle_low = Decimal(str(instance.evidence["third_candle_low"]))
        latest_candle = window[-1]

        if latest_candle.close < third_candle_low:
            return {
                "invalidation_reason": "CLOSE_BELOW_LOW",
                "third_candle_low": float(third_candle_low),
                "invalidation_price": float(latest_candle.close),
                "invalidation_ts": latest_candle.ts.isoformat(),
            }

        return None
