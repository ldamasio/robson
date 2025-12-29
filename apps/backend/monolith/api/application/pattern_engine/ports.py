"""
Pattern Engine Ports (Interfaces).

Protocol definitions for dependency injection.
"""

from __future__ import annotations

from decimal import Decimal
from typing import Protocol

from .domain import CandleWindow, PatternSignature, PivotPoint


class CandleProvider(Protocol):
    """
    Interface for fetching candle data.

    Implementations:
    - BinanceCandleProvider (uses MarketDataService)
    - PersistentCandleProvider (future: queries from database)
    - TestCandleProvider (fixtures for testing)

    CRITICAL: All timestamps MUST come from exchange data, NOT system clock.
    """

    def get_candles(self, symbol: str, timeframe: str, limit: int) -> CandleWindow:
        """
        Fetch recent candles for symbol/timeframe.

        Args:
            symbol: Trading pair (e.g., "BTCUSDT")
            timeframe: Candle interval (e.g., "15m", "1h")
            limit: Number of candles to fetch

        Returns:
            CandleWindow with ordered candles (oldest first)

        Raises:
            CandleProviderError: If fetch fails or data invalid
        """
        ...


class PatternRepository(Protocol):
    """
    Interface for pattern persistence.

    Implements idempotent upserts and alert emission.

    CRITICAL: All timestamps MUST be passed in from candle data.
    Repository MUST NOT use datetime.now() for event identity.
    """

    def get_or_create_instance(
        self, signature: PatternSignature
    ) -> tuple[object, bool]:  # Returns (PatternInstance, created: bool)
        """
        Idempotent instance creation.

        Uniqueness key: (pattern_code, symbol, timeframe, start_ts)

        Args:
            signature: Pattern signature with candle timestamps

        Returns:
            (PatternInstance, created) tuple
        """
        ...

    def update_status(
        self,
        instance_id: int,
        status: str,
        event_ts: datetime,  # From candle timestamp
        evidence: dict,
    ) -> None:
        """
        Update pattern instance status.

        Args:
            instance_id: Pattern instance ID
            status: New status (CONFIRMED, INVALIDATED, etc.)
            event_ts: Event timestamp FROM CANDLE DATA
            evidence: Evidence payload
        """
        ...

    def emit_alert(
        self,
        instance_id: int,
        alert_type: str,
        alert_ts: datetime,  # From candle timestamp
        confidence: Decimal,
        payload: dict,
    ) -> tuple[object, bool]:  # Returns (PatternAlert, created: bool)
        """
        Emit pattern alert (idempotent).

        Uniqueness key: (instance_id, alert_type, alert_ts)

        CRITICAL: alert_ts MUST come from candle data, NOT datetime.now()

        Args:
            instance_id: Pattern instance ID
            alert_type: Alert type (FORMING, CONFIRM, INVALIDATE, etc.)
            alert_ts: Alert timestamp FROM CANDLE DATA
            confidence: Confidence score [0-1]
            payload: Evidence and thresholds

        Returns:
            (PatternAlert, created) tuple for idempotency tracking
        """
        ...

    def store_candlestick_detail(self, instance_id: int, metrics: dict) -> None:
        """
        Create CandlestickPatternDetail record.

        Args:
            instance_id: Pattern instance ID
            metrics: Candle metrics (body_pct, wick_pcts, etc.)
        """
        ...

    def store_chart_detail(self, instance_id: int, metrics: dict) -> None:
        """
        Create ChartPatternDetail record.

        Args:
            instance_id: Pattern instance ID
            metrics: Chart pattern metrics (neckline_slope, etc.)
        """
        ...

    def store_pattern_points(self, instance_id: int, points: list[PivotPoint]) -> None:
        """
        Create PatternPoint records (for chart patterns).

        Args:
            instance_id: Pattern instance ID
            points: List of pivot points (LS, HEAD, RS, neckline, etc.)
        """
        ...


class PatternDetector(Protocol):
    """
    Interface for pattern detection logic.

    Each detector implements:
    - detect(): Find new pattern signatures
    - check_confirmation(): Verify if FORMING pattern confirmed
    - check_invalidation(): Verify if pattern invalidated
    """

    pattern_code: str  # Must be set by implementation

    def detect(self, window: CandleWindow) -> list[PatternSignature]:
        """
        Detect pattern signatures in candle window.

        Returns:
            List of signatures (empty if none found)
        """
        ...

    def check_confirmation(self, instance: object, window: CandleWindow) -> dict | None:
        """
        Check if FORMING pattern is now confirmed.

        Args:
            instance: PatternInstance (from Django model)
            window: Recent candles for confirmation check

        Returns:
            Evidence dict if confirmed, None otherwise
        """
        ...

    def check_invalidation(self, instance: object, window: CandleWindow) -> dict | None:
        """
        Check if pattern is invalidated.

        Args:
            instance: PatternInstance (from Django model)
            window: Recent candles for invalidation check

        Returns:
            Evidence dict if invalidated, None otherwise
        """
        ...


class CandleProviderError(Exception):
    """Raised when candle fetching fails."""
