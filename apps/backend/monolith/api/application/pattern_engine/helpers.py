"""
Pattern Engine Helper Functions.

Pure functions for candle analysis - NO side effects, NO Django dependencies.
All functions are deterministic and use Decimal precision.
"""

from __future__ import annotations

from decimal import Decimal

from .domain import OHLCV, CandleMetrics, CandleWindow, PivotPoint


def compute_candle_metrics(candle: OHLCV) -> CandleMetrics:
    """
    Compute candle anatomy metrics.

    Args:
        candle: OHLCV candle to analyze

    Returns:
        CandleMetrics with body_pct, wick_pcts, etc.

    Example:
        >>> candle = OHLCV(ts=..., open=100, high=110, low=95, close=105, volume=1000)
        >>> metrics = compute_candle_metrics(candle)
        >>> metrics.body_pct  # (105-100) / (110-95) = 0.333...
    """
    candle_range = candle.high - candle.low

    # Handle doji (zero range) - all percentages = 0
    if candle_range == 0:
        return CandleMetrics(
            body=Decimal("0"),
            range=Decimal("0"),
            upper_wick=Decimal("0"),
            lower_wick=Decimal("0"),
            body_pct=Decimal("0"),
            upper_wick_pct=Decimal("0"),
            lower_wick_pct=Decimal("0"),
            is_bullish=False,
            is_bearish=False,
        )

    # Compute absolute values
    body = abs(candle.close - candle.open)
    is_bullish = candle.close > candle.open
    is_bearish = candle.close < candle.open

    # Wick calculations
    if is_bullish:
        upper_wick = candle.high - candle.close
        lower_wick = candle.open - candle.low
    elif is_bearish:
        upper_wick = candle.high - candle.open
        lower_wick = candle.close - candle.low
    else:
        # Doji with range (open == close but high != low)
        upper_wick = candle.high - candle.close
        lower_wick = candle.close - candle.low

    # Compute percentages
    body_pct = body / candle_range
    upper_wick_pct = upper_wick / candle_range
    lower_wick_pct = lower_wick / candle_range

    return CandleMetrics(
        body=body,
        range=candle_range,
        upper_wick=upper_wick,
        lower_wick=lower_wick,
        body_pct=body_pct,
        upper_wick_pct=upper_wick_pct,
        lower_wick_pct=lower_wick_pct,
        is_bullish=is_bullish,
        is_bearish=is_bearish,
    )


def find_pivots(window: CandleWindow, k: int = 3, pivot_type: str = "HIGH") -> list[PivotPoint]:
    """
    Find pivot points (swing highs/lows) using fractal window algorithm.

    A pivot HIGH at index i exists if:
        candles[i].high > candles[i-k:i].high AND candles[i].high > candles[i+1:i+k+1].high

    A pivot LOW at index i exists if:
        candles[i].low < candles[i-k:i].low AND candles[i].low < candles[i+1:i+k+1].low

    Args:
        window: Candle window to analyze
        k: Fractal window size (default 3)
        pivot_type: "HIGH" or "LOW"

    Returns:
        List of PivotPoint objects (chronologically ordered)

    Example:
        >>> window = CandleWindow(...)
        >>> pivots = find_pivots(window, k=3, pivot_type="HIGH")
        >>> len(pivots)  # Number of swing highs found
        5
    """
    if len(window) < (2 * k + 1):
        return []  # Not enough data for pivot detection

    pivots: list[PivotPoint] = []

    for i in range(k, len(window) - k):
        candle = window[i]

        if pivot_type == "HIGH":
            # Check if this is a swing high
            is_pivot = True

            # Check left window
            for j in range(i - k, i):
                if window[j].high >= candle.high:
                    is_pivot = False
                    break

            # Check right window
            if is_pivot:
                for j in range(i + 1, i + k + 1):
                    if window[j].high >= candle.high:
                        is_pivot = False
                        break

            if is_pivot:
                pivots.append(
                    PivotPoint(
                        ts=candle.ts,
                        price=candle.high,
                        pivot_type="HIGH",
                        bar_index=i,
                    )
                )

        elif pivot_type == "LOW":
            # Check if this is a swing low
            is_pivot = True

            # Check left window
            for j in range(i - k, i):
                if window[j].low <= candle.low:
                    is_pivot = False
                    break

            # Check right window
            if is_pivot:
                for j in range(i + 1, i + k + 1):
                    if window[j].low <= candle.low:
                        is_pivot = False
                        break

            if is_pivot:
                pivots.append(
                    PivotPoint(
                        ts=candle.ts,
                        price=candle.low,
                        pivot_type="LOW",
                        bar_index=i,
                    )
                )

    return pivots


def calculate_neckline_slope(point1: PivotPoint, point2: PivotPoint) -> Decimal:
    """
    Calculate slope percentage for neckline.

    Slope % = (price2 - price1) / price1

    Args:
        point1: First neckline point (chronologically earlier)
        point2: Second neckline point (chronologically later)

    Returns:
        Slope percentage (can be positive or negative)

    Example:
        >>> p1 = PivotPoint(ts=..., price=Decimal("100"), ...)
        >>> p2 = PivotPoint(ts=..., price=Decimal("102"), ...)
        >>> calculate_neckline_slope(p1, p2)
        Decimal("0.02")  # 2% upward slope
    """
    if point1.price == 0:
        return Decimal("0")

    slope_pct = (point2.price - point1.price) / point1.price
    return slope_pct


