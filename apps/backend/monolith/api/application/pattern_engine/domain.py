"""
Pattern Engine Domain Layer.

Pure Python entities - NO Django dependencies.
All timestamps MUST come from candle data, NEVER from datetime.now().
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime
from decimal import Decimal


@dataclass(frozen=True)
class OHLCV:
    """
    Immutable OHLCV candle.

    Timestamp MUST come from exchange data, NOT system clock.
    """

    ts: datetime
    open: Decimal
    high: Decimal
    low: Decimal
    close: Decimal
    volume: Decimal

    def __post_init__(self):
        """Validate candle data."""
        if self.high < self.low:
            msg = f"Invalid candle: high ({self.high}) < low ({self.low})"
            raise ValueError(msg)
        if self.open < 0 or self.high < 0 or self.low < 0 or self.close < 0:
            msg = "Negative price values not allowed"
            raise ValueError(msg)


@dataclass(frozen=True)
class CandleWindow:
    """
    Immutable sequence of OHLCV candles for analysis.

    Ordered chronologically (oldest first).
    """

    symbol: str
    timeframe: str
    candles: tuple[OHLCV, ...]  # Tuple for immutability
    start_ts: datetime
    end_ts: datetime

    def __post_init__(self):
        """Validate window."""
        if not self.candles:
            msg = "CandleWindow cannot be empty"
            raise ValueError(msg)
        if len(self.candles) > 0:
            # Verify chronological order
            for i in range(1, len(self.candles)):
                if self.candles[i].ts <= self.candles[i - 1].ts:
                    msg = f"Candles not chronologically ordered at index {i}"
                    raise ValueError(msg)

    def __len__(self) -> int:
        """Return number of candles in window."""
        return len(self.candles)

    def __getitem__(self, index: int) -> OHLCV:
        """Get candle by index."""
        return self.candles[index]


@dataclass(frozen=True)
class PivotPoint:
    """
    Pivot point (swing high/low) for chart pattern analysis.

    Used for HNS, IHNS, and other chart patterns.
    """

    ts: datetime  # From candle timestamp
    price: Decimal
    pivot_type: str  # "HIGH" or "LOW"
    bar_index: int  # Index in candle window


@dataclass(frozen=True)
class PatternSignature:
    """
    Detected pattern signature (immutable).

    Represents initial detection (FORMING state).
    """

    pattern_code: str
    symbol: str
    timeframe: str
    start_ts: datetime  # From candle timestamp
    end_ts: datetime  # From candle timestamp
    confidence: Decimal  # 0-1
    evidence: dict  # Numeric metrics (body_pct, wick_pcts, thresholds, etc.)
    key_points: tuple[PivotPoint, ...]  # For chart patterns (empty for candlestick)

    def __post_init__(self):
        """Validate signature."""
        if not (Decimal("0") <= self.confidence <= Decimal("1")):
            msg = f"Confidence must be [0,1], got {self.confidence}"
            raise ValueError(msg)


@dataclass(frozen=True)
class PatternLifecycleEvent:
    """
    Pattern lifecycle state transition event.

    All timestamps MUST come from candle data.
    """

    instance_id: int
    event_type: str  # FORMING, CONFIRMED, INVALIDATED, FAILED, TARGET_HIT
    event_ts: datetime  # From candle timestamp (NOT datetime.now())
    confidence: Decimal
    evidence: dict
    version: str  # Detector version (e.g., "pattern_engine_v1.0.0")

    def to_dict(self) -> dict:
        """Convert to dictionary for logging."""
        return {
            "instance_id": self.instance_id,
            "event_type": self.event_type,
            "event_ts": self.event_ts.isoformat(),
            "confidence": str(self.confidence),
            "evidence": self.evidence,
            "version": self.version,
        }


@dataclass(frozen=True)
class CandleMetrics:
    """
    Computed candle anatomy metrics.

    All values are Decimal for precision.
    Percentages are ratios (0-1), not 0-100.
    """

    body: Decimal
    range: Decimal
    upper_wick: Decimal
    lower_wick: Decimal
    body_pct: Decimal  # body / range
    upper_wick_pct: Decimal  # upper_wick / range
    lower_wick_pct: Decimal  # lower_wick / range
    is_bullish: bool  # close > open
    is_bearish: bool  # close < open

    def __post_init__(self):
        """Validate metrics sum to 1.0."""
        total_pct = self.body_pct + self.upper_wick_pct + self.lower_wick_pct
        # Allow small floating point tolerance
        if abs(total_pct - Decimal("1.0")) > Decimal("0.001"):
            msg = f"Percentages must sum to 1.0, got {total_pct}"
            raise ValueError(msg)
