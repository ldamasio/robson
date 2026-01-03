"""
Technical Stop Adapter.

Integrates the domain TechnicalStopCalculator with Binance market data.
Provides a simple interface for calculating technical stops in production.
"""

import logging
from decimal import Decimal
from typing import List, Optional

from django.conf import settings

from api.domain.technical_stop import (
    TechnicalStopCalculator,
    TechnicalStopResult,
    OHLCV,
    calculate_position_from_technical_stop,
)
from .adapters import BinanceMarketData

logger = logging.getLogger(__name__)


class BinanceTechnicalStopService:
    """
    Service that calculates technical stops using Binance market data.
    
    Usage:
        service = BinanceTechnicalStopService()
        result = service.calculate_stop(
            symbol="BTCUSDC",
            side="BUY",
            entry_price=Decimal("95000"),
        )
    """
    
    # Timeframe mapping from our format to Binance format
    TIMEFRAME_MAP = {
        "1m": "1m",
        "5m": "5m",
        "15m": "15m",
        "30m": "30m",
        "1h": "1h",
        "4h": "4h",
        "1d": "1d",
    }
    
    def __init__(
        self,
        level_n: int = 2,
        default_timeframe: str = "15m",
        lookback_periods: int = 100,
        timeout: float = 5.0,
        client_id: Optional[int] = None,
    ):
        """
        Initialize the service.

        Args:
            level_n: Which support/resistance level to use (2nd by default)
            default_timeframe: Default chart timeframe
            lookback_periods: Number of candles to analyze
            timeout: Timeout for Binance API calls in seconds (default: 5.0)
            client_id: Optional client ID for multi-tenant setup
        """
        self.level_n = level_n
        self.default_timeframe = default_timeframe
        self.lookback_periods = lookback_periods
        self.timeout = timeout
        self.client_id = client_id

        # Pass timeout to market data adapter for HTTP-level timeout
        self.market_data = BinanceMarketData(client_id=client_id, timeout=timeout)
        self.calculator = TechnicalStopCalculator(level_n=level_n)

    def calculate_stop(
        self,
        symbol: str,
        side: str,
        entry_price: Optional[Decimal] = None,
        timeframe: Optional[str] = None,
    ) -> TechnicalStopResult:
        """
        Calculate technical stop for a trade.

        Args:
            symbol: Trading pair (e.g., "BTCUSDC")
            side: "BUY" or "SELL"
            entry_price: Entry price (fetched from market if not provided)
            timeframe: Chart timeframe (default: 15m)

        Returns:
            TechnicalStopResult with stop price and context

        Raises:
            TimeoutError: If Binance API calls exceed timeout
        """
        timeframe = timeframe or self.default_timeframe

        # Get current price if not provided
        # Timeout is handled at HTTP client level (requests library)
        if entry_price is None:
            if side == "BUY":
                entry_price = self.market_data.best_ask(symbol)
            else:
                entry_price = self.market_data.best_bid(symbol)

        # Fetch OHLCV data
        # Timeout is handled at HTTP client level (requests library)
        candles = self._fetch_ohlcv(symbol, timeframe, self.lookback_periods)
        
        if not candles:
            logger.warning(f"No OHLCV data for {symbol} {timeframe}")
            # Return a fallback result
            return self.calculator._fallback_stop(
                entry_price, 
                side, 
                timeframe,
                ["Failed to fetch OHLCV data"],
            )
        
        # Calculate technical stop
        result = self.calculator.calculate(
            candles=candles,
            entry_price=entry_price,
            side=side,
            timeframe=timeframe,
        )
        
        logger.info(
            f"Technical stop calculated: {symbol} {side} "
            f"entry={entry_price} stop={result.stop_price} "
            f"method={result.method_used.value} confidence={result.confidence.value}"
        )
        
        return result
    
    def calculate_position_with_technical_stop(
        self,
        symbol: str,
        side: str,
        capital: Decimal,
        entry_price: Optional[Decimal] = None,
        timeframe: Optional[str] = None,
        max_risk_percent: Decimal = Decimal("1.0"),
    ) -> dict:
        """
        Calculate both technical stop AND position size in one call.
        
        This is the main method for risk-managed trading.
        
        Returns:
            Dictionary with all trade parameters:
            - stop_result: TechnicalStopResult
            - quantity: Position size
            - risk_amount: Amount at risk
            - position_value: Total position value
        """
        # Get technical stop
        stop_result = self.calculate_stop(symbol, side, entry_price, timeframe)
        
        # Use the entry price from the result (may have been fetched)
        entry = stop_result.entry_price
        
        # Calculate position size
        quantity, risk_amount, position_value = calculate_position_from_technical_stop(
            capital=capital,
            entry_price=entry,
            technical_stop=stop_result.stop_price,
            max_risk_percent=max_risk_percent,
        )
        
        return {
            "symbol": symbol,
            "side": side,
            "entry_price": entry,
            "stop_price": stop_result.stop_price,
            "stop_distance": stop_result.stop_distance,
            "stop_distance_pct": stop_result.stop_distance_pct,
            "quantity": quantity,
            "position_value": position_value,
            "risk_amount": risk_amount,
            "risk_percent": max_risk_percent,
            "capital": capital,
            "method_used": stop_result.method_used.value,
            "confidence": stop_result.confidence.value,
            "levels_found": len(stop_result.levels_found),
            "timeframe": stop_result.timeframe,
            "warnings": stop_result.warnings,
            "stop_result": stop_result,
        }
    
    def _fetch_ohlcv(
        self,
        symbol: str,
        timeframe: str,
        limit: int,
    ) -> List[OHLCV]:
        """Fetch OHLCV data from Binance."""
        try:
            # Map timeframe
            binance_interval = self.TIMEFRAME_MAP.get(timeframe, "15m")

            # Fetch klines using market data adapter
            klines = self.market_data.get_klines(
                symbol=symbol,
                interval=binance_interval,
                limit=limit,
            )
            
            # Convert to OHLCV objects
            candles = []
            for k in klines:
                candles.append(OHLCV(
                    timestamp=k[0],
                    open=Decimal(str(k[1])),
                    high=Decimal(str(k[2])),
                    low=Decimal(str(k[3])),
                    close=Decimal(str(k[4])),
                    volume=Decimal(str(k[5])),
                ))
            
            return candles
            
        except Exception as e:
            logger.error(f"Failed to fetch OHLCV: {e}")
            return []
    
    def get_support_resistance_levels(
        self,
        symbol: str,
        timeframe: str = "15m",
        current_price: Optional[Decimal] = None,
    ) -> dict:
        """
        Get all support and resistance levels for a symbol.
        
        Useful for displaying on charts or analysis.
        """
        if current_price is None:
            current_price = self.market_data.best_ask(symbol)
        
        candles = self._fetch_ohlcv(symbol, timeframe, self.lookback_periods)
        
        if not candles:
            return {"supports": [], "resistances": [], "current_price": str(current_price)}
        
        supports = self.calculator._find_support_levels(candles, current_price)
        resistances = self.calculator._find_resistance_levels(candles, current_price)
        
        return {
            "supports": [
                {
                    "price": str(s.price),
                    "touches": s.touches,
                    "strength": s.strength,
                }
                for s in supports
            ],
            "resistances": [
                {
                    "price": str(r.price),
                    "touches": r.touches,
                    "strength": r.strength,
                }
                for r in resistances
            ],
            "current_price": str(current_price),
            "timeframe": timeframe,
            "candles_analyzed": len(candles),
        }