def calculate_head_prominence(
    head: PivotPoint, left_shoulder: PivotPoint, right_shoulder: PivotPoint
) -> Decimal:
    """
    Calculate head prominence for H&S pattern.

    For bearish H&S (swing highs):
        Prominence % = (head_high - avg_shoulder_high) / avg_shoulder_high

    For bullish IHNS (swing lows):
        Prominence % = (avg_shoulder_low - head_low) / avg_shoulder_low

    Args:
        head: Head pivot point
        left_shoulder: Left shoulder pivot point
        right_shoulder: Right shoulder pivot point

    Returns:
        Prominence percentage (should be positive for valid pattern)

    Example:
        >>> head = PivotPoint(price=Decimal("110"), pivot_type="HIGH", ...)
        >>> ls = PivotPoint(price=Decimal("100"), pivot_type="HIGH", ...)
        >>> rs = PivotPoint(price=Decimal("102"), pivot_type="HIGH", ...)
        >>> calculate_head_prominence(head, ls, rs)
        Decimal("0.0891...")  # ~8.9% prominence
    """
    avg_shoulder_price = (left_shoulder.price + right_shoulder.price) / Decimal("2")

    if avg_shoulder_price == 0:
        return Decimal("0")

    if head.pivot_type == "HIGH":
        # Bearish H&S - head should be ABOVE shoulders
        prominence = (head.price - avg_shoulder_price) / avg_shoulder_price
    else:
        # Bullish IHNS - head should be BELOW shoulders
        prominence = (avg_shoulder_price - head.price) / avg_shoulder_price

    return prominence


def calculate_shoulder_symmetry(left_shoulder: PivotPoint, right_shoulder: PivotPoint) -> Decimal:
    """
    Calculate shoulder symmetry (height difference as percentage).

    Symmetry % = |ls_price - rs_price| / avg_price

    Lower values indicate better symmetry.

    Args:
        left_shoulder: Left shoulder pivot point
        right_shoulder: Right shoulder pivot point

    Returns:
        Symmetry percentage (0 = perfect symmetry)

    Example:
        >>> ls = PivotPoint(price=Decimal("100"), ...)
        >>> rs = PivotPoint(price=Decimal("102"), ...)
        >>> calculate_shoulder_symmetry(ls, rs)
        Decimal("0.0198...")  # ~2% asymmetry
    """
    avg_price = (left_shoulder.price + right_shoulder.price) / Decimal("2")

    if avg_price == 0:
        return Decimal("0")

    symmetry = abs(left_shoulder.price - right_shoulder.price) / avg_price
    return symmetry


def validate_pivot_spacing(pivots: list[PivotPoint], min_bars_between: int) -> bool:
    """
    Validate that pivots are spaced sufficiently apart.

    Args:
        pivots: List of pivot points (chronologically ordered)
        min_bars_between: Minimum bars required between consecutive pivots

    Returns:
        True if all pivots meet spacing requirement

    Example:
        >>> pivots = [
        ...     PivotPoint(bar_index=10, ...),
        ...     PivotPoint(bar_index=20, ...),
        ...     PivotPoint(bar_index=30, ...),
        ... ]
        >>> validate_pivot_spacing(pivots, min_bars_between=5)
        True
    """
    if len(pivots) < 2:
        return True

    for i in range(1, len(pivots)):
        spacing = pivots[i].bar_index - pivots[i - 1].bar_index
        if spacing < min_bars_between:
            return False

    return True


def get_latest_price(window: CandleWindow) -> Decimal:
    """
    Get most recent closing price from window.

    Args:
        window: Candle window

    Returns:
        Latest close price

    Example:
        >>> window = CandleWindow(candles=(c1, c2, c3), ...)
        >>> get_latest_price(window)
        c3.close
    """
    return window.candles[-1].close


def price_breaks_level(
    current_price: Decimal, level: Decimal, direction: str, threshold_pct: Decimal
) -> bool:
    """
    Check if price has broken through a level with threshold.

    Args:
        current_price: Current market price
        level: Price level to check (e.g., neckline, stop)
        direction: "ABOVE" or "BELOW"
        threshold_pct: Percentage threshold for confirmation (e.g., 0.002 for 0.2%)

    Returns:
        True if price has broken level with threshold

    Example:
        >>> price_breaks_level(Decimal("100"), Decimal("99"), "ABOVE", Decimal("0.01"))
        True  # 100 is > 99 * 1.01 = 99.99
    """
    if direction == "ABOVE":
        required_price = level * (Decimal("1") + threshold_pct)
        return current_price > required_price
    if direction == "BELOW":
        required_price = level * (Decimal("1") - threshold_pct)
        return current_price < required_price
    return False
