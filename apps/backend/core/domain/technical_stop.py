"""
Technical Stop-Loss Calculator.

Calculates stop-loss levels based on technical analysis of price action,
NOT arbitrary percentage-based stops.

The stop should be placed where the trade thesis is invalidated:
- Below key support levels (for LONG)
- Above key resistance levels (for SHORT)

This module is PURE DOMAIN LOGIC - no framework dependencies.

================================================================================
GOLDEN RULE: POSITION SIZE IS DERIVED FROM THE TECHNICAL STOP
================================================================================

The position size is NEVER arbitrary. It is ALWAYS calculated backwards
from the technical stop-loss level determined by chart analysis.

THE CORRECT ORDER OF OPERATIONS:
1. FIRST  → Analyze chart → Find 2nd support level → This is TECHNICAL STOP
2. THEN   → Calculate STOP DISTANCE = |Entry Price - Technical Stop|
3. THEN   → Calculate MAX RISK = Capital × 1%
4. FINALLY → DERIVE POSITION SIZE = Max Risk / Stop Distance

THE FORMULA:
    Position Size = (Capital × 1%) / |Entry Price - Technical Stop|

EXAMPLE:
    Capital         = $10,000
    Entry Price     = $95,000
    Technical Stop  = $93,500 (2nd support on 15m chart)
    
    Stop Distance   = $1,500
    Max Risk (1%)   = $100
    Position Size   = $100 / $1,500 = 0.0667 BTC
    
    If stopped: Loss = 0.0667 × $1,500 = $100 = 1% ✓

KEY INSIGHT:
- Wide technical stop  → Smaller position size
- Tight technical stop → Larger position size  
- Risk amount stays CONSTANT at 1%

FOR AI AGENTS:
- ❌ NEVER ask "how much do you want to invest?"
- ✅ ALWAYS ask "where is your technical invalidation level?"
- The investment amount is CALCULATED, not chosen
- The stop comes from CHART ANALYSIS, not arbitrary percentage

See: docs/requirements/POSITION-SIZING-GOLDEN-RULE.md
================================================================================
"""

from __future__ import annotations
from dataclasses import dataclass, field
from decimal import Decimal
from typing import List, Optional, Tuple
from enum import Enum


class StopMethod(Enum):
    """Method used to determine technical stop."""
    SUPPORT_RESISTANCE = "support_resistance"
    SWING_POINT = "swing_point"
    ATR = "atr"
    FALLBACK_PERCENT = "fallback_percent"


class Confidence(Enum):
    """Confidence level of the technical stop."""
    HIGH = "high"      # Clear, well-tested level
    MEDIUM = "medium"  # Level detected but fewer confirmations
    LOW = "low"        # Fallback method used


@dataclass(frozen=True)
class PriceLevel:
    """
    A significant price level (support or resistance).
    
    Attributes:
        price: The price level
        touches: Number of times price touched this level
        level_type: "support" or "resistance"
        strength: Calculated strength score (0-100)
    """
    price: Decimal
    touches: int
    level_type: str  # "support" or "resistance"
    strength: int = 0  # 0-100

    def __str__(self) -> str:
        return f"{self.level_type.upper()} @ {self.price} (touches: {self.touches}, strength: {self.strength})"


@dataclass(frozen=True)
class OHLCV:
    """Single candlestick data."""
    timestamp: int
    open: Decimal
    high: Decimal
    low: Decimal
    close: Decimal
    volume: Decimal


