"""
Pattern Detector Base Classes.

Abstract base for all pattern detectors.
"""

from __future__ import annotations

from abc import ABC, abstractmethod

from ..domain import CandleWindow, PatternSignature


class BasePatternDetector(ABC):
    """
    Abstract base class for pattern detectors.

    All detectors must implement:
    - detect(): Find new pattern signatures
    - check_confirmation(): Verify if FORMING pattern confirmed
    - check_invalidation(): Verify if pattern invalidated
    """

    def __init__(self, pattern_code: str):
        """
        Initialize detector.

        Args:
            pattern_code: Pattern code (e.g., "HAMMER", "HNS")
        """
        self.pattern_code = pattern_code

    @abstractmethod
    def detect(self, window: CandleWindow) -> list[PatternSignature]:
        """
        Detect pattern signatures in candle window.

        Scans the window and returns all detected patterns.

        Args:
            window: Candle window to analyze

        Returns:
            List of signatures (empty if none found)

        Example:
            >>> detector = HammerDetector()
            >>> window = CandleWindow(...)
            >>> signatures = detector.detect(window)
            >>> len(signatures)
            1
        """

    @abstractmethod
    def check_confirmation(self, instance: object, window: CandleWindow) -> dict | None:
        """
        Check if FORMING pattern is now confirmed.

        For candlestick patterns:
            - Typically confirmed by next candle action
            - Example: Hammer confirmed if next candle closes above hammer high

        For chart patterns:
            - Confirmed by breakout through key level
            - Example: H&S confirmed by neckline break

        Args:
            instance: PatternInstance (from Django model)
            window: Recent candles for confirmation check

        Returns:
            Evidence dict if confirmed, None otherwise

        Example:
            >>> evidence = detector.check_confirmation(instance, window)
            >>> evidence["confirmation_price"]
            Decimal("95500.00")
        """

    @abstractmethod
    def check_invalidation(self, instance: object, window: CandleWindow) -> dict | None:
        """
        Check if pattern is invalidated.

        Invalidation rules vary by pattern:
        - Candlestick: Opposite direction move
        - Chart: Break of critical support/resistance

        Args:
            instance: PatternInstance (from Django model)
            window: Recent candles for invalidation check

        Returns:
            Evidence dict if invalidated, None otherwise

        Example:
            >>> evidence = detector.check_invalidation(instance, window)
            >>> evidence["invalidation_reason"]
            "Price broke below hammer low"
        """


class BaseCandlestickDetector(BasePatternDetector):
    """
    Base class for single-candle pattern detectors.

    Provides common logic for candlestick pattern detection.
    """

    def detect(self, window: CandleWindow) -> list[PatternSignature]:
        """
        Scan window for candlestick patterns.

        Default implementation: Check last N candles individually.

        Args:
            window: Candle window to analyze

        Returns:
            List of detected signatures
        """
        signatures = []

        # For single-candle patterns, check last candle only
        # (Multi-candle patterns like Engulfing override this method)
        if len(window) >= 1:
            signature = self._check_candle(window, len(window) - 1)
            if signature:
                signatures.append(signature)

        return signatures

    @abstractmethod
    def _check_candle(self, window: CandleWindow, index: int) -> PatternSignature | None:
        """
        Check if candle at index matches pattern.

        Args:
            window: Candle window
            index: Index of candle to check

        Returns:
            PatternSignature if pattern found, None otherwise
        """


class BaseChartDetector(BasePatternDetector):
    """
    Base class for chart pattern detectors.

    Provides common logic for multi-bar chart patterns.
    """

    def detect(self, window: CandleWindow) -> list[PatternSignature]:
        """
        Scan window for chart patterns.

        Default implementation: Find pivots, then check for pattern.

        Args:
            window: Candle window to analyze

        Returns:
            List of detected signatures
        """
        # Chart patterns require pivot detection
        # Implementation varies by pattern (H&S, IHNS, etc.)
        # Each detector overrides this method
        return []
