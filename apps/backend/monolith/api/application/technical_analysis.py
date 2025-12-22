"""
Technical Analysis Module - Robson Bot

Identifies support and resistance levels for technical stop calculation.
Follows the "second technical event" rule for stop placement.
"""

from decimal import Decimal
from typing import List, Dict, Tuple, Optional
from dataclasses import dataclass
import logging

logger = logging.getLogger(__name__)


@dataclass
class TechnicalLevel:
    """Represents a support or resistance level."""
    price: Decimal
    strength: int  # Number of times price touched this level
    candle_index: int  # Index in candles array (for visualization)
    level_type: str  # "SUPPORT" or "RESISTANCE"


@dataclass
class Candle:
    """OHLCV candle data."""
    timestamp: int
    open: Decimal
    high: Decimal
    low: Decimal
    close: Decimal
    volume: Decimal


class TechnicalStopCalculator:
    """
    Calculates technical stops based on support/resistance analysis.

    Business Rule:
    - For LONG: Use second support below current price
    - For SHORT: Use second resistance above current price
    - Timeframe: 15-minute chart
    - Lookback: 100-200 candles
    """

    def __init__(self, lookback_candles: int = 150, min_touches: int = 1):
        """
        Initialize calculator.

        Args:
            lookback_candles: Number of candles to analyze (default: 150)
            min_touches: Minimum touches to consider a level valid (default: 1)
        """
        self.lookback_candles = lookback_candles
        self.min_touches = min_touches

    def calculate_stop(
        self,
        candles: List[Candle],
        current_price: Decimal,
        side: str
    ) -> Tuple[Decimal, List[TechnicalLevel]]:
        """
        Calculate technical stop price.

        Args:
            candles: List of OHLCV candles (newest first)
            current_price: Current market price
            side: "LONG" or "SHORT"

        Returns:
            Tuple of (stop_price, all_levels)

        Raises:
            ValueError: If insufficient data or invalid parameters
        """
        if len(candles) < 20:
            raise ValueError("Insufficient candle data (minimum 20 required)")

        if side not in ("LONG", "SHORT"):
            raise ValueError(f"Invalid side: {side}. Must be LONG or SHORT")

        # Reverse candles to have oldest first (easier for analysis)
        candles = list(reversed(candles[:self.lookback_candles]))

        if side == "LONG":
            levels = self._find_supports(candles, current_price)
            level_type = "SUPPORT"
        else:
            levels = self._find_resistances(candles, current_price)
            level_type = "RESISTANCE"

        # Filter valid levels (below/above current price)
        if side == "LONG":
            valid_levels = [lvl for lvl in levels if lvl.price < current_price]
            valid_levels.sort(key=lambda x: x.price, reverse=True)  # Closest first
        else:
            valid_levels = [lvl for lvl in levels if lvl.price > current_price]
            valid_levels.sort(key=lambda x: x.price)  # Closest first

        # Select SECOND level (index 1)
        if len(valid_levels) >= 2:
            stop_price = valid_levels[1].price
            logger.info(
                f"Technical stop calculated: {stop_price} "
                f"({level_type}, second event, strength={valid_levels[1].strength})"
            )
        elif len(valid_levels) == 1:
            # Only one level found, use it (safer than default)
            stop_price = valid_levels[0].price
            logger.warning(
                f"Only one {level_type} found, using it as stop: {stop_price}"
            )
        else:
            # Fallback to default 2% stop if no levels found
            if side == "LONG":
                stop_price = current_price * Decimal("0.98")
            else:
                stop_price = current_price * Decimal("1.02")

            logger.warning(
                f"No {level_type} found, using default 2% stop: {stop_price}"
            )

        return stop_price, levels

    def _find_supports(
        self,
        candles: List[Candle],
        current_price: Decimal
    ) -> List[TechnicalLevel]:
        """
        Find support levels (local minima where price bounced up).

        A support is identified when:
        - Candle low is lower than previous and next candles
        - Price bounced up after touching the support

        Args:
            candles: List of candles (oldest first)
            current_price: Current market price

        Returns:
            List of TechnicalLevel objects
        """
        supports = []
        window_size = 5  # Look at 5 candles around each point

        for i in range(window_size, len(candles) - window_size):
            current_low = candles[i].low

            # Check if current candle is a local minimum
            is_local_min = True
            for j in range(i - window_size, i + window_size + 1):
                if j != i and candles[j].low < current_low:
                    is_local_min = False
                    break

            if not is_local_min:
                continue

            # Check if price bounced up (close > low, or next candles are higher)
            bounced_up = (
                candles[i].close > candles[i].low or
                candles[i + 1].low > current_low
            )

            if bounced_up:
                # Check if this level was touched multiple times
                touches = self._count_touches(candles, current_low, tolerance=Decimal("0.005"))

                if touches >= self.min_touches:
                    supports.append(TechnicalLevel(
                        price=current_low,
                        strength=touches,
                        candle_index=i,
                        level_type="SUPPORT"
                    ))

        # Merge nearby levels (within 0.5% of each other)
        supports = self._merge_levels(supports, tolerance=Decimal("0.005"))

        logger.debug(f"Found {len(supports)} support levels")
        return supports

    def _find_resistances(
        self,
        candles: List[Candle],
        current_price: Decimal
    ) -> List[TechnicalLevel]:
        """
        Find resistance levels (local maxima where price bounced down).

        A resistance is identified when:
        - Candle high is higher than previous and next candles
        - Price bounced down after touching the resistance

        Args:
            candles: List of candles (oldest first)
            current_price: Current market price

        Returns:
            List of TechnicalLevel objects
        """
        resistances = []
        window_size = 5

        for i in range(window_size, len(candles) - window_size):
            current_high = candles[i].high

            # Check if current candle is a local maximum
            is_local_max = True
            for j in range(i - window_size, i + window_size + 1):
                if j != i and candles[j].high > current_high:
                    is_local_max = False
                    break

            if not is_local_max:
                continue

            # Check if price bounced down
            bounced_down = (
                candles[i].close < candles[i].high or
                candles[i + 1].high < current_high
            )

            if bounced_down:
                touches = self._count_touches(candles, current_high, tolerance=Decimal("0.005"))

                if touches >= self.min_touches:
                    resistances.append(TechnicalLevel(
                        price=current_high,
                        strength=touches,
                        candle_index=i,
                        level_type="RESISTANCE"
                    ))

        resistances = self._merge_levels(resistances, tolerance=Decimal("0.005"))

        logger.debug(f"Found {len(resistances)} resistance levels")
        return resistances

    def _count_touches(
        self,
        candles: List[Candle],
        level: Decimal,
        tolerance: Decimal = Decimal("0.005")
    ) -> int:
        """
        Count how many times price touched a level.

        Args:
            candles: List of candles
            level: Price level to check
            tolerance: Price tolerance (default: 0.5%)

        Returns:
            Number of touches
        """
        touches = 0
        lower_bound = level * (Decimal("1") - tolerance)
        upper_bound = level * (Decimal("1") + tolerance)

        for candle in candles:
            if lower_bound <= candle.low <= upper_bound:
                touches += 1
            elif lower_bound <= candle.high <= upper_bound:
                touches += 1

        return touches

    def _merge_levels(
        self,
        levels: List[TechnicalLevel],
        tolerance: Decimal = Decimal("0.005")
    ) -> List[TechnicalLevel]:
        """
        Merge levels that are close to each other.

        Args:
            levels: List of technical levels
            tolerance: Price tolerance (default: 0.5%)

        Returns:
            Merged list of levels
        """
        if not levels:
            return []

        # Sort by price
        levels = sorted(levels, key=lambda x: x.price)

        merged = []
        current_group = [levels[0]]

        for i in range(1, len(levels)):
            level = levels[i]
            group_avg = sum(lvl.price for lvl in current_group) / len(current_group)

            # Check if within tolerance
            if abs(level.price - group_avg) / group_avg <= tolerance:
                current_group.append(level)
            else:
                # Create merged level from group
                merged_price = sum(lvl.price for lvl in current_group) / len(current_group)
                merged_strength = sum(lvl.strength for lvl in current_group)
                merged_index = current_group[-1].candle_index  # Use latest

                merged.append(TechnicalLevel(
                    price=merged_price,
                    strength=merged_strength,
                    candle_index=merged_index,
                    level_type=current_group[0].level_type
                ))

                # Start new group
                current_group = [level]

        # Add last group
        if current_group:
            merged_price = sum(lvl.price for lvl in current_group) / len(current_group)
            merged_strength = sum(lvl.strength for lvl in current_group)
            merged_index = current_group[-1].candle_index

            merged.append(TechnicalLevel(
                price=merged_price,
                strength=merged_strength,
                candle_index=merged_index,
                level_type=current_group[0].level_type
            ))

        return merged