@dataclass
class TechnicalStopResult:
    """
    Result of technical stop calculation.
    
    Contains the stop price and all context about how it was derived.
    """
    stop_price: Decimal
    entry_price: Decimal
    side: str  # "BUY" or "SELL"
    
    stop_distance: Decimal = Decimal("0")
    stop_distance_pct: Decimal = Decimal("0")
    
    method_used: StopMethod = StopMethod.FALLBACK_PERCENT
    confidence: Confidence = Confidence.LOW
    
    levels_found: List[PriceLevel] = field(default_factory=list)
    selected_level: Optional[PriceLevel] = None
    
    atr_value: Optional[Decimal] = None
    timeframe: str = "15m"
    lookback_periods: int = 100
    
    warnings: List[str] = field(default_factory=list)
    
    def __post_init__(self):
        """Calculate derived fields."""
        if self.stop_distance == Decimal("0"):
            self.stop_distance = abs(self.entry_price - self.stop_price)
        if self.stop_distance_pct == Decimal("0") and self.entry_price > 0:
            self.stop_distance_pct = (self.stop_distance / self.entry_price) * Decimal("100")
    
    def is_valid(self) -> bool:
        """Check if the stop is valid for the trade direction."""
        if self.side == "BUY":
            return self.stop_price < self.entry_price
        else:  # SELL
            return self.stop_price > self.entry_price
    
    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization."""
        return {
            "stop_price": str(self.stop_price),
            "entry_price": str(self.entry_price),
            "side": self.side,
            "stop_distance": str(self.stop_distance),
            "stop_distance_pct": str(self.stop_distance_pct),
            "method_used": self.method_used.value,
            "confidence": self.confidence.value,
            "levels_found": [
                {
                    "price": str(lvl.price),
                    "touches": lvl.touches,
                    "type": lvl.level_type,
                    "strength": lvl.strength,
                }
                for lvl in self.levels_found
            ],
            "selected_level": {
                "price": str(self.selected_level.price),
                "touches": self.selected_level.touches,
                "type": self.selected_level.level_type,
            } if self.selected_level else None,
            "atr_value": str(self.atr_value) if self.atr_value else None,
            "timeframe": self.timeframe,
            "warnings": self.warnings,
        }


class TechnicalStopCalculator:
    """
    Calculates technical stop-loss levels from price data.
    
    This is the core algorithm for determining where to place stops
    based on technical analysis rather than arbitrary percentages.
    
    Usage:
        calculator = TechnicalStopCalculator()
        result = calculator.calculate(
            candles=ohlcv_data,
            entry_price=Decimal("95000"),
            side="BUY",
        )
    """
    
    def __init__(
        self,
        level_n: int = 2,
        min_touches: int = 2,
        level_tolerance_pct: Decimal = Decimal("0.5"),
        atr_period: int = 14,
        atr_multiplier: Decimal = Decimal("1.5"),
        min_stop_pct: Decimal = Decimal("0.1"),
        max_stop_pct: Decimal = Decimal("10.0"),
    ):
        """
        Initialize the calculator.
        
        Args:
            level_n: Which support/resistance level to use (2 = second level)
            min_touches: Minimum touches to confirm a level
            level_tolerance_pct: Price tolerance for grouping levels (%)
            atr_period: Periods for ATR calculation
            atr_multiplier: Multiplier for ATR-based stop
            min_stop_pct: Minimum stop distance (%)
            max_stop_pct: Maximum stop distance (%)
        """
        self.level_n = level_n
        self.min_touches = min_touches
        self.level_tolerance_pct = level_tolerance_pct
        self.atr_period = atr_period
        self.atr_multiplier = atr_multiplier
        self.min_stop_pct = min_stop_pct
        self.max_stop_pct = max_stop_pct
    
    def calculate(
        self,
        candles: List[OHLCV],
        entry_price: Decimal,
        side: str,
        timeframe: str = "15m",
    ) -> TechnicalStopResult:
        """
        Calculate the technical stop-loss level.
        
        Args:
            candles: List of OHLCV candlestick data
            entry_price: Intended entry price
            side: "BUY" or "SELL"
            timeframe: Chart timeframe for context
            
        Returns:
            TechnicalStopResult with stop price and context
        """
        warnings = []
        
        if len(candles) < self.atr_period:
            warnings.append(f"Insufficient data: {len(candles)} candles (need {self.atr_period})")
            return self._fallback_stop(entry_price, side, timeframe, warnings)
        
        # Step 1: Find support/resistance levels
        if side == "BUY":
            levels = self._find_support_levels(candles, entry_price)
        else:
            levels = self._find_resistance_levels(candles, entry_price)
        
        # Step 2: Select the Nth level
        if len(levels) >= self.level_n:
            selected_level = levels[self.level_n - 1]
            stop_price = selected_level.price
            
            # Add buffer below support (for LONG) or above resistance (for SHORT)
            buffer = entry_price * Decimal("0.001")  # 0.1% buffer
            if side == "BUY":
                stop_price = stop_price - buffer
            else:
                stop_price = stop_price + buffer
            
            return TechnicalStopResult(
                stop_price=stop_price,
                entry_price=entry_price,
                side=side,
                method_used=StopMethod.SUPPORT_RESISTANCE,
                confidence=Confidence.HIGH if selected_level.touches >= 3 else Confidence.MEDIUM,
                levels_found=levels,
                selected_level=selected_level,
                timeframe=timeframe,
                warnings=warnings,
            )
        
        # Step 3: Try swing points
        warnings.append(f"Only {len(levels)} levels found, need {self.level_n}. Trying swing points.")
        
        swing_stop = self._find_swing_stop(candles, entry_price, side)
        if swing_stop:
            return TechnicalStopResult(
                stop_price=swing_stop,
                entry_price=entry_price,
                side=side,
                method_used=StopMethod.SWING_POINT,
                confidence=Confidence.MEDIUM,
                levels_found=levels,
                timeframe=timeframe,
                warnings=warnings,
            )
        
        # Step 4: Fall back to ATR
        warnings.append("No swing point found. Using ATR-based stop.")
        
        atr = self._calculate_atr(candles)
        if atr and atr > 0:
            atr_distance = atr * self.atr_multiplier
            if side == "BUY":
                stop_price = entry_price - atr_distance
            else:
                stop_price = entry_price + atr_distance
            
            return TechnicalStopResult(
                stop_price=stop_price,
                entry_price=entry_price,
                side=side,
                method_used=StopMethod.ATR,
                confidence=Confidence.LOW,
                levels_found=levels,
                atr_value=atr,
                timeframe=timeframe,
                warnings=warnings,
            )
        
        # Step 5: Ultimate fallback
        warnings.append("ATR calculation failed. Using percentage fallback.")
        return self._fallback_stop(entry_price, side, timeframe, warnings)
    
    def _find_support_levels(
        self,
        candles: List[OHLCV],
        current_price: Decimal,
    ) -> List[PriceLevel]:
        """Find support levels below current price."""
        # Extract lows
        lows = [(c.low, i) for i, c in enumerate(candles)]
        
        # Find local minima (swing lows)
        swing_lows = []
        for i in range(2, len(lows) - 2):
            price = lows[i][0]
            if (price < lows[i-1][0] and price < lows[i-2][0] and
                price < lows[i+1][0] and price < lows[i+2][0]):
                swing_lows.append(price)
        
        # Group nearby levels
        levels = self._group_levels(swing_lows, current_price, "support")
        
        # Filter to only levels below current price
        levels = [l for l in levels if l.price < current_price]
        
        # Sort by price descending (closest first)
        levels.sort(key=lambda x: x.price, reverse=True)
        
        return levels
    
    def _find_resistance_levels(
        self,
        candles: List[OHLCV],
        current_price: Decimal,
    ) -> List[PriceLevel]:
        """Find resistance levels above current price."""
        # Extract highs
        highs = [(c.high, i) for i, c in enumerate(candles)]
        
        # Find local maxima (swing highs)
        swing_highs = []
        for i in range(2, len(highs) - 2):
            price = highs[i][0]
            if (price > highs[i-1][0] and price > highs[i-2][0] and
                price > highs[i+1][0] and price > highs[i+2][0]):
                swing_highs.append(price)
        
        # Group nearby levels
        levels = self._group_levels(swing_highs, current_price, "resistance")
        
        # Filter to only levels above current price
        levels = [l for l in levels if l.price > current_price]
        
        # Sort by price ascending (closest first)
        levels.sort(key=lambda x: x.price)
        
        return levels
    
    def _group_levels(
        self,
        prices: List[Decimal],
        reference_price: Decimal,
        level_type: str,
    ) -> List[PriceLevel]:
        """Group nearby prices into levels."""
        if not prices:
            return []
        
        tolerance = reference_price * (self.level_tolerance_pct / Decimal("100"))
        
        # Sort prices
        sorted_prices = sorted(prices)
        
        # Group nearby prices
        groups: List[List[Decimal]] = []
        current_group: List[Decimal] = [sorted_prices[0]]
        
        for price in sorted_prices[1:]:
            if price - current_group[-1] <= tolerance:
                current_group.append(price)
            else:
                groups.append(current_group)
                current_group = [price]
        groups.append(current_group)
        
        # Convert groups to PriceLevels
        levels = []
        for group in groups:
            if len(group) >= self.min_touches:
                avg_price = sum(group) / len(group)
                strength = min(100, len(group) * 20)  # More touches = stronger
                levels.append(PriceLevel(
                    price=avg_price.quantize(Decimal("0.01")),
                    touches=len(group),
                    level_type=level_type,
                    strength=strength,
                ))
        
        return levels
    
    def _find_swing_stop(
        self,
        candles: List[OHLCV],
        entry_price: Decimal,
        side: str,
    ) -> Optional[Decimal]:
        """Find the most recent swing point for stop placement."""
        if len(candles) < 5:
            return None
        
        # Look at recent candles for swing points
        recent = candles[-20:]  # Last 20 candles
        
        if side == "BUY":
            # Find lowest low in recent candles
            lowest = min(c.low for c in recent)
            if lowest < entry_price:
                # Add small buffer
                buffer = entry_price * Decimal("0.001")
                return lowest - buffer
        else:
            # Find highest high in recent candles
            highest = max(c.high for c in recent)
            if highest > entry_price:
                buffer = entry_price * Decimal("0.001")
                return highest + buffer
        
        return None
    
    def _calculate_atr(self, candles: List[OHLCV]) -> Optional[Decimal]:
        """Calculate Average True Range."""
        if len(candles) < self.atr_period + 1:
            return None
        
        true_ranges = []
        for i in range(1, len(candles)):
            high = candles[i].high
            low = candles[i].low
            prev_close = candles[i-1].close
            
            tr = max(
                high - low,
                abs(high - prev_close),
                abs(low - prev_close),
            )
            true_ranges.append(tr)
        
        # Calculate ATR (simple moving average of TR)
        recent_tr = true_ranges[-self.atr_period:]
        atr = sum(recent_tr) / len(recent_tr)
        
        return atr
    
    def _fallback_stop(
        self,
        entry_price: Decimal,
        side: str,
        timeframe: str,
        warnings: List[str],
    ) -> TechnicalStopResult:
        """Create a fallback percentage-based stop."""
        # Use 2% as default fallback
        fallback_pct = Decimal("2.0")
        distance = entry_price * (fallback_pct / Decimal("100"))
        
        if side == "BUY":
            stop_price = entry_price - distance
        else:
            stop_price = entry_price + distance
        
        warnings.append(f"Using {fallback_pct}% fallback stop")
        
        return TechnicalStopResult(
            stop_price=stop_price,
            entry_price=entry_price,
            side=side,
            method_used=StopMethod.FALLBACK_PERCENT,
            confidence=Confidence.LOW,
            timeframe=timeframe,
            warnings=warnings,
        )


def calculate_position_from_technical_stop(
    capital: Decimal,
    entry_price: Decimal,
    technical_stop: Decimal,
    max_risk_percent: Decimal = Decimal("1.0"),
) -> Tuple[Decimal, Decimal, Decimal]:
    """
    Calculate position size from technical stop level.
    
    ⚠️ THIS IS THE GOLDEN RULE IMPLEMENTATION ⚠️
    
    Position Size = (Capital × Risk%) / |Entry - Technical Stop|
    
    The position size is DERIVED from the technical stop, never arbitrary.
    The technical stop comes FIRST (from chart analysis), then we calculate
    how much we can buy while risking exactly 1% of capital.
    
    Args:
        capital: Total available capital (e.g., $10,000)
        entry_price: Entry price (e.g., $95,000)
        technical_stop: Stop price from chart analysis (e.g., $93,500)
        max_risk_percent: Maximum risk per trade (default: 1%)
        
    Returns:
        Tuple of:
        - quantity: Position size in base asset (e.g., 0.0667 BTC)
        - risk_amount: Amount at risk in quote (e.g., $100)
        - position_value: Total position value (e.g., $6,333)
    
    Example:
        >>> qty, risk, value = calculate_position_from_technical_stop(
        ...     capital=Decimal("10000"),
        ...     entry_price=Decimal("95000"),
        ...     technical_stop=Decimal("93500"),  # From chart!
        ... )
        >>> print(f"Buy {qty} BTC, risking ${risk}")
        Buy 0.0667 BTC, risking $100
    """
    stop_distance = abs(entry_price - technical_stop)
    
    if stop_distance == 0:
        return Decimal("0"), Decimal("0"), Decimal("0")
    
    max_risk_amount = capital * (max_risk_percent / Decimal("100"))
    quantity = (max_risk_amount / stop_distance).quantize(Decimal("0.00001"))
    position_value = quantity * entry_price
    
    return quantity, max_risk_amount, position_value

