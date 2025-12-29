"""
Derivatives Data Service

Service for collecting derivatives market data from Binance Futures API.
Wraps BinanceService to provide derivatives-specific data methods.

Part of Core 2: Market Research & Context Engine (ADR-0017).

Architecture:
- This service wraps BinanceService (singleton client wrapper)
- Returns raw API responses (dictionaries)
- Adapters will normalize responses to domain entities (MetricPoint)

Key Methods:
- get_mark_price(): Fetch mark price + funding rate (single call optimization)
- get_open_interest(): Fetch current open interest
- get_funding_rate_history(): Fetch historical funding rates (optional)

Usage:
    >>> service = DerivativesDataService()
    >>> mark_data = service.get_mark_price("BTCUSDT")
    >>> mark_data["markPrice"]
    '95000.00'
    >>> mark_data["lastFundingRate"]
    '0.0001'
"""

from __future__ import annotations
import logging
from typing import Dict, List, Optional
from datetime import datetime
from .binance_service import BinanceService

logger = logging.getLogger(__name__)


class DerivativesDataService:
    """
    Service for collecting derivatives data (funding rate, OI, mark price).

    This service provides a clean interface to Binance Futures API endpoints.
    It wraps the BinanceService singleton and provides domain-specific methods.

    Design Principles:
    - Thin wrapper around BinanceService (no business logic)
    - Returns raw API responses (dicts)
    - Adapter layer will normalize to domain entities
    - Error handling: let exceptions bubble up (caller handles retry/fallback)
    """

    def __init__(self, use_testnet: bool = None):
        """
        Initialize DerivativesDataService.

        Args:
            use_testnet: Override testnet setting. If None, uses settings.BINANCE_USE_TESTNET
        """
        self.binance = BinanceService(use_testnet=use_testnet)
        mode = "TESTNET" if self.binance.use_testnet else "PRODUCTION"
        logger.info(f"DerivativesDataService initialized in {mode} mode")

    def get_mark_price(self, symbol: str) -> Dict[str, any]:
        """
        Fetch current mark price and funding rate for a symbol.

        This method calls the Binance Futures /fapi/v1/premiumIndex endpoint,
        which returns BOTH mark price AND current funding rate in a single call.

        API Endpoint: GET /fapi/v1/premiumIndex
        Binance Method: client.futures_mark_price(symbol=symbol)

        Args:
            symbol: Trading pair (e.g., "BTCUSDT")

        Returns:
            Dictionary with mark price, funding rate, and timing information:
            {
                "symbol": "BTCUSDT",
                "markPrice": "95000.00",
                "indexPrice": "94995.50",
                "lastFundingRate": "0.0001",
                "nextFundingTime": 1640000000000,
                "time": 1639999900000
            }

        Raises:
            BinanceAPIException: If API call fails
            Exception: For unexpected errors

        Example:
            >>> service = DerivativesDataService()
            >>> data = service.get_mark_price("BTCUSDT")
            >>> mark_price = data["markPrice"]
            >>> funding_rate = data["lastFundingRate"]
        """
        try:
            logger.debug(f"Fetching mark price for {symbol}")
            response = self.binance.client.futures_mark_price(symbol=symbol)

            # Log successful fetch
            mark_price = response.get("markPrice", "N/A")
            funding_rate = response.get("lastFundingRate", "N/A")
            logger.info(
                f"Mark price for {symbol}: {mark_price} "
                f"(funding rate: {funding_rate})"
            )

            return response

        except Exception as e:
            logger.error(f"Failed to fetch mark price for {symbol}: {e}")
            raise

    def get_open_interest(self, symbol: str) -> Dict[str, any]:
        """
        Fetch current open interest for a symbol.

        API Endpoint: GET /fapi/v1/openInterest
        Binance Method: client.futures_open_interest(symbol=symbol)

        Args:
            symbol: Trading pair (e.g., "BTCUSDT")

        Returns:
            Dictionary with open interest data:
            {
                "openInterest": "12345.67",
                "symbol": "BTCUSDT",
                "time": 1639999900000
            }

        Raises:
            BinanceAPIException: If API call fails
            Exception: For unexpected errors

        Example:
            >>> service = DerivativesDataService()
            >>> data = service.get_open_interest("BTCUSDT")
            >>> oi = data["openInterest"]
        """
        try:
            logger.debug(f"Fetching open interest for {symbol}")
            response = self.binance.client.futures_open_interest(symbol=symbol)

            # Log successful fetch
            oi = response.get("openInterest", "N/A")
            logger.info(f"Open interest for {symbol}: {oi}")

            return response

        except Exception as e:
            logger.error(f"Failed to fetch open interest for {symbol}: {e}")
            raise

    def get_funding_rate_history(
        self,
        symbol: str,
        start_time: Optional[int] = None,
        end_time: Optional[int] = None,
        limit: int = 100,
    ) -> List[Dict[str, any]]:
        """
        Fetch historical funding rates for a symbol.

        API Endpoint: GET /fapi/v1/fundingRate
        Binance Method: client.futures_funding_rate(symbol=symbol, ...)

        Args:
            symbol: Trading pair (e.g., "BTCUSDT")
            start_time: Start timestamp in milliseconds (optional)
            end_time: End timestamp in milliseconds (optional)
            limit: Number of records to fetch (default: 100, max: 1000)

        Returns:
            List of funding rate records:
            [
                {
                    "symbol": "BTCUSDT",
                    "fundingRate": "-0.03750000",
                    "fundingTime": 1640000000000,
                    "markPrice": "95000.00"
                },
                ...
            ]

        Raises:
            BinanceAPIException: If API call fails
            Exception: For unexpected errors

        Note:
            - Without time parameters, returns most recent 100 records
            - Records are returned in ascending order by fundingTime

        Example:
            >>> service = DerivativesDataService()
            >>> history = service.get_funding_rate_history("BTCUSDT", limit=10)
            >>> latest = history[-1]  # Most recent
            >>> latest["fundingRate"]
            '0.0001'
        """
        try:
            logger.debug(
                f"Fetching funding rate history for {symbol} "
                f"(limit: {limit}, start: {start_time}, end: {end_time})"
            )

            # Build parameters
            params = {"symbol": symbol, "limit": limit}
            if start_time is not None:
                params["startTime"] = start_time
            if end_time is not None:
                params["endTime"] = end_time

            response = self.binance.client.futures_funding_rate(**params)

            # Log successful fetch
            logger.info(
                f"Fetched {len(response)} funding rate records for {symbol}"
            )

            return response

        except Exception as e:
            logger.error(
                f"Failed to fetch funding rate history for {symbol}: {e}"
            )
            raise

    def collect_all_metrics(self, symbol: str) -> Dict[str, Dict[str, any]]:
        """
        Convenience method to collect all derivatives metrics in one call.

        This method fetches:
        - Mark price (includes funding rate)
        - Open interest

        Optimized version that uses get_mark_price() for both mark price
        and funding rate (single API call instead of two).

        Args:
            symbol: Trading pair (e.g., "BTCUSDT")

        Returns:
            Dictionary with all metrics:
            {
                "mark_price_data": {...},  # Includes lastFundingRate
                "open_interest_data": {...},
            }

        Raises:
            Exception: If any API call fails

        Example:
            >>> service = DerivativesDataService()
            >>> metrics = service.collect_all_metrics("BTCUSDT")
            >>> mark_price = metrics["mark_price_data"]["markPrice"]
            >>> funding_rate = metrics["mark_price_data"]["lastFundingRate"]
            >>> oi = metrics["open_interest_data"]["openInterest"]
        """
        try:
            logger.info(f"Collecting all derivatives metrics for {symbol}")

            # Fetch mark price (includes funding rate)
            mark_price_data = self.get_mark_price(symbol)

            # Fetch open interest
            open_interest_data = self.get_open_interest(symbol)

            logger.info(f"Successfully collected all metrics for {symbol}")

            return {
                "mark_price_data": mark_price_data,
                "open_interest_data": open_interest_data,
            }

        except Exception as e:
            logger.error(
                f"Failed to collect all metrics for {symbol}: {e}"
            )
            raise

    @property
    def is_production(self) -> bool:
        """Check if service is running in production mode."""
        return self.binance.is_production
